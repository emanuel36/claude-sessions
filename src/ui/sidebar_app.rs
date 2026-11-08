//! The sidebar of a session window, `cst sidebar`: redrawn on events, idle while hidden.
//!
//! tmux tells the pane when its window comes on screen (F12), leaves it (F11) and when windows
//! or panes change (F10). While shown, inotify reports the records, the journal, Claude Code's
//! history and the transcripts of working or waiting sessions, and the pane reads again only
//! what changed. tmux is called only when shown, on a layout change and on a click.

use std::path::PathBuf;

use anyhow::Context;
use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::widgets::Paragraph;

use crate::claude::history::HistoryReader;
use crate::claude::hook::{self, QUIET_SECONDS};
use crate::claude::statusline::load_limits;
use crate::core::state::unix_now;
use crate::core::state::{self, Record, Status, TIMED};
use crate::term::tmux::{self, Cli, Pane, Runner};
use crate::ui::pane::{self, own_socket, own_window, App, LogOnce, Step, Visible, WINDOW_PAUSE};
use crate::ui::runtime::{signal, Msg, Runtime, Schedule, Screen, Signal};
use crate::ui::sidebar::{column, entry_at, load_entries, Entry, Run, Stats};
use crate::ui::usage::Reader;
use crate::ui::watch::{interruptible, Stale, Watches};

/// Runs the sidebar in the tmux pane this process runs in, until its terminal goes away.
///
/// # Errors
///
/// Fails when it does not run in a tmux pane, or the terminal or the event loop cannot be set
/// up.
pub fn main_sidebar() -> anyhow::Result<()> {
    let tmux = Cli::default();
    let pane = std::env::var("TMUX_PANE").ok();
    let window = own_window(&tmux, pane.as_deref(), WINDOW_PAUSE)?;
    let mut runtime = Runtime::start().context("cannot start the event loop")?;
    let mut screen = Screen::enter().context("cannot set the terminal up")?;
    let mut app = Sidebar::new(window.id, own_socket(), Box::new(tmux));
    // On screen if its window is: a window just opened is, unless the user already moved on;
    // F12 shows it once its window is
    if window.current {
        app.show();
    }
    app.update(unix_now(), &mut runtime, &mut screen);
    pane::run(&mut app, &mut runtime, &mut screen);
    Ok(())
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

/// The sidebar between two wake-ups: what it last read and when it must wake up.
struct Sidebar {
    /// The window the sidebar is in, highlighted in the list.
    window_id: String,
    /// The tmux server's socket; sessions of other servers are left out.
    socket: String,
    /// The server the sidebar runs in.
    tmux: Box<dyn Runner>,
    /// The directories watched now.
    watches: Watches,
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
    fn new(window_id: String, socket: String, tmux: Box<dyn Runner>) -> Self {
        Sidebar {
            window_id,
            socket,
            tmux,
            watches: Watches::default(),
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
            load_error: LogOnce::new("sidebar"),
            draw_error: LogOnce::new("sidebar"),
        }
    }

    /// Takes a message while on screen.
    fn take(&mut self, msg: Msg, now: f64) -> Step {
        match msg {
            Msg::Key(key) => match signal(&key) {
                Some(Signal::Layout) => {
                    self.stale.panes = true;
                    Step::Draw
                }
                // Ctrl+C, Ctrl+Q and every other key do nothing
                _ => Step::Wait,
            },
            Msg::Mouse(mouse) => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    self.click(mouse.row);
                }
                Step::Wait
            }
            Msg::Resize => {
                self.schedule.dirty = true;
                Step::Wait
            }
            Msg::Changed(path) => {
                let touched = self.watches.changed(&path, &self.records, now);
                if touched.any() {
                    self.stale.add(touched);
                    self.schedule.dirty = true;
                }
                Step::Wait
            }
        }
    }

    /// Goes to the window of the entry on row `y`.
    fn click(&self, y: u16) {
        if let Some(entry) = entry_at(&self.entries, y, self.height, &self.stats) {
            self.tmux.run(&["select-window", "-t", &entry.window_id]);
        }
    }

    /// Watches the transcripts of the sessions that can be interrupted, those not watched yet.
    fn watch_transcripts(&mut self, runtime: &mut Runtime) {
        for dir in transcript_dirs(&self.records) {
            // The quiet deadline settles the session if a change was missed
            self.watches.watch(runtime, &dir, &mut self.load_error);
        }
    }

    /// Reads again what is stale.
    fn reload(&mut self, now: f64) {
        let stale = std::mem::take(&mut self.stale);
        if stale.panes {
            self.panes = tmux::panes(self.tmux.as_ref());
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

impl Visible for Sidebar {
    fn visible(&self) -> bool {
        self.schedule.visible
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
        self.watches.clear(runtime);
    }
}

impl App for Sidebar {
    fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    fn handle(&mut self, msg: Msg, now: f64, runtime: &mut Runtime) -> Step {
        pane::gated(self, msg, runtime, |side, msg, _| side.take(msg, now))
    }

    fn update(&mut self, now: f64, runtime: &mut Runtime, screen: &mut Screen) {
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
            let missed = self.watches.roots(runtime, &mut self.load_error);
            self.stale.add(missed);
        }
        self.reload(now);
        self.watch_transcripts(runtime);
        self.schedule.animating = animating(&self.entries);
        self.schedule.labels = labels(&self.entries, &self.runs, &self.stats);
        self.schedule.quiet = quiet_deadlines(&self.records);
        self.draw(now, screen);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    use crate::claude::statusline::Quota;
    use crate::core::state::{Record, Status};
    use crate::term::tmux::fake::FakeTmux;
    use crate::ui::pane::{App, Step, Visible};
    use crate::ui::runtime::{Msg, Runtime};
    use crate::ui::sidebar::{Entry, Run, Stats};
    use crate::ui::sidebar_app::{
        animating, labels, quiet_deadlines, runs_of, transcript_dirs, Sidebar,
    };
    use crate::ui::watch::Stale;

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

    fn click(row: u16) -> Msg {
        Msg::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn what_only_a_shown_sidebar_gets_shows_a_hidden_one() {
        let mut runtime = Runtime::headless().unwrap();
        let layout = Msg::Key(KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE));
        for msg in [Msg::Resize, layout, click(30)] {
            let mut side = Sidebar::new("@1".into(), String::new(), Box::new(FakeTmux::silent()));
            side.hide(&mut runtime);
            let step = side.handle(msg.clone(), NOW, &mut runtime);
            assert_eq!(step, Step::Draw, "{msg:?}");
            assert!(side.schedule.visible, "{msg:?}");
            assert_eq!(side.stale, Stale::ALL, "{msg:?}");
        }
    }

    #[test]
    fn a_hidden_sidebar_drops_changes_on_disk() {
        let mut runtime = Runtime::headless().unwrap();
        let mut side = Sidebar::new("@1".into(), String::new(), Box::new(FakeTmux::silent()));
        side.hide(&mut runtime);
        let changed = Msg::Changed(PathBuf::from("/run/cst/s1.json"));
        assert_eq!(side.handle(changed, NOW, &mut runtime), Step::Wait);
        assert!(!side.schedule.visible && !side.schedule.dirty);
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
}
