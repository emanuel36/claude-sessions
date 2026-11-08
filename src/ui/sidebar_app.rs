//! The sidebar of a session window, `cst sidebar`: redrawn on events, idle while hidden.
//!
//! tmux tells the pane when its window comes on screen (F12), leaves it (F11) and when windows
//! or panes change (F10). While shown, inotify reports the records, the journal, Claude Code's
//! history and the transcripts of working or waiting sessions, and the pane reads again only
//! what changed. tmux is called only when shown, on a layout change and on a click.

use std::collections::HashSet;
use std::fmt::Display;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::widgets::Paragraph;

use crate::claude::history::{HistoryReader, HISTORY};
use crate::claude::hook::{self, QUIET_SECONDS};
use crate::claude::statusline::load_limits;
use crate::journal::journal_dir;
use crate::paths::{self, LIMITS_FILE};
use crate::state::{self, Record, Status, INTERRUPTIBLE, TIMED};
use crate::term::tmux::{self, Cli, Pane, Runner};
use crate::ui::runtime::{signal, unix_now, Msg, Runtime, Schedule, Screen, Signal, Wake};
use crate::ui::sidebar::{column, entry_at, load_entries, Entry, Run, Stats};
use crate::ui::usage::Reader;

/// What a record's file name ends with; its lock file and temporary files do not.
const RECORD_SUFFIX: &str = ".json";

/// Runs the sidebar in the tmux pane this process runs in, until its terminal goes away.
///
/// # Errors
///
/// Fails when it does not run in a tmux pane, or the terminal or the event loop cannot be set
/// up.
pub fn main_sidebar() -> anyhow::Result<()> {
    let tmux = Cli::default();
    let window_id = own_window(&tmux)?;
    let mut runtime = Runtime::start().context("cannot start the event loop")?;
    let mut screen = Screen::enter().context("cannot set the terminal up")?;
    let mut app = Sidebar::new(window_id, own_socket());
    // A sidebar starts on screen: its window was just opened
    app.show();
    app.update(unix_now(), &mut runtime, &mut screen, &tmux);
    loop {
        let deadline = app.schedule.next_wake(unix_now());
        let wake = runtime.wait(deadline);
        let now = unix_now();
        let at_once = match wake {
            Wake::Msg(msg) => app.handle(msg, now, &mut runtime, &tmux),
            Wake::Timeout => true,
            // Neither the terminal nor inotify can wake it any more
            Wake::Closed => return Ok(()),
        };
        // A burst of events redraws at once, then at most every 100 ms
        if at_once || app.schedule.next_wake(now).is_some_and(|at| at <= now) {
            app.update(now, &mut runtime, &mut screen, &tmux);
        }
    }
}

/// The id of the tmux window this process's pane is in.
fn own_window(tmux: &dyn Runner) -> anyhow::Result<String> {
    let Some(pane) = std::env::var("TMUX_PANE").ok().filter(|p| !p.is_empty()) else {
        bail!("not in a tmux pane: TMUX_PANE is not set");
    };
    let window_id = tmux.run(&["display-message", "-p", "-t", &pane, "#{window_id}"]);
    if window_id.is_empty() {
        bail!("cannot find the window of pane {pane}");
    }
    Ok(window_id)
}

/// The socket of the tmux server this process runs in; records name their server by it.
fn own_socket() -> String {
    std::env::var("TMUX")
        .ok()
        .and_then(|t| t.split(',').next().map(str::to_owned))
        .unwrap_or_default()
}

/// What must be read again before the next draw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Stale {
    /// The tmux panes and windows.
    panes: bool,
    /// The session records.
    records: bool,
    /// Claude Code's prompt history.
    history: bool,
    /// The journal of the last days.
    journal: bool,
    /// The usage limits.
    limits: bool,
}

impl Stale {
    /// Nothing.
    const NONE: Stale = Stale {
        panes: false,
        records: false,
        history: false,
        journal: false,
        limits: false,
    };
    /// Everything.
    const ALL: Stale = Stale {
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

    fn any(self) -> bool {
        self != Stale::NONE
    }

    fn add(&mut self, other: Stale) {
        self.panes |= other.panes;
        self.records |= other.records;
        self.history |= other.history;
        self.journal |= other.journal;
        self.limits |= other.limits;
    }
}

/// The directories the sidebar reads from.
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
fn interruptible(r: &Record) -> bool {
    INTERRUPTIBLE.contains(&r.status) && r.background == 0
}

/// When each interruptible session's grace ends, so the sidebar wakes up to settle it.
fn quiet_deadlines(records: &[Record]) -> Vec<f64> {
    records
        .iter()
        .filter(|r| interruptible(r))
        .map(|r| r.updated_at + QUIET_SECONDS)
        .collect()
}

/// The directories of the transcripts of interruptible sessions, once each.
fn transcript_dirs(records: &[Record]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let found = records
        .iter()
        .filter(|r| interruptible(r))
        .filter_map(|r| r.transcript.as_deref()?.parent());
    for dir in found {
        if !dirs.iter().any(|d| d == dir) {
            dirs.push(dir.to_path_buf());
        }
    }
    dirs
}

/// The runs without a terminal still going, oldest first.
fn runs_of(records: &[Record]) -> Vec<Run> {
    let mut runs: Vec<Run> = records
        .iter()
        // Finished, or idle between prompts: not running
        .filter(|r| r.headless && !matches!(r.status, Status::Done | Status::Idle))
        .map(|r| Run {
            name: state::project(r).to_owned(),
            // Records written before runs kept their start fall back to their last change
            started: if r.started_at != 0.0 {
                r.started_at
            } else {
                state::since(r)
            },
        })
        .collect();
    runs.sort_by(|a, b| a.started.total_cmp(&b.started));
    runs
}

/// The start times of the labels that change by the minute: waiting sessions, background runs
/// and quota resets.
fn labels(entries: &[Entry], runs: &[Run], stats: &Stats) -> Vec<f64> {
    let waits = entries
        .iter()
        .filter(|e| TIMED.contains(&e.status))
        .map(|e| e.since);
    let runs = runs.iter().map(|r| r.started);
    let resets = stats.limits.values().map(|q| q.resets_at);
    waits.chain(runs).chain(resets).collect()
}

/// Whether a spinner turns: some session works.
fn animating(entries: &[Entry]) -> bool {
    entries.iter().any(|e| e.status == Status::Working)
}

/// Logs a failure unless it repeats the last one, which would fill the log on every redraw.
#[derive(Debug, Default)]
struct LogOnce {
    last: Option<String>,
}

impl LogOnce {
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
    fn log(&mut self, what: &dyn Display) {
        if let Some(line) = self.check(what) {
            paths::log_error(&format_args!("sidebar: {line}"));
        }
    }
}

/// The sidebar between two wake-ups: what it last read and when it must wake up.
struct Sidebar {
    /// The window the sidebar is in, highlighted in the list.
    window_id: String,
    /// The tmux server's socket; sessions of other servers are left out.
    socket: String,
    /// Where it reads from, once shown.
    dirs: Option<Dirs>,
    /// The directories watched now.
    watching: HashSet<PathBuf>,
    /// What to read again before the next draw.
    stale: Stale,
    /// The server's panes, listed again only on a layout change or when shown.
    panes: Vec<Pane>,
    records: Vec<Record>,
    /// When the records were read.
    records_at: f64,
    entries: Vec<Entry>,
    runs: Vec<Run>,
    stats: Stats,
    /// The minute the statistics were added up in; they change with the time too.
    stats_minute: Option<i64>,
    history: HistoryReader,
    usage: Reader,
    schedule: Schedule,
    /// The terminal's height at the last draw, to find the entry a click lands on.
    height: u16,
    load_error: LogOnce,
    draw_error: LogOnce,
}

impl Sidebar {
    fn new(window_id: String, socket: String) -> Self {
        Sidebar {
            window_id,
            socket,
            dirs: None,
            watching: HashSet::new(),
            stale: Stale::NONE,
            panes: Vec::new(),
            records: Vec::new(),
            records_at: 0.0,
            entries: Vec::new(),
            runs: Vec::new(),
            stats: Stats::default(),
            stats_minute: None,
            history: HistoryReader::new(false),
            usage: Reader::new(),
            schedule: Schedule::default(),
            height: 0,
            load_error: LogOnce::default(),
            draw_error: LogOnce::default(),
        }
    }

    /// On screen: everything is read again, panes included, before the next draw.
    fn show(&mut self) {
        self.schedule.visible = true;
        self.stale = Stale::ALL;
    }

    /// Off screen: no watches, no timer, nothing read until shown again.
    fn hide(&mut self, runtime: &mut Runtime) {
        self.schedule.visible = false;
        self.schedule.dirty = false;
        runtime.unwatch_all();
        self.watching.clear();
    }

    /// Takes one message; returns whether to read and draw at once.
    fn handle(&mut self, msg: Msg, now: f64, runtime: &mut Runtime, tmux: &dyn Runner) -> bool {
        let visible = self.schedule.visible;
        match msg {
            Msg::Key(key) => match signal(&key) {
                Some(Signal::Shown) => {
                    self.show();
                    true
                }
                Some(Signal::Hidden) => {
                    self.hide(runtime);
                    false
                }
                Some(Signal::Layout) if visible => {
                    self.stale.panes = true;
                    true
                }
                // Ctrl+C, Ctrl+Q and every other key do nothing
                _ => false,
            },
            Msg::Mouse(mouse) if visible => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    self.click(mouse.row, tmux);
                }
                false
            }
            Msg::Resize if visible => {
                self.schedule.dirty = true;
                false
            }
            Msg::Changed(path) if visible => {
                // A watched directory reported itself: watch it again if it is gone
                self.watching.remove(&path);
                if let Some(dirs) = &self.dirs {
                    let touched = dirs.touched(&path, &self.records, now);
                    if touched.any() {
                        self.stale.add(touched);
                        self.schedule.dirty = true;
                    }
                }
                false
            }
            // Hidden: events queued before the watches went are dropped
            Msg::Mouse(_) | Msg::Resize | Msg::Changed(_) => false,
        }
    }

    /// Goes to the window of the entry on row `y`.
    fn click(&self, y: u16, tmux: &dyn Runner) {
        if let Some(entry) = entry_at(&self.entries, y, self.height, &self.stats) {
            tmux.run(&["select-window", "-t", &entry.window_id]);
        }
    }

    /// Reads again what is stale, then draws.
    fn update(&mut self, now: f64, runtime: &mut Runtime, screen: &mut Screen, tmux: &dyn Runner) {
        if !self.schedule.visible {
            return;
        }
        // A session's grace ended since the records were read: they settle it
        if (self.schedule.quiet.iter()).any(|&at| at > self.records_at && at <= now) {
            self.stale.records = true;
        }
        if self.stats.limits.values().any(|q| q.resets_at <= now) {
            self.stale.limits = true;
        }
        if self.stale.any() {
            self.watch_roots(runtime);
        }
        self.reload(now, tmux);
        self.watch_transcripts(runtime);
        self.schedule.animating = animating(&self.entries);
        self.schedule.labels = labels(&self.entries, &self.runs, &self.stats);
        self.schedule.quiet = quiet_deadlines(&self.records);
        self.draw(now, screen);
    }

    /// Watches the records, the journal and Claude Code's folder, those not watched yet.
    ///
    /// Watched again after one was deleted, and read again since changes were missed.
    fn watch_roots(&mut self, runtime: &mut Runtime) {
        let dirs = match Dirs::resolve() {
            Ok(dirs) => dirs,
            Err(e) => {
                self.load_error
                    .log(&format_args!("cannot use the records folder: {e}"));
                return;
            }
        };
        for (dir, holds) in dirs.roots() {
            if self.watch(runtime, dir) {
                self.stale.add(holds);
            }
        }
        self.dirs = Some(dirs);
    }

    /// Watches the transcripts of the sessions that can be interrupted, those not watched yet.
    fn watch_transcripts(&mut self, runtime: &mut Runtime) {
        for dir in transcript_dirs(&self.records) {
            // The quiet deadline settles the session if a change was missed
            self.watch(runtime, &dir);
        }
    }

    /// Watches a directory unless it is watched; returns whether it was not.
    ///
    /// A directory that does not exist yet is tried again on the next read.
    fn watch(&mut self, runtime: &mut Runtime, dir: &Path) -> bool {
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
                self.load_error
                    .log(&format_args!("cannot watch {shown}: {e}"));
                false
            }
        }
    }

    /// Reads again what is stale.
    fn reload(&mut self, now: f64, tmux: &dyn Runner) {
        let stale = std::mem::take(&mut self.stale);
        if stale.panes {
            self.panes = tmux::panes(tmux);
        }
        if stale.records {
            self.records = hook::live_sessions(now);
            self.records_at = now;
        }
        if stale.records || stale.panes {
            self.entries = load_entries(&self.records, &self.panes, &self.socket);
            self.runs = runs_of(&self.records);
        }
        if stale.history {
            self.history.load();
        }
        // Today's waiting grows with the time, and the week moves on at midnight
        let minute = (now / 60.0).floor() as i64;
        if stale.history || stale.journal || self.stats_minute != Some(minute) {
            let generation = self.history.generation();
            let totals = self.usage.totals(self.history.sessions(), generation, now);
            self.stats.today = totals.today;
            self.stats.week = totals.days.clone();
            self.stats.day = Some(self.usage.day(now));
            self.stats_minute = Some(minute);
        }
        if stale.limits {
            self.stats.limits = load_limits(now);
        }
    }

    /// Draws the column; a failed draw keeps the last screen.
    fn draw(&mut self, now: f64, screen: &mut Screen) {
        let mut height = self.height;
        let drawn = screen.draw(|frame| {
            let area = frame.area();
            height = area.height;
            let lines = column(
                &self.entries,
                &self.window_id,
                &self.stats,
                now,
                area.height,
                &self.runs,
            );
            frame.render_widget(Paragraph::new(lines), area);
        });
        self.height = height;
        if let Err(e) = drawn {
            self.draw_error.log(&format_args!("cannot draw: {e}"));
        }
        self.schedule.last_draw = now;
        self.schedule.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use crate::claude::statusline::Quota;
    use crate::state::{Record, Status};
    use crate::ui::sidebar::{Entry, Run, Stats};
    use crate::ui::sidebar_app::{
        animating, labels, quiet_deadlines, runs_of, transcript_dirs, Dirs, LogOnce, Stale,
    };

    const NOW: f64 = 1_700_000_000.0;

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
    fn quiet_deadlines_are_three_seconds_after_the_last_hook() {
        let working = record("a", Status::Working);
        let asking = Record {
            updated_at: NOW - 10.0,
            ..record("b", Status::Input)
        };
        let done = record("c", Status::Done);
        let background = Record {
            background: 2,
            ..record("d", Status::Permission)
        };
        let got = quiet_deadlines(&[working, asking, done, background]);
        assert_eq!(got, [NOW + 2.0, NOW - 7.0]);
    }

    #[test]
    fn transcripts_of_interruptible_sessions_are_watched_once() {
        let a = record("a", Status::Working);
        let a2 = Record {
            id: "a2".to_owned(),
            ..a.clone()
        };
        let none = Record {
            transcript: None,
            ..record("n", Status::Permission)
        };
        let idle = record("i", Status::Idle);
        let got = transcript_dirs(&[a, a2, none, idle, record("b", Status::Input)]);
        assert_eq!(got, [PathBuf::from("/p/a"), PathBuf::from("/p/b")]);
    }

    #[test]
    fn headless_runs_are_listed_until_they_finish() {
        let run = Record {
            headless: true,
            started_at: NOW - 120.0,
            cwd: "/tmp/daily-recap".to_owned(),
            ..record("r1", Status::Working)
        };
        let done = Record {
            status: Status::Done,
            cwd: "/tmp/trending-repos".to_owned(),
            ..run.clone()
        };
        let waiting = Record {
            status: Status::Idle,
            cwd: "/tmp/sdk-app".to_owned(),
            ..run.clone()
        };
        // Records written before runs kept their start fall back to their last change
        let old = Record {
            started_at: 0.0,
            status_since: NOW - 60.0,
            cwd: "/tmp/old".to_owned(),
            ..run.clone()
        };
        let not_headless = record("x", Status::Working);
        let got = runs_of(&[run, done, old, waiting, not_headless]);
        let want = [
            Run {
                name: "daily-recap".to_owned(),
                started: NOW - 120.0,
            },
            Run {
                name: "old".to_owned(),
                started: NOW - 60.0,
            },
        ];
        assert_eq!(got, want);
    }

    fn entry(status: Status, since: f64) -> Entry {
        Entry {
            window_id: "@1".to_owned(),
            name: "api".to_owned(),
            status,
            since,
            ..Entry::default()
        }
    }

    #[test]
    fn labels_are_waits_runs_and_resets() {
        let entries = [
            entry(Status::Permission, NOW - 30.0),
            entry(Status::Done, NOW - 40.0),
            entry(Status::Shell, NOW - 50.0),
        ];
        let runs = [Run {
            name: "r".to_owned(),
            started: NOW - 70.0,
        }];
        let stats = Stats {
            limits: BTreeMap::from([(
                "five_hour".to_owned(),
                Quota {
                    pct: 10.0,
                    resets_at: NOW + 3600.0,
                },
            )]),
            ..Stats::default()
        };
        let got = labels(&entries, &runs, &stats);
        assert_eq!(got, [NOW - 30.0, NOW - 50.0, NOW - 70.0, NOW + 3600.0]);
    }

    #[test]
    fn only_a_working_session_animates() {
        assert!(!animating(&[entry(Status::Permission, NOW)]));
        assert!(animating(&[
            entry(Status::Idle, NOW),
            entry(Status::Working, NOW)
        ]));
    }

    #[test]
    fn a_failure_is_logged_once_until_another_one() {
        let mut log = LogOnce::default();
        assert_eq!(log.check(&"x: gone"), Some("x: gone".to_owned()));
        assert_eq!(log.check(&"x: gone"), None);
        assert_eq!(log.check(&"y: bad"), Some("y: bad".to_owned()));
        assert_eq!(log.check(&"x: gone"), Some("x: gone".to_owned()));
    }

    #[test]
    fn load_and_draw_failures_are_logged_apart() {
        let (mut load, mut draw) = (LogOnce::default(), LogOnce::default());
        assert!(load.check(&"x").is_some());
        assert!(draw.check(&"x").is_some());
    }
}
