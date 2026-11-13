//! What the sidebar, the landing page and the Alt+n panel share: the loop they run on, how they
//! learn whether they are on screen and how they log failures; the sidebar and the landing
//! page also share a [`Shell`]. What they watch is in [`crate::ui::watch`].

use std::fmt::Display;
use std::io;
use std::thread;
use std::time::Duration;

use anyhow::bail;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::claude::history::HistoryReader;
use crate::claude::statusline::load_limits;
use crate::core::paths;
use crate::core::state::unix_now;
use crate::term::tmux::{self, Runner};
use crate::ui::picker;
use crate::ui::runtime::{signal, Msg, Runtime, Schedule, Screen, Signal, Wake};
use crate::ui::sidebar::Stats;
use crate::ui::usage::Reader;
use crate::ui::watch::{Stale, Watches};

/// How many times tmux is asked for the pane's window before giving up.
const WINDOW_TRIES: u32 = 5;
/// The pause between two of those asks.
pub const WINDOW_PAUSE: Duration = Duration::from_millis(100);

/// What a pane does after taking a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Waits for more; a redraw comes when the schedule says.
    Wait,
    /// Reads what is stale and draws at once.
    Draw,
    /// Ends the loop.
    Quit,
}

/// A pane run by [`run`].
pub trait App {
    /// When the pane must wake up with no event.
    fn schedule(&self) -> &Schedule;
    /// Takes one message.
    fn handle(&mut self, msg: Msg, now: f64, runtime: &mut Runtime) -> Step;
    /// Reads what is stale, then draws.
    fn update(&mut self, now: f64, runtime: &mut Runtime, screen: &mut Screen);
}

/// Runs a pane until it quits or its terminal goes away.
///
/// A burst of events draws at once, then at most every 100 ms: events only mark what is
/// stale, and the reading happens in [`App::update`].
pub fn run(app: &mut impl App, runtime: &mut Runtime, screen: &mut Screen) {
    loop {
        let deadline = app.schedule().next_wake(unix_now());
        let wake = runtime.wait(deadline);
        let now = unix_now();
        let at_once = match wake {
            Wake::Msg(msg) => match app.handle(msg, now, runtime) {
                Step::Quit => return,
                Step::Draw => true,
                Step::Wait => false,
            },
            Wake::Timeout => true,
            // The terminal is gone: nobody sees the pane any more
            Wake::Closed => return,
        };
        if at_once || app.schedule().next_wake(now).is_some_and(|at| at <= now) {
            app.update(now, runtime, screen);
        }
    }
}

/// What a message tells about whether the pane is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sight {
    /// It is on screen now.
    Shown,
    /// It left the screen.
    Hidden,
    /// Nothing new.
    Same,
}

/// What `msg` tells a pane that believes itself `visible` about being on screen.
///
/// tmux says so with F12 and F11. A pane believed hidden that gets a key the user typed, a
/// click, a resize or a layout change is on screen all the same: tmux's word was lost.
///
/// A resize is such a proof because tmux (3.1 on; tmux.conf requires 3.3) resizes a window
/// only when it is current or becomes current, never while it is out of sight.
pub fn sight(msg: &Msg, visible: bool) -> Sight {
    match msg {
        Msg::Key(key) => match signal(key) {
            Some(Signal::Shown) => Sight::Shown,
            Some(Signal::Hidden) => Sight::Hidden,
            Some(Signal::Layout) | None if !visible => Sight::Shown,
            Some(Signal::Layout) | None => Sight::Same,
        },
        Msg::Mouse(_) | Msg::Resize if !visible => Sight::Shown,
        Msg::Mouse(_) | Msg::Resize | Msg::Changed(_) => Sight::Same,
    }
}

/// What the sidebar and the landing page share: when to wake up, what they watch and must read
/// again, and the statistics they add up from the history and the journal.
pub struct Shell {
    /// When the pane must wake up, and whether it is on screen.
    pub schedule: Schedule,
    /// The directories watched now.
    pub watches: Watches,
    /// What to read again before the next draw.
    pub stale: Stale,
    pub history: HistoryReader,
    pub usage: Reader,
    pub stats: Stats,
    /// The minute the statistics were added up in; they change with the time too.
    stats_minute: Option<i64>,
    pub load_error: LogOnce,
    draw_error: LogOnce,
}

impl Shell {
    /// A shell for the pane named `who` in its error log; `stored` keeps the history's index on
    /// disk for the next process.
    pub fn new(who: &'static str, stored: bool) -> Self {
        Shell {
            schedule: Schedule::default(),
            watches: Watches::default(),
            stale: Stale::NONE,
            history: HistoryReader::new(stored),
            usage: Reader::new(),
            stats: Stats::default(),
            stats_minute: None,
            load_error: LogOnce::new(who),
            draw_error: LogOnce::new(who),
        }
    }

    /// Before an update: marks the quotas stale once one resets, and what changed while a
    /// watch was missing. Returns whether there is anything to do: nothing while off screen.
    pub fn prepare(&mut self, now: f64, runtime: &mut Runtime) -> bool {
        if !self.schedule.visible {
            return false;
        }
        if self.stats.limits.values().any(|q| q.resets_at <= now) {
            self.stale.limits = true;
        }
        if self.stale.any() {
            let missed = self.watches.roots(runtime, &mut self.load_error);
            self.stale.add(missed);
        }
        true
    }

    /// Reads again the history, the statistics and the quotas, where `stale` says so.
    ///
    /// The statistics are added up again when the history or the journal changed, and every
    /// minute: today's figures grow with the time, and the week moves on at midnight. `full`
    /// adds the week's totals and hours, which only the landing page shows. Returns
    /// whether they were added up again.
    pub fn read_stats(&mut self, stale: &Stale, now: f64, full: bool) -> bool {
        if stale.history {
            self.history.load();
        }
        let minute = (now / 60.0).floor() as i64;
        let due = stale.history || stale.journal || self.stats_minute != Some(minute);
        if due {
            let generation = self.history.generation();
            let totals = self.usage.totals(self.history.sessions(), generation, now);
            self.stats.today = totals.today;
            self.stats.week = totals.days.clone();
            if full {
                self.stats.totals = Some(totals.clone());
            }
            self.stats.day = Some(self.usage.day(now));
            if full {
                self.stats.hours = self.usage.hours(now).to_vec();
            }
            self.stats_minute = Some(minute);
        }
        if stale.limits {
            self.stats.limits = load_limits(now);
        }
        due
    }

    /// After a draw at `now`: logs a failed one, which keeps the last screen.
    pub fn drawn(&mut self, result: io::Result<()>, now: f64) {
        if let Err(e) = result {
            self.draw_error.log(&format_args!("cannot draw: {e}"));
        }
        self.schedule.last_draw = now;
        self.schedule.dirty = false;
    }
}

/// A pane tmux shows and hides: the sidebar and the landing page.
pub trait Visible {
    /// The pane's [`Shell`].
    fn shell(&self) -> &Shell;
    /// The pane's [`Shell`], to change.
    fn shell_mut(&mut self) -> &mut Shell;

    /// Whether the pane believes itself on screen.
    fn visible(&self) -> bool {
        self.shell().schedule.visible
    }

    /// Comes on screen: everything is read again before the next draw.
    fn show(&mut self) {
        let shell = self.shell_mut();
        shell.schedule.visible = true;
        shell.stale = Stale::ALL;
    }

    /// Leaves the screen: no watches, no timer, nothing read until shown again.
    fn hide(&mut self, runtime: &mut Runtime) {
        let shell = self.shell_mut();
        shell.schedule.visible = false;
        shell.schedule.dirty = false;
        shell.watches.clear(runtime);
    }
}

/// Takes a message for a pane tmux shows and hides; `take` handles it once the pane is known
/// to be on screen.
///
/// The pane is shown or hidden as the message tells ([`sight`]). A hidden pane does no work:
/// the message that hid it, and any that comes while hidden (such as changes queued before
/// its watches went), is dropped. A pane just shown draws at once, whatever `take` returns.
pub fn gated<A: Visible>(
    app: &mut A,
    msg: Msg,
    runtime: &mut Runtime,
    take: impl FnOnce(&mut A, Msg, &mut Runtime) -> Step,
) -> Step {
    match sight(&msg, app.visible()) {
        Sight::Shown => {
            app.show();
            match take(app, msg, runtime) {
                Step::Quit => Step::Quit,
                Step::Wait | Step::Draw => Step::Draw,
            }
        }
        Sight::Hidden => {
            app.hide(runtime);
            Step::Wait
        }
        Sight::Same if app.visible() => take(app, msg, runtime),
        Sight::Same => Step::Wait,
    }
}

/// The tmux window a pane is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnWindow {
    /// The window's id, as `@3`.
    pub id: String,
    /// Whether it is its session's current window; a pane started in another one starts out
    /// of sight.
    pub current: bool,
}

/// The tmux window `pane` is in, asking tmux up to five times, `pause` apart.
///
/// tmux can be slow to answer a pane it has just started, as on a busy server.
///
/// # Errors
///
/// Fails when `pane` is `None` or empty (not in tmux) or tmux never answers.
pub fn own_window(
    tmux: &dyn Runner,
    pane: Option<&str>,
    pause: Duration,
) -> anyhow::Result<OwnWindow> {
    let Some(pane) = pane.filter(|p| !p.is_empty()) else {
        bail!("not in a tmux pane: TMUX_PANE is not set");
    };
    let format = "#{window_id}\t#{window_active}";
    for attempt in 0..WINDOW_TRIES {
        if attempt > 0 {
            thread::sleep(pause);
        }
        let out = tmux.run(&["display-message", "-p", "-t", pane, format]);
        if let Some((id, active)) = out.split_once('\t').filter(|(id, _)| !id.is_empty()) {
            return Ok(OwnWindow {
                id: id.to_owned(),
                current: active == "1",
            });
        }
    }
    bail!("cannot find the window of pane {pane}")
}

/// The socket of the tmux server this process runs in; records name their server by it.
pub fn own_socket() -> String {
    std::env::var("TMUX")
        .ok()
        .and_then(|t| tmux::socket_of(&t).map(str::to_owned))
        .unwrap_or_default()
}

/// Whether a key ends a panel: Ctrl+C or Ctrl+Q.
pub fn quits(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'q'))
}

/// The key the folder list takes for a key press, if any: arrows, Enter, Backspace, Ctrl+n and
/// Ctrl+o for the entries, and a character typed with no modifier but Shift.
pub fn picker_key(key: &KeyEvent) -> Option<picker::Key> {
    let ctrl = key.modifiers == KeyModifiers::CONTROL;
    Some(match key.code {
        KeyCode::Char('n') if ctrl => picker::Key::Entry(picker::Root::New),
        KeyCode::Char('o') if ctrl => picker::Key::Entry(picker::Root::Open),
        KeyCode::Up => picker::Key::Up,
        KeyCode::Down => picker::Key::Down,
        KeyCode::Left => picker::Key::Left,
        KeyCode::Right => picker::Key::Right,
        KeyCode::Enter => picker::Key::Enter,
        KeyCode::Backspace => picker::Key::Backspace,
        KeyCode::Char(c) if (key.modifiers - KeyModifiers::SHIFT).is_empty() => {
            picker::Key::Char(c)
        }
        _ => return None,
    })
}

/// Runs `new-window` for a row of the folder list: resume its session, or start one.
///
/// A create row's folder, and any missing parent, is created first, as is the folder of a
/// session found that was deleted; when that fails, the failure goes to the error log and
/// nothing starts.
pub fn open(tmux: &dyn Runner, row: &picker::Row, folders: &[picker::Folder]) {
    open_with(tmux, row, folders, &paths::process_env);
}

/// [`open`], logging to the state directory `env` names.
fn open_with(
    tmux: &dyn Runner,
    row: &picker::Row,
    folders: &[picker::Folder],
    env: paths::Lookup<'_>,
) {
    if matches!(row.kind, picker::RowKind::Create | picker::RowKind::Hit) {
        if let Err(e) = std::fs::create_dir_all(&row.path) {
            paths::log_error_with(env, &format_args!("cannot create {}: {e}", row.path));
            return;
        }
    }
    let command = picker::command(row, folders);
    let mut args = vec!["new-window"];
    args.extend(command.iter().map(String::as_str));
    tmux.run(&args);
}

/// Logs a failure unless it repeats the last one, which would fill the log on every redraw.
#[derive(Debug)]
pub struct LogOnce {
    /// The pane, named before each line.
    pane: &'static str,
    last: Option<String>,
}

impl LogOnce {
    /// Logs for the pane called `pane`.
    pub fn new(pane: &'static str) -> Self {
        LogOnce { pane, last: None }
    }

    /// Returns the line to log, or `None` when it is the same as the last failure.
    fn check(&mut self, what: &dyn Display) -> Option<String> {
        let line = what.to_string();
        if self.last.as_ref() == Some(&line) {
            return None;
        }
        self.last = Some(line.clone());
        Some(line)
    }

    /// Writes a failure to the error log, unless it repeats the last one.
    pub fn log(&mut self, what: &dyn Display) {
        if let Some(line) = self.check(what) {
            paths::log_error(&format_args!("{}: {line}", self.pane));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::time::Duration;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    use crate::core::paths::ERROR_LOG;
    use crate::term::tmux::fake::FakeTmux;
    use crate::ui::pane::{open, open_with, own_window, picker_key, quits, sight, LogOnce, Sight};
    use crate::ui::picker::{Key, Root, Row, RowKind};
    use crate::ui::runtime::Msg;

    /// Answers each call with the next of `answers`, then with nothing.
    fn answering(answers: &[&'static str]) -> FakeTmux {
        let left = RefCell::new(answers.iter().rev().copied().collect::<Vec<_>>());
        FakeTmux::new(move |_| left.borrow_mut().pop().unwrap_or_default().to_owned())
    }

    /// How many times `tmux` was asked for pane `%7`'s window, failing on any other call.
    fn asks(tmux: &FakeTmux) -> usize {
        let ask = [
            "display-message",
            "-p",
            "-t",
            "%7",
            "#{window_id}\t#{window_active}",
        ];
        let calls = tmux.calls();
        assert!(calls.iter().all(|c| c == &ask), "{calls:?}");
        calls.len()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn click() -> Msg {
        Msg::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 4,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn own_window_asks_again_until_tmux_answers() {
        let tmux = answering(&["", "", "@3\t1"]);
        let got = own_window(&tmux, Some("%7"), Duration::ZERO).unwrap();
        assert_eq!((got.id.as_str(), got.current, asks(&tmux)), ("@3", true, 3));
    }

    #[test]
    fn own_window_tells_a_window_out_of_sight() {
        let tmux = answering(&["@3\t0"]);
        let got = own_window(&tmux, Some("%7"), Duration::ZERO).unwrap();
        assert_eq!((got.id.as_str(), got.current), ("@3", false));
    }

    #[test]
    fn own_window_gives_up_after_five_asks() {
        let tmux = answering(&[]);
        let err = own_window(&tmux, Some("%7"), Duration::ZERO).unwrap_err();
        assert_eq!(asks(&tmux), 5);
        assert!(err.to_string().contains("%7"), "{err}");
    }

    #[test]
    fn own_window_needs_a_pane() {
        let tmux = answering(&["@3"]);
        assert!(own_window(&tmux, None, Duration::ZERO).is_err());
        assert!(own_window(&tmux, Some(""), Duration::ZERO).is_err());
        assert_eq!(asks(&tmux), 0);
    }

    #[test]
    fn tmux_signals_show_and_hide() {
        for visible in [true, false] {
            let f = |n| Msg::Key(key(KeyCode::F(n)));
            assert_eq!(sight(&f(12), visible), Sight::Shown);
            assert_eq!(sight(&f(11), visible), Sight::Hidden);
        }
    }

    #[test]
    fn what_only_a_pane_on_screen_gets_shows_a_hidden_one() {
        let shows = [
            click(),
            Msg::Resize,
            Msg::Key(key(KeyCode::F(10))),
            Msg::Key(key(KeyCode::Char('a'))),
            Msg::Key(key(KeyCode::Up)),
        ];
        for msg in &shows {
            assert_eq!(sight(msg, false), Sight::Shown, "{msg:?}");
            assert_eq!(sight(msg, true), Sight::Same, "{msg:?}");
        }
    }

    #[test]
    fn a_change_on_disk_does_not_show_a_hidden_pane() {
        // Queued before the watches went
        let changed = Msg::Changed(PathBuf::from("/run/cst/s1.json"));
        assert_eq!(sight(&changed, false), Sight::Same);
    }

    #[test]
    fn ctrl_c_and_ctrl_q_quit() {
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert!(quits(&ctrl('c')) && quits(&ctrl('q')));
        assert!(!quits(&key(KeyCode::Char('q'))) && !quits(&ctrl('x')));
    }

    #[test]
    fn keys_map_to_the_folder_list() {
        assert_eq!(picker_key(&key(KeyCode::Up)), Some(Key::Up));
        assert_eq!(picker_key(&key(KeyCode::Down)), Some(Key::Down));
        assert_eq!(picker_key(&key(KeyCode::Left)), Some(Key::Left));
        assert_eq!(picker_key(&key(KeyCode::Right)), Some(Key::Right));
        assert_eq!(picker_key(&key(KeyCode::Enter)), Some(Key::Enter));
        assert_eq!(picker_key(&key(KeyCode::Backspace)), Some(Key::Backspace));
        assert_eq!(picker_key(&key(KeyCode::Char('/'))), Some(Key::Char('/')));
        let shifted = KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT);
        assert_eq!(picker_key(&shifted), Some(Key::Char('W')));
        let ctrl = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        let alt = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT);
        assert_eq!((picker_key(&ctrl), picker_key(&alt)), (None, None));
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(picker_key(&ctrl('n')), Some(Key::Entry(Root::New)));
        assert_eq!(picker_key(&ctrl('o')), Some(Key::Entry(Root::Open)));
        assert_eq!(picker_key(&key(KeyCode::Tab)), None);
        assert_eq!(picker_key(&key(KeyCode::Esc)), None);
    }

    #[test]
    fn a_failure_is_logged_once_until_another_one() {
        let mut log = LogOnce::new("sidebar");
        assert_eq!(log.check(&"x: gone"), Some("x: gone".to_owned()));
        assert_eq!(log.check(&"x: gone"), None);
        assert_eq!(log.check(&"y: bad"), Some("y: bad".to_owned()));
        assert_eq!(log.check(&"x: gone"), Some("x: gone".to_owned()));
    }

    /// A row that creates the folder `path`, then starts a session in it.
    fn create_row(path: &std::path::Path) -> Row {
        Row {
            kind: RowKind::Create,
            path: path.to_string_lossy().into_owned(),
            ..Row::default()
        }
    }

    #[test]
    fn open_creates_the_folder_then_starts_there() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("Documents/Meu-Projeto");
        let tmux = FakeTmux::silent();
        open(&tmux, &create_row(&path), &[]);
        assert!(path.is_dir());
        let want = ["new-window", "-c", &path.to_string_lossy(), "claude"];
        assert_eq!(tmux.calls(), [want.map(str::to_owned).to_vec()]);
    }

    #[test]
    fn open_logs_and_starts_nothing_when_create_fails() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("x");
        std::fs::write(&path, "").unwrap();
        let state = t.path().join("state");
        let env = |key: &str| (key == "CST_STATE_DIR").then(|| state.clone().into_os_string());
        let tmux = FakeTmux::silent();
        open_with(&tmux, &create_row(&path), &[], &env);
        assert!(tmux.calls().is_empty());
        let log = std::fs::read_to_string(state.join(ERROR_LOG)).unwrap();
        assert!(log.contains(&*path.to_string_lossy()), "{log}");
    }

    #[test]
    fn a_hit_in_a_deleted_folder_creates_it_and_resumes() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("gone/Projeto");
        let row = Row {
            kind: RowKind::Hit,
            id: "id".into(),
            path: path.to_string_lossy().into_owned(),
            ..Row::default()
        };
        let tmux = FakeTmux::silent();
        open(&tmux, &row, &[]);
        assert!(path.is_dir());
        let want = [
            "new-window",
            "-c",
            &path.to_string_lossy(),
            "claude --resume id",
        ];
        assert_eq!(tmux.calls(), [want.map(str::to_owned).to_vec()]);
    }

    #[test]
    fn a_hit_logs_and_starts_nothing_when_its_folder_cannot_be_made() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("x");
        std::fs::write(&path, "").unwrap();
        let state = t.path().join("state");
        let env = |key: &str| (key == "CST_STATE_DIR").then(|| state.clone().into_os_string());
        let row = Row {
            kind: RowKind::Hit,
            id: "id".into(),
            path: path.to_string_lossy().into_owned(),
            ..Row::default()
        };
        let tmux = FakeTmux::silent();
        open_with(&tmux, &row, &[], &env);
        assert!(tmux.calls().is_empty());
        let log = std::fs::read_to_string(state.join(ERROR_LOG)).unwrap();
        assert!(log.contains(&*path.to_string_lossy()), "{log}");
    }

    #[test]
    fn load_and_draw_failures_are_logged_apart() {
        let (mut load, mut draw) = (LogOnce::new("sidebar"), LogOnce::new("sidebar"));
        assert!(load.check(&"x").is_some());
        assert!(draw.check(&"x").is_some());
    }
}
