//! What the sidebar, the landing page and the Alt+n panel share: the loop they run on, how they
//! learn whether they are on screen, the folders they watch and how they log failures.

use std::collections::HashSet;
use std::fmt::Display;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::bail;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::claude::history::HISTORY;
use crate::claude::hook::QUIET_SECONDS;
use crate::journal::journal_dir;
use crate::paths::{self, LIMITS_FILE};
use crate::state::{Record, INTERRUPTIBLE};
use crate::term::tmux::Runner;
use crate::ui::picker;
use crate::ui::runtime::{signal, unix_now, Msg, Runtime, Schedule, Screen, Signal, Wake};

/// What a record's file name ends with; its lock file and temporary files do not.
const RECORD_SUFFIX: &str = ".json";
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

/// The id of the tmux window `pane` is in, asking tmux up to five times, `pause` apart.
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
) -> anyhow::Result<String> {
    let Some(pane) = pane.filter(|p| !p.is_empty()) else {
        bail!("not in a tmux pane: TMUX_PANE is not set");
    };
    for attempt in 0..WINDOW_TRIES {
        if attempt > 0 {
            thread::sleep(pause);
        }
        let window_id = tmux.run(&["display-message", "-p", "-t", pane, "#{window_id}"]);
        if !window_id.is_empty() {
            return Ok(window_id);
        }
    }
    bail!("cannot find the window of pane {pane}")
}

/// The socket of the tmux server this process runs in; records name their server by it.
pub fn own_socket() -> String {
    std::env::var("TMUX")
        .ok()
        .and_then(|t| t.split(',').next().map(str::to_owned))
        .unwrap_or_default()
}

/// Whether a key ends a panel: Ctrl+C or Ctrl+Q.
pub fn quits(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'q'))
}

/// The key the folder list takes for a key press, if any: arrows, Enter, Backspace, and a
/// character typed with no modifier but Shift.
pub fn picker_key(key: &KeyEvent) -> Option<picker::Key> {
    Some(match key.code {
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
pub fn open(tmux: &dyn Runner, row: &picker::Row, folders: &[picker::Folder]) {
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

/// What must be read again before the next draw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stale {
    /// The tmux panes and windows.
    pub panes: bool,
    /// The session records.
    pub records: bool,
    /// Claude Code's prompt history.
    pub history: bool,
    /// The journal of the last days.
    pub journal: bool,
    /// The usage limits.
    pub limits: bool,
}

impl Stale {
    /// Nothing.
    pub const NONE: Stale = Stale {
        panes: false,
        records: false,
        history: false,
        journal: false,
        limits: false,
    };
    /// Everything.
    pub const ALL: Stale = Stale {
        panes: true,
        records: true,
        history: true,
        journal: true,
        limits: true,
    };
    /// What the records directory holds.
    const RECORDS_DIR: Stale = Stale {
        records: true,
        limits: true,
        ..Stale::NONE
    };
    /// A record, or a transcript, which tells only whether its session was interrupted: the
    /// records settle that.
    const RECORDS: Stale = Stale {
        records: true,
        ..Stale::NONE
    };
    /// The limits file.
    const LIMITS: Stale = Stale {
        limits: true,
        ..Stale::NONE
    };
    /// The journal's day files.
    const JOURNAL: Stale = Stale {
        journal: true,
        ..Stale::NONE
    };
    /// Claude Code's prompt history.
    const HISTORY: Stale = Stale {
        history: true,
        ..Stale::NONE
    };

    /// Whether anything is stale.
    pub fn any(self) -> bool {
        self != Stale::NONE
    }

    /// Adds what `other` makes stale.
    pub fn add(&mut self, other: Stale) {
        self.panes |= other.panes;
        self.records |= other.records;
        self.history |= other.history;
        self.journal |= other.journal;
        self.limits |= other.limits;
    }
}

/// The directories the panes read from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Dirs {
    /// The records and the limits.
    records: PathBuf,
    /// The journal's day files.
    journal: PathBuf,
    /// Claude Code's folder, which holds `history.jsonl`.
    config: PathBuf,
}

impl Dirs {
    /// Resolves the directories; the records directory is created if it is missing.
    fn resolve() -> io::Result<Self> {
        Ok(Dirs {
            records: paths::runtime_dir()?,
            journal: journal_dir(),
            config: paths::config_dir(),
        })
    }

    /// The directories always watched, with what each holds.
    fn roots(&self) -> [(&Path, Stale); 3] {
        [
            (&self.records, Stale::RECORDS_DIR),
            (&self.journal, Stale::JOURNAL),
            (&self.config, Stale::HISTORY),
        ]
    }

    /// What a change at `path` makes stale, given the records last read.
    ///
    /// A watched directory itself stands for all it holds: its events overflowed, or it was
    /// deleted. A transcript counts only for a session past its grace, which the records then
    /// settle.
    fn touched(&self, path: &Path, records: &[Record], now: f64) -> Stale {
        if let Some((_, holds)) = self.roots().into_iter().find(|(dir, _)| *dir == path) {
            return holds;
        }
        let in_dir = |dir: &Path| path.parent() == Some(dir);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if in_dir(&self.records) {
            if name == LIMITS_FILE {
                return Stale::LIMITS;
            }
            if name.ends_with(RECORD_SUFFIX) && !name.starts_with('.') {
                return Stale::RECORDS;
            }
            return Stale::NONE;
        }
        if in_dir(&self.journal) {
            return Stale::JOURNAL;
        }
        if in_dir(&self.config) {
            return if name == HISTORY {
                Stale::HISTORY
            } else {
                Stale::NONE
            };
        }
        let settles = records.iter().filter(|r| interruptible(r)).any(|r| {
            let transcript = r.transcript.as_deref();
            transcript.and_then(Path::parent) == Some(path)
                || (transcript == Some(path) && now - r.updated_at > QUIET_SECONDS)
        });
        if settles {
            Stale::RECORDS
        } else {
            Stale::NONE
        }
    }
}

/// Whether a session can have been interrupted with Esc unseen: no hook reports that, so its
/// transcript is checked once it has had no hook for [`QUIET_SECONDS`].
pub fn interruptible(r: &Record) -> bool {
    INTERRUPTIBLE.contains(&r.status) && r.background == 0
}

/// The directories a pane watches while it is on screen: the records, the journal and Claude
/// Code's folder, and any other it adds, such as transcripts' folders.
#[derive(Debug, Default)]
pub struct Watches {
    /// Where the pane reads from, once it watched the roots.
    dirs: Option<Dirs>,
    /// The directories watched now.
    watching: HashSet<PathBuf>,
}

impl Watches {
    /// Watches the records, the journal and Claude Code's folder, those not watched yet.
    ///
    /// Returns what the newly watched ones hold, to be read again: their changes were missed
    /// while they were not watched (or were deleted).
    pub fn roots(&mut self, runtime: &mut Runtime, log: &mut LogOnce) -> Stale {
        let dirs = match Dirs::resolve() {
            Ok(dirs) => dirs,
            Err(e) => {
                log.log(&format_args!("cannot use the records folder: {e}"));
                return Stale::NONE;
            }
        };
        let mut stale = Stale::NONE;
        for (dir, holds) in dirs.roots() {
            if self.watch(runtime, dir, log) {
                stale.add(holds);
            }
        }
        self.dirs = Some(dirs);
        stale
    }

    /// Watches a directory unless it is watched; returns whether it was not.
    ///
    /// A directory that does not exist yet is tried again on the next call.
    pub fn watch(&mut self, runtime: &mut Runtime, dir: &Path, log: &mut LogOnce) -> bool {
        if self.watching.contains(dir) {
            return false;
        }
        match runtime.watch(dir) {
            Ok(()) => {
                self.watching.insert(dir.to_path_buf());
                true
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => {
                let shown = dir.display();
                log.log(&format_args!("cannot watch {shown}: {e}"));
                false
            }
        }
    }

    /// What a change at `path` makes stale, given the records last read; a watched directory
    /// that reported itself is watched again on the next [`Watches::roots`] or
    /// [`Watches::watch`].
    pub fn changed(&mut self, path: &Path, records: &[Record], now: f64) -> Stale {
        self.watching.remove(path);
        self.dirs
            .as_ref()
            .map_or(Stale::NONE, |dirs| dirs.touched(path, records, now))
    }

    /// Drops every watch, as a pane does when it leaves the screen.
    pub fn clear(&mut self, runtime: &mut Runtime) {
        runtime.unwatch_all();
        self.watching.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    use crate::state::{Record, Status};
    use crate::term::tmux::Runner;
    use crate::ui::pane::{own_window, picker_key, quits, sight, Dirs, LogOnce, Sight, Stale};
    use crate::ui::picker::Key;
    use crate::ui::runtime::Msg;

    const NOW: f64 = 1_700_000_000.0;

    /// Answers each call with the next of its answers, then with nothing.
    struct Answers(RefCell<Vec<&'static str>>, RefCell<u32>);

    impl Answers {
        fn new(answers: &[&'static str]) -> Self {
            let mut answers = answers.to_vec();
            answers.reverse();
            Answers(RefCell::new(answers), RefCell::new(0))
        }

        fn calls(&self) -> u32 {
            *self.1.borrow()
        }
    }

    impl Runner for Answers {
        fn run(&self, args: &[&str]) -> String {
            assert_eq!(args, ["display-message", "-p", "-t", "%7", "#{window_id}"]);
            *self.1.borrow_mut() += 1;
            self.0.borrow_mut().pop().unwrap_or_default().to_owned()
        }
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
        let tmux = Answers::new(&["", "", "@3"]);
        let got = own_window(&tmux, Some("%7"), Duration::ZERO).unwrap();
        assert_eq!((got.as_str(), tmux.calls()), ("@3", 3));
    }

    #[test]
    fn own_window_gives_up_after_five_asks() {
        let tmux = Answers::new(&[]);
        let err = own_window(&tmux, Some("%7"), Duration::ZERO).unwrap_err();
        assert_eq!(tmux.calls(), 5);
        assert!(err.to_string().contains("%7"), "{err}");
    }

    #[test]
    fn own_window_needs_a_pane() {
        let tmux = Answers::new(&["@3"]);
        assert!(own_window(&tmux, None, Duration::ZERO).is_err());
        assert!(own_window(&tmux, Some(""), Duration::ZERO).is_err());
        assert_eq!(tmux.calls(), 0);
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
        assert_eq!(picker_key(&key(KeyCode::Tab)), None);
        assert_eq!(picker_key(&key(KeyCode::Esc)), None);
    }

    fn record(id: &str, status: Status) -> Record {
        Record {
            id: id.to_owned(),
            status,
            cwd: format!("/w/{id}"),
            updated_at: NOW - 1.0,
            transcript: Some(PathBuf::from(format!("/p/{id}/t.jsonl"))),
            ..Record::default()
        }
    }

    fn dirs() -> Dirs {
        Dirs {
            records: PathBuf::from("/run/cst"),
            journal: PathBuf::from("/state/journal"),
            config: PathBuf::from("/home/.claude"),
        }
    }

    fn only(set: impl Fn(&mut Stale)) -> Stale {
        let mut stale = Stale::NONE;
        set(&mut stale);
        stale
    }

    #[test]
    fn a_record_written_makes_the_records_stale() {
        let got = dirs().touched(Path::new("/run/cst/s1.json"), &[], NOW);
        assert_eq!(got, only(|s| s.records = true));
    }

    #[test]
    fn temp_and_lock_files_change_nothing() {
        for name in [".tmp-12-0", "s1.json.lock"] {
            let path = Path::new("/run/cst").join(name);
            assert_eq!(dirs().touched(&path, &[], NOW), Stale::NONE, "{name}");
        }
    }

    #[test]
    fn the_limits_file_makes_the_limits_stale() {
        let got = dirs().touched(Path::new("/run/cst/limits.json"), &[], NOW);
        assert_eq!(got, only(|s| s.limits = true));
    }

    #[test]
    fn a_journal_day_makes_the_journal_stale() {
        let got = dirs().touched(Path::new("/state/journal/2026-11-08.jsonl"), &[], NOW);
        assert_eq!(got, only(|s| s.journal = true));
    }

    #[test]
    fn only_the_history_file_of_the_config_dir_counts() {
        let got = dirs().touched(Path::new("/home/.claude/history.jsonl"), &[], NOW);
        assert_eq!(got, only(|s| s.history = true));
        let other = dirs().touched(Path::new("/home/.claude/settings.json"), &[], NOW);
        assert_eq!(other, Stale::NONE);
    }

    #[test]
    fn a_watched_directory_itself_makes_its_sources_stale() {
        // It overflowed, or was deleted and must be watched again
        let records = dirs().touched(Path::new("/run/cst"), &[], NOW);
        assert_eq!(
            records,
            only(|s| {
                s.records = true;
                s.limits = true;
            })
        );
        let journal = dirs().touched(Path::new("/state/journal"), &[], NOW);
        assert_eq!(journal, only(|s| s.journal = true));
        let config = dirs().touched(Path::new("/home/.claude"), &[], NOW);
        assert_eq!(config, only(|s| s.history = true));
        let transcripts = dirs().touched(Path::new("/p/a"), &[record("a", Status::Working)], NOW);
        assert_eq!(transcripts, only(|s| s.records = true));
    }

    #[test]
    fn a_transcript_counts_only_past_its_grace() {
        let path = Path::new("/p/a/t.jsonl");
        let fresh = record("a", Status::Working);
        assert_eq!(
            dirs().touched(path, std::slice::from_ref(&fresh), NOW),
            Stale::NONE
        );
        let quiet = Record {
            updated_at: NOW - 3.5,
            ..fresh
        };
        assert_eq!(
            dirs().touched(path, std::slice::from_ref(&quiet), NOW),
            only(|s| s.records = true)
        );
        let idle = Record {
            status: Status::Idle,
            ..quiet.clone()
        };
        assert_eq!(dirs().touched(path, &[idle], NOW), Stale::NONE);
        let background = Record {
            background: 1,
            ..quiet
        };
        assert_eq!(dirs().touched(path, &[background], NOW), Stale::NONE);
    }

    #[test]
    fn a_failure_is_logged_once_until_another_one() {
        let mut log = LogOnce::new("sidebar");
        assert_eq!(log.check(&"x: gone"), Some("x: gone".to_owned()));
        assert_eq!(log.check(&"x: gone"), None);
        assert_eq!(log.check(&"y: bad"), Some("y: bad".to_owned()));
        assert_eq!(log.check(&"x: gone"), Some("x: gone".to_owned()));
    }

    #[test]
    fn load_and_draw_failures_are_logged_apart() {
        let (mut load, mut draw) = (LogOnce::new("sidebar"), LogOnce::new("sidebar"));
        assert!(load.check(&"x").is_some());
        assert!(draw.check(&"x").is_some());
    }
}
