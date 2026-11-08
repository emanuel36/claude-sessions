//! The event loop the panes run on: terminal input, inotify and a computed wake-up time.
//!
//! A pane's loop asks its [`Schedule`] when it must next wake up and waits on
//! [`Runtime::next`] until then. Two threads feed the one channel it waits on, each blocked in
//! a read: the terminal input thread and the inotify thread. A hidden pane has no deadline and
//! no watches, so it sleeps until tmux sends it a key.

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Stdout};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, Once, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, MouseEvent};
use crossterm::{cursor, execute, terminal, Command};
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask, Watches};
use ratatui::backend::CrosstermBackend;
use ratatui::{Frame, Terminal};

/// One thing the pane's loop wakes up for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// A key press or repeat (releases are dropped).
    Key(KeyEvent),
    /// A mouse event.
    Mouse(MouseEvent),
    /// The terminal changed size.
    Resize,
    /// An entry of a watched directory was created, written, renamed into it or deleted: its
    /// full path. The directory itself when the kernel's event queue overflowed, or when the
    /// directory was deleted and its watch is gone (watch it again once it is back).
    Changed(PathBuf),
}

/// What the threads send the pane's loop.
#[derive(Debug)]
enum Feed {
    /// Something to wake up for.
    Msg(Msg),
    /// The terminal is gone: no key, click or resize will come any more.
    InputEnded,
}

/// What tmux tells the pane by sending it a function key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// The pane's window came on screen (F12).
    Shown,
    /// The pane's window left the screen (F11).
    Hidden,
    /// Windows or panes changed: list them again (F10).
    Layout,
}

/// Maps the function keys tmux sends to the sidebar and landing panes to a signal.
pub fn signal(key: &KeyEvent) -> Option<Signal> {
    match key.code {
        KeyCode::F(12) => Some(Signal::Shown),
        KeyCode::F(11) => Some(Signal::Hidden),
        KeyCode::F(10) => Some(Signal::Layout),
        _ => None,
    }
}

/// The current time as seconds since the Unix epoch.
pub(crate) fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The events a watched directory reports: enough to see a file written in place, appended
/// to, written atomically (renamed into the directory) or removed.
const WATCH_MASK: WatchMask = WatchMask::CREATE
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::DELETE)
    .union(WatchMask::MODIFY);

/// Room for at least 15 events with the longest file name (16 bytes + 256 each).
const EVENT_BUFFER: usize = 4096;

/// The watched directories, by path and by watch descriptor.
///
/// Shared by the thread that adds and removes watches and the thread that reads events. The
/// lock is held across `inotify_add_watch` and the insert, so the reader never sees an event
/// for a watch it cannot name yet.
#[derive(Debug, Default)]
struct Watched {
    by_path: HashMap<PathBuf, WatchDescriptor>,
    by_wd: HashMap<WatchDescriptor, PathBuf>,
}

impl Watched {
    /// Adds the paths one event stands for to `out`, once each.
    fn changes(&mut self, event: inotify::Event<&OsStr>, out: &mut Vec<PathBuf>) {
        let mut push = |path: PathBuf| {
            if !out.contains(&path) {
                out.push(path);
            }
        };
        if event.mask.contains(EventMask::Q_OVERFLOW) {
            // Events were lost: every watched directory may have changed
            self.by_path.keys().cloned().for_each(push);
        } else if event.mask.contains(EventMask::IGNORED) {
            // The kernel dropped a watch `unwatch_all` did not remove, as it does when the
            // directory is deleted: the pane learns of it to watch the directory again
            // (`unwatch_all` already forgot its watches, so they are not found here)
            if let Some(path) = self.by_wd.remove(&event.wd) {
                if self.by_path.get(&path) == Some(&event.wd) {
                    self.by_path.remove(&path);
                }
                push(path);
            }
        } else if let Some(dir) = self.by_wd.get(&event.wd) {
            push(
                event
                    .name
                    .map_or_else(|| dir.clone(), |name| dir.join(name)),
            );
        }
    }
}

/// Locks the shared watch table; a panic elsewhere leaves it usable.
fn lock(watched: &Mutex<Watched>) -> MutexGuard<'_, Watched> {
    watched.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Inotify watches on directories, read by a thread that sends [`Msg::Changed`].
///
/// The inotify file descriptor is shared: the reader thread owns the `Inotify` and blocks in
/// `read_events_blocking`, while this struct keeps a `Watches` handle (inotify 0.11 hands out
/// clones holding an `Arc` of the same descriptor) to add and remove watches from the pane's
/// thread. `inotify_add_watch` and `inotify_rm_watch` are safe to call while another thread
/// blocks in `read` on the same descriptor; `read_events_blocking` only toggles `O_NONBLOCK`,
/// which those calls ignore.
#[derive(Debug)]
pub struct Watcher {
    watches: Watches,
    watched: Arc<Mutex<Watched>>,
}

impl Watcher {
    /// Creates the inotify instance and starts the thread that reads it.
    ///
    /// The thread lives as long as the process, or until the channel's receiver is dropped
    /// and one more event arrives.
    ///
    /// # Errors
    ///
    /// Fails when inotify cannot be initialised or the thread cannot be spawned.
    fn start(tx: Sender<Feed>) -> io::Result<Self> {
        let inotify = Inotify::init()?;
        let watches = inotify.watches();
        let watched = Arc::new(Mutex::new(Watched::default()));
        let shared = Arc::clone(&watched);
        thread::Builder::new()
            .name("cst-inotify".into())
            .spawn(move || read_changes(inotify, &shared, &tx))?;
        Ok(Watcher { watches, watched })
    }

    /// Watches a directory; watching one already watched does nothing.
    ///
    /// # Errors
    ///
    /// Fails when the directory cannot be watched, for example when it does not exist.
    pub fn watch(&mut self, path: &Path) -> io::Result<()> {
        let mut watched = lock(&self.watched);
        if watched.by_path.contains_key(path) {
            return Ok(());
        }
        let wd = self.watches.add(path, WATCH_MASK)?;
        watched.by_wd.insert(wd.clone(), path.to_path_buf());
        watched.by_path.insert(path.to_path_buf(), wd);
        Ok(())
    }

    /// Removes every watch; events already queued for them are dropped.
    pub fn unwatch_all(&mut self) {
        let mut watched = lock(&self.watched);
        for (_, wd) in watched.by_path.drain() {
            // Fails only when the kernel already dropped the watch
            let _ = self.watches.remove(wd);
        }
        watched.by_wd.clear();
    }
}

/// Reads inotify events until the descriptor fails or nobody listens any more.
fn read_changes(mut inotify: Inotify, watched: &Mutex<Watched>, tx: &Sender<Feed>) {
    let mut buffer = [0; EVENT_BUFFER];
    loop {
        let events = match inotify.read_events_blocking(&mut buffer) {
            Ok(events) => events,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        let mut changed = Vec::new();
        {
            let mut watched = lock(watched);
            for event in events {
                watched.changes(event, &mut changed);
            }
        }
        for path in changed {
            if tx.send(Feed::Msg(Msg::Changed(path))).is_err() {
                return;
            }
        }
    }
}

/// Reads terminal events with `read` until the terminal fails or nobody listens any more.
///
/// A terminal that fails is gone: the loop learns it from [`Feed::InputEnded`].
fn read_input(read: &mut dyn FnMut() -> io::Result<Event>, tx: &Sender<Feed>) {
    loop {
        let msg = match read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => Msg::Key(key),
            Ok(Event::Mouse(mouse)) => Msg::Mouse(mouse),
            Ok(Event::Resize(..)) => Msg::Resize,
            Ok(_) => continue,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                // Nobody listening either: nothing left to tell
                let _ = tx.send(Feed::InputEnded);
                return;
            }
        };
        if tx.send(Feed::Msg(msg)).is_err() {
            return;
        }
    }
}

/// The pane's event sources merged into one channel.
#[derive(Debug)]
pub struct Runtime {
    rx: Receiver<Feed>,
    watcher: Watcher,
    /// Whether the terminal input ended: every wait from then on is [`Wake::Closed`].
    ended: Cell<bool>,
}

impl Runtime {
    /// Starts the terminal input thread and the inotify thread.
    ///
    /// # Errors
    ///
    /// Fails when inotify cannot be initialised or a thread cannot be spawned.
    pub fn start() -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::start(tx.clone())?;
        thread::Builder::new()
            .name("cst-input".into())
            .spawn(move || read_input(&mut event::read, &tx))?;
        Ok(Runtime::with(rx, watcher))
    }

    fn with(rx: Receiver<Feed>, watcher: Watcher) -> Self {
        Runtime {
            rx,
            watcher,
            ended: Cell::new(false),
        }
    }

    /// A runtime with the inotify thread only, for tests that have no terminal.
    #[cfg(test)]
    pub(crate) fn headless() -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        Ok(Runtime::with(rx, Watcher::start(tx)?))
    }

    /// Watches a directory: its entries' changes arrive as [`Msg::Changed`].
    ///
    /// Watch directories, not files: a file written atomically is a new inode renamed into
    /// its directory, which a watch on the old file never sees.
    ///
    /// # Errors
    ///
    /// Fails when the directory cannot be watched, for example when it does not exist.
    pub fn watch(&mut self, path: &Path) -> io::Result<()> {
        self.watcher.watch(path)
    }

    /// Removes every watch, as a pane does when it is hidden.
    pub fn unwatch_all(&mut self) {
        self.watcher.unwatch_all();
    }

    /// Waits for the next message, until `deadline` (Unix time) or forever when it is `None`.
    ///
    /// A deadline already past only takes a message that is waiting.
    ///
    /// Returns `None` when the deadline passes first, or when the terminal input ended.
    pub fn next(&self, deadline: Option<f64>) -> Option<Msg> {
        match self.wait(deadline) {
            Wake::Msg(msg) => Some(msg),
            Wake::Timeout | Wake::Closed => None,
        }
    }

    /// Like [`Runtime::next`], telling a deadline that passed from a terminal that is gone.
    pub fn wait(&self, deadline: Option<f64>) -> Wake {
        if self.ended.get() {
            return Wake::Closed;
        }
        let received = match deadline {
            None => self.rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(at) => {
                let wait = (at - unix_now()).max(0.0);
                // Too far away to represent: as good as forever
                let wait = Duration::try_from_secs_f64(wait).unwrap_or(Duration::MAX);
                self.rx.recv_timeout(wait)
            }
        };
        match received {
            Ok(Feed::Msg(msg)) => Wake::Msg(msg),
            Err(RecvTimeoutError::Timeout) => Wake::Timeout,
            Ok(Feed::InputEnded) | Err(RecvTimeoutError::Disconnected) => {
                self.ended.set(true);
                Wake::Closed
            }
        }
    }
}

/// What a wait on the pane's event sources ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wake {
    /// A message arrived.
    Msg(Msg),
    /// The deadline passed first.
    Timeout,
    /// The terminal is gone, or both threads stopped: the pane's loop must end.
    Closed,
}

/// Spinner frames per second while a session works.
const FRAMES_PER_SECOND: f64 = 8.0;

/// How often a clock label can change, in seconds.
const LABEL_PERIOD: f64 = 60.0;

/// The shortest gap between two redraws, in seconds: about 10 redraws a second at most.
const REDRAW_GAP: f64 = 0.1;

/// What decides when a pane must wake up with no event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Schedule {
    /// Whether the pane is on screen.
    pub visible: bool,
    /// Whether a spinner runs.
    pub animating: bool,
    /// Start times (Unix) of labels that change every minute after them.
    pub labels: Vec<f64>,
    /// One-shot deadlines (Unix), such as the end of a session's quiet grace.
    pub quiet: Vec<f64>,
    /// When the pane last drew (Unix).
    pub last_draw: f64,
    /// Whether something changed since the last draw.
    pub dirty: bool,
}

impl Schedule {
    /// The next time (Unix) the pane must wake up, or `None` to sleep until an event.
    ///
    /// A hidden pane never wakes. A visible one wakes at the earliest of: the next 1/8 s
    /// boundary while animating; the next whole minute after each label's start; each quiet
    /// deadline still ahead; and 100 ms after the last draw when dirty (which may already be
    /// past, meaning at once).
    pub fn next_wake(&self, now: f64) -> Option<f64> {
        if !self.visible {
            return None;
        }
        let frame = self
            .animating
            .then(|| next_boundary(0.0, FRAMES_PER_SECOND.recip(), now));
        let labels = self
            .labels
            .iter()
            .map(|&since| next_boundary(since, LABEL_PERIOD, now));
        let quiet = self.quiet.iter().copied().filter(|&at| at > now);
        let redraw = self.dirty.then_some(self.last_draw + REDRAW_GAP);
        frame
            .into_iter()
            .chain(labels)
            .chain(quiet)
            .chain(redraw)
            .reduce(f64::min)
    }
}

/// The first `origin + period * k` (any integer `k`) strictly after `now`.
fn next_boundary(origin: f64, period: f64, now: f64) -> f64 {
    let at = origin + period * ((now - origin) / period).floor();
    if at > now {
        at
    } else {
        at + period
    }
}

/// Turns on (`true`) or off mouse reports for clicks and the wheel, in SGR encoding.
///
/// crossterm's `EnableMouseCapture` also asks for every pointer motion (mode 1003), which
/// would wake a visible pane each time the mouse crosses it; the panes only need clicks.
#[derive(Debug, Clone, Copy)]
struct MouseClicks(bool);

impl Command for MouseClicks {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        if self.0 {
            f.write_str("\x1b[?1000h\x1b[?1006h")
        } else {
            f.write_str("\x1b[?1006l\x1b[?1000l")
        }
    }
}

/// The terminal set up for a pane, restored when dropped or when the process panics.
///
/// Raw mode, alternate screen, cursor hidden, mouse clicks reported.
///
/// The alternate screen is on because tmux keeps no history for it. On the main screen,
/// ratatui clears the screen on every resize and tmux (`scroll-on-clear`, on by default)
/// scrolls the cleared frame into the pane's history, which then fills with stale frames.
pub struct Screen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Screen {
    /// Sets the terminal up and installs the panic hook that restores it.
    ///
    /// # Errors
    ///
    /// Fails when stdout is not a terminal or the terminal rejects a setting.
    pub fn enter() -> io::Result<Self> {
        install_panic_hook();
        terminal::enable_raw_mode()?;
        let set_up = (|| {
            execute!(
                io::stdout(),
                terminal::EnterAlternateScreen,
                cursor::Hide,
                MouseClicks(true)
            )?;
            let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
            Ok(Screen { terminal })
        })();
        if set_up.is_err() {
            restore();
        }
        set_up
    }

    /// Draws one frame; only the cells that changed since the last one are written.
    ///
    /// # Errors
    ///
    /// Fails when writing to the terminal fails.
    pub fn draw<F: FnOnce(&mut Frame)>(&mut self, render: F) -> io::Result<()> {
        self.terminal.draw(render).map(drop)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}

/// Gives the terminal back as it was: mouse reports off, cursor shown, main screen, raw mode
/// off.
fn restore() {
    // Best effort: the process is leaving and has nowhere to report a failure
    let _ = execute!(
        io::stdout(),
        MouseClicks(false),
        cursor::Show,
        terminal::LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}

/// Restores the terminal before the default panic message prints, once per process.
fn install_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::Command;
    use tempfile::TempDir;

    use crate::core::paths::write_atomic;
    use crate::ui::runtime::{
        read_input, signal, unix_now, MouseClicks, Msg, Runtime, Schedule, Signal, Wake, Watcher,
    };

    fn visible() -> Schedule {
        Schedule {
            visible: true,
            ..Schedule::default()
        }
    }

    #[test]
    fn hidden_never_wakes() {
        let schedule = Schedule {
            visible: false,
            animating: true,
            labels: vec![10.0],
            quiet: vec![1003.0],
            last_draw: 999.0,
            dirty: true,
        };
        assert_eq!(schedule.next_wake(1000.0), None);
    }

    #[test]
    fn visible_with_nothing_pending_never_wakes() {
        assert_eq!(visible().next_wake(1000.0), None);
    }

    #[test]
    fn animating_wakes_at_next_eighth() {
        let schedule = Schedule {
            animating: true,
            ..visible()
        };
        assert_eq!(schedule.next_wake(1000.3), Some(1000.375));
        assert_eq!(schedule.next_wake(1000.25), Some(1000.375));
    }

    #[test]
    fn label_wakes_at_next_minute_boundary() {
        let schedule = Schedule {
            labels: vec![1000.5, 900.0],
            ..visible()
        };
        // 1000.5 + 60 = 1060.5; 900 + 120 = 1020 comes first
        assert_eq!(schedule.next_wake(1010.0), Some(1020.0));
        // Exactly on a boundary, the next one
        assert_eq!(schedule.next_wake(1020.0), Some(1060.5));
    }

    #[test]
    fn label_in_the_future_wakes_at_its_start() {
        let schedule = Schedule {
            labels: vec![1030.0],
            ..visible()
        };
        assert_eq!(schedule.next_wake(1000.0), Some(1030.0));
    }

    #[test]
    fn quiet_deadline_wins_when_earlier() {
        let schedule = Schedule {
            labels: vec![990.0],
            quiet: vec![1003.0],
            ..visible()
        };
        assert_eq!(schedule.next_wake(1000.0), Some(1003.0));
    }

    #[test]
    fn past_quiet_deadline_is_ignored() {
        let schedule = Schedule {
            quiet: vec![999.0, 1000.0],
            ..visible()
        };
        assert_eq!(schedule.next_wake(1000.0), None);
    }

    #[test]
    fn dirty_coalesces_to_100ms() {
        let schedule = Schedule {
            dirty: true,
            last_draw: 1000.0,
            animating: true,
            ..visible()
        };
        assert_eq!(schedule.next_wake(1000.0), Some(1000.1));
        // Long after the last draw, the redraw is due at once
        let late = Schedule {
            dirty: true,
            last_draw: 990.0,
            ..visible()
        };
        assert_eq!(late.next_wake(1000.0), Some(990.1));
    }

    #[test]
    fn f_keys_map_to_signals() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(signal(&key(KeyCode::F(12))), Some(Signal::Shown));
        assert_eq!(signal(&key(KeyCode::F(11))), Some(Signal::Hidden));
        assert_eq!(signal(&key(KeyCode::F(10))), Some(Signal::Layout));
        assert_eq!(signal(&key(KeyCode::F(9))), None);
        assert_eq!(signal(&key(KeyCode::Char('q'))), None);
    }

    #[test]
    fn mouse_clicks_turn_on_buttons_and_sgr_only() {
        let mut ansi = String::new();
        MouseClicks(true).write_ansi(&mut ansi).unwrap();
        assert_eq!(ansi, "\x1b[?1000h\x1b[?1006h");
    }

    #[test]
    fn mouse_clicks_off_turns_both_modes_off() {
        let mut ansi = String::new();
        MouseClicks(false).write_ansi(&mut ansi).unwrap();
        assert_eq!(ansi, "\x1b[?1006l\x1b[?1000l");
    }

    #[test]
    fn wait_tells_a_timeout_from_a_closed_channel() {
        let runtime = Runtime::headless().unwrap();
        assert_eq!(runtime.wait(Some(unix_now() - 5.0)), Wake::Timeout);
        // Nothing feeds this channel any more: a wait with a far deadline ends at once
        let (tx, rx) = mpsc::channel();
        drop(tx);
        let closed = Runtime::with(rx, Watcher::start(mpsc::channel().0).unwrap());
        let started = Instant::now();
        assert_eq!(closed.wait(Some(unix_now() + 5.0)), Wake::Closed);
        assert_eq!(closed.wait(None), Wake::Closed);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn the_end_of_input_closes_the_wait_though_inotify_runs() {
        let (tx, rx) = mpsc::channel();
        let runtime = Runtime::with(rx, Watcher::start(tx.clone()).unwrap());
        let mut reads = vec![
            Err(io::Error::other("terminal gone")),
            Ok(Event::Key(KeyEvent::new(
                KeyCode::Char('a'),
                KeyModifiers::NONE,
            ))),
        ];
        read_input(&mut || reads.pop().unwrap(), &tx);
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(runtime.wait(None), Wake::Msg(Msg::Key(key)));
        let started = Instant::now();
        assert_eq!(runtime.wait(Some(unix_now() + 5.0)), Wake::Closed);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn past_deadline_returns_at_once() {
        let runtime = Runtime::headless().unwrap();
        assert_eq!(runtime.next(Some(unix_now() - 5.0)), None);
    }

    #[test]
    fn watcher_reports_a_file_written_atomically() {
        let dir = TempDir::new().unwrap();
        let watched = dir.path().join("records");
        fs::create_dir(&watched).unwrap();
        let mut runtime = Runtime::headless().unwrap();
        runtime.watch(&watched).unwrap();
        runtime.watch(&watched).unwrap();
        let target = watched.join("s1.json");
        write_atomic(&target, b"{}", None).unwrap();

        let deadline = unix_now() + 1.0;
        let mut seen = false;
        while let Some(msg) = runtime.next(Some(deadline)) {
            if msg == Msg::Changed(target.clone()) {
                seen = true;
                break;
            }
        }
        assert!(seen, "no Changed({}) within 1 s", target.display());
    }

    #[test]
    fn unwatched_directory_reports_nothing() {
        let dir = TempDir::new().unwrap();
        let mut runtime = Runtime::headless().unwrap();
        runtime.watch(dir.path()).unwrap();
        runtime.unwatch_all();
        fs::write(dir.path().join("a"), b"x").unwrap();
        assert_eq!(runtime.next(Some(unix_now() + 0.2)), None);
    }

    #[test]
    fn removed_directory_reports_itself_and_can_be_watched_again() {
        let dir = TempDir::new().unwrap();
        let watched = dir.path().join("records");
        fs::create_dir(&watched).unwrap();
        let mut runtime = Runtime::headless().unwrap();
        runtime.watch(&watched).unwrap();
        fs::remove_dir(&watched).unwrap();

        let deadline = unix_now() + 1.0;
        let mut seen = false;
        while let Some(msg) = runtime.next(Some(deadline)) {
            if msg == Msg::Changed(watched.clone()) {
                seen = true;
                break;
            }
        }
        assert!(seen, "no Changed({}) within 1 s", watched.display());

        fs::create_dir(&watched).unwrap();
        runtime.watch(&watched).unwrap();
        let target = watched.join("a");
        fs::write(&target, b"x").unwrap();
        assert_eq!(
            runtime.next(Some(unix_now() + 1.0)),
            Some(Msg::Changed(target))
        );
    }

    #[test]
    fn watch_after_unwatch_all_reports_again() {
        let dir = TempDir::new().unwrap();
        let mut runtime = Runtime::headless().unwrap();
        runtime.watch(dir.path()).unwrap();
        runtime.unwatch_all();
        runtime.watch(dir.path()).unwrap();
        let target = dir.path().join("a");
        fs::write(&target, b"x").unwrap();
        assert_eq!(
            runtime.next(Some(unix_now() + 1.0)),
            Some(Msg::Changed(target))
        );
    }
}
