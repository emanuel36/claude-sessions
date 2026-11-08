//! The landing page, `cst landing`: what `cst` opens with, and what it returns to when no
//! session is left.
//!
//! It runs in the first window, which stays for the life of the server. Like the sidebar it
//! learns from tmux when it comes on screen (F12) and leaves it (F11), and while shown it reads
//! again what inotify reports changed in the records, the journal and Claude Code's history.

use std::collections::HashSet;

use anyhow::Context;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::Paragraph;

use crate::claude::history::HistoryReader;
use crate::claude::hook;
use crate::claude::statusline::load_limits;
use crate::claude::transcript::Titles;
use crate::state::Record;
use crate::term::tmux::{Cli, Runner};
use crate::ui::folders::Scan;
use crate::ui::landing;
use crate::ui::pane::{self, open, picker_key, App, LogOnce, Stale, Step, Visible, Watches};
use crate::ui::picker::{self, Folder, Picker};
use crate::ui::runtime::{signal, unix_now, Msg, Runtime, Schedule, Screen, Signal};
use crate::ui::sidebar::Stats;
use crate::ui::usage::Reader;

/// Runs the landing page in the tmux pane this process runs in, until its terminal goes away.
///
/// # Errors
///
/// Fails when the terminal or the event loop cannot be set up.
pub fn main_landing() -> anyhow::Result<()> {
    let mut runtime = Runtime::start().context("cannot start the event loop")?;
    let mut screen = Screen::enter().context("cannot set the terminal up")?;
    let mut app = Landing::new(Box::new(Cli::default()), Picker::default());
    // The page starts on screen: its window was just opened
    app.show();
    app.update(unix_now(), &mut runtime, &mut screen);
    pane::run(&mut app, &mut runtime, &mut screen);
    Ok(())
}

/// How many sessions are open, and the ids not to offer to resume.
///
/// A running `claude -p` is no session to resume either, though it is not counted open.
fn open_sessions(records: &[Record]) -> (usize, HashSet<String>) {
    let opened = records.iter().filter(|r| !r.headless).count();
    (opened, records.iter().map(|r| r.id.clone()).collect())
}

/// The start times of what changes by the minute: the folders' and sessions' ages and the
/// quota resets, and every whole minute for the date and today's figures.
fn labels(folders: &[Folder], stats: &Stats) -> Vec<f64> {
    // From the epoch, a label changes on every whole minute
    let minutes = std::iter::once(0.0);
    let ages = folders
        .iter()
        .flat_map(|f| std::iter::once(f.last).chain(f.sessions.iter().map(|s| s.last)));
    let resets = stats.limits.values().map(|q| q.resets_at);
    minutes.chain(ages).chain(resets).collect()
}

/// The landing page between two wake-ups: what it last read and when it must wake up.
struct Landing {
    /// The server the page runs in.
    tmux: Box<dyn Runner>,
    /// The directories watched now.
    watches: Watches,
    /// What to read again before the next draw.
    stale: Stale,
    /// The folder list and what the user did in it.
    picker: Picker,
    /// Whether the user is typing a search.
    searching: bool,
    /// How many sessions are open.
    opened: usize,
    /// The sessions running now, not offered to resume.
    live: HashSet<String>,
    history: HistoryReader,
    usage: Reader,
    scan: Scan,
    stats: Stats,
    /// The minute the statistics were added up in; they change with the time too.
    stats_minute: Option<i64>,
    schedule: Schedule,
    load_error: LogOnce,
    draw_error: LogOnce,
}

impl Landing {
    /// A page that runs tmux commands with `tmux`, its folders listed in `picker`; history and
    /// titles are kept on disk for the next process.
    fn new(tmux: Box<dyn Runner>, picker: Picker) -> Self {
        Landing {
            tmux,
            watches: Watches::default(),
            stale: Stale::NONE,
            picker,
            searching: false,
            opened: 0,
            live: HashSet::new(),
            history: HistoryReader::new(true),
            usage: Reader::new(),
            scan: Scan::new(Titles::new(true)),
            stats: Stats::default(),
            stats_minute: None,
            schedule: Schedule::default(),
            load_error: LogOnce::new("landing"),
            draw_error: LogOnce::new("landing"),
        }
    }

    /// Takes a message while on screen.
    fn take(&mut self, msg: Msg, now: f64) -> Step {
        match msg {
            Msg::Key(key) => match signal(&key) {
                // A window closed or opened: the open sessions may have changed
                Some(Signal::Layout) => {
                    self.stale.records = true;
                    Step::Draw
                }
                Some(_) => Step::Wait,
                None if self.key(&key) => Step::Draw,
                None => Step::Wait,
            },
            Msg::Mouse(_) => Step::Wait,
            Msg::Resize => {
                self.schedule.dirty = true;
                Step::Wait
            }
            Msg::Changed(path) => {
                // No transcript is watched: the page shows no session's status
                let touched = self.watches.changed(&path, &[], now);
                if touched.any() {
                    self.stale.add(touched);
                    self.schedule.dirty = true;
                }
                Step::Wait
            }
        }
    }

    /// Takes a key the user typed; returns whether the page may have changed.
    ///
    /// Searching, Enter and Esc end the search (Esc also clears it) and every other key goes
    /// to the folder list. Otherwise `/` starts a search, `q` ends the server, Esc clears the
    /// search, and the arrows and Enter go to the list, Enter opening a window.
    fn key(&mut self, key: &KeyEvent) -> bool {
        if self.searching {
            match key.code {
                KeyCode::Enter => self.searching = false,
                KeyCode::Esc => {
                    self.searching = false;
                    self.picker.query.clear();
                }
                _ => match picker_key(key) {
                    Some(key) => {
                        self.picker.key(key);
                    }
                    None => return false,
                },
            }
            return true;
        }
        let Some(typed) = picker_key(key) else {
            if key.code != KeyCode::Esc {
                return false;
            }
            self.picker.query.clear();
            return true;
        };
        match typed {
            picker::Key::Char('/') => self.searching = true,
            picker::Key::Char('q') => {
                self.tmux.run(&["kill-server"]);
            }
            picker::Key::Char(_) | picker::Key::Backspace => return false,
            _ => {
                if let Some(row) = self.picker.key(typed) {
                    open(self.tmux.as_ref(), &row, &self.picker.folders);
                }
            }
        }
        true
    }

    /// Reads again what is stale.
    fn reload(&mut self, now: f64) {
        let stale = std::mem::take(&mut self.stale);
        if stale.records {
            let records = hook::live_sessions(now);
            (self.opened, self.live) = open_sessions(&records);
        }
        if stale.history {
            self.history.load();
        }
        // Today's figures grow with the time, and the week moves on at midnight
        let minute = (now / 60.0).floor() as i64;
        let totals_due = stale.history || stale.journal || self.stats_minute != Some(minute);
        if totals_due {
            let generation = self.history.generation();
            let totals = self.usage.totals(self.history.sessions(), generation, now);
            self.stats.today = totals.today;
            self.stats.week = totals.days.clone();
            self.stats.totals = Some(totals.clone());
            self.stats.day = Some(self.usage.day(now));
            self.stats.hours = self.usage.hours(now).to_vec();
            self.stats_minute = Some(minute);
        }
        if stale.records || totals_due {
            let costs = self.stats.totals.as_ref().map(|t| &t.per_session);
            let generation = self.history.generation();
            self.picker.folders = self.scan.load(
                self.history.sessions(),
                generation,
                costs.unwrap_or(&Default::default()),
                &self.live,
            );
        }
        if stale.limits {
            self.stats.limits = load_limits(now);
        }
    }

    /// Draws the page; a failed draw keeps the last screen.
    fn draw(&mut self, now: f64, screen: &mut Screen) {
        let drawn = screen.draw(|frame| {
            let area = frame.area();
            let lines = landing::screen(
                &mut self.picker,
                self.searching,
                self.opened,
                &self.stats,
                now,
                area.width,
                area.height,
            );
            frame.render_widget(Paragraph::new(lines), area);
        });
        if let Err(e) = drawn {
            self.draw_error.log(&format_args!("cannot draw: {e}"));
        }
        self.schedule.last_draw = now;
        self.schedule.dirty = false;
    }
}

impl Visible for Landing {
    fn visible(&self) -> bool {
        self.schedule.visible
    }

    /// On screen: everything is read again before the next draw.
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

impl App for Landing {
    fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    fn handle(&mut self, msg: Msg, now: f64, runtime: &mut Runtime) -> Step {
        pane::gated(self, msg, runtime, |page, msg, _| page.take(msg, now))
    }

    fn update(&mut self, now: f64, runtime: &mut Runtime, screen: &mut Screen) {
        if !self.schedule.visible {
            return;
        }
        if self.stats.limits.values().any(|q| q.resets_at <= now) {
            self.stale.limits = true;
        }
        if self.stale.any() {
            let missed = self.watches.roots(runtime, &mut self.load_error);
            self.stale.add(missed);
        }
        self.reload(now);
        self.schedule.labels = labels(&self.picker.folders, &self.stats);
        self.draw(now, screen);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, HashSet};
    use std::path::PathBuf;
    use std::rc::Rc;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::claude::statusline::Quota;
    use crate::state::Record;
    use crate::term::tmux::Runner;
    use crate::ui::landing_app::{labels, open_sessions, Landing};
    use crate::ui::pane::{App, LogOnce, Step, Visible};
    use crate::ui::picker::{Folder, Picker, Session};
    use crate::ui::runtime::{Msg, Runtime};
    use crate::ui::sidebar::Stats;

    const NOW: f64 = 1_700_000_000.0;

    /// Records the tmux commands it is asked to run.
    #[derive(Clone, Default)]
    struct Calls(Rc<RefCell<Vec<Vec<String>>>>);

    impl Calls {
        fn all(&self) -> Vec<Vec<String>> {
            self.0.borrow().clone()
        }

        fn ran(&self, command: &[&str]) -> bool {
            self.all().iter().any(|c| c == command)
        }
    }

    impl Runner for Calls {
        fn run(&self, args: &[&str]) -> String {
            let args = args.iter().map(|a| (*a).to_owned()).collect();
            self.0.borrow_mut().push(args);
            String::new()
        }
    }

    fn folders() -> Vec<Folder> {
        vec![
            Folder {
                path: "/w/api".into(),
                name: "api".into(),
                last: NOW,
                count: 1,
                spend: 3.0,
                sessions: vec![Session {
                    id: "s1".into(),
                    title: "fix it".into(),
                    last: NOW - 60.0,
                    cost: Some(1.0),
                }],
            },
            Folder {
                path: "/w/web".into(),
                name: "web".into(),
                last: NOW - 9.0,
                count: 0,
                spend: 0.0,
                sessions: Vec::new(),
            },
        ]
    }

    /// A page on screen over two folders, its tmux calls recorded.
    fn page() -> (Landing, Calls, Runtime) {
        let calls = Calls::default();
        let mut picker = Picker::new(Box::new(|_| false));
        picker.folders = folders();
        let mut page = Landing::new(Box::new(calls.clone()), picker);
        page.schedule.visible = true;
        (page, calls, Runtime::headless().unwrap())
    }

    fn press(page: &mut Landing, runtime: &mut Runtime, codes: &[KeyCode]) {
        for &code in codes {
            let key = KeyEvent::new(code, KeyModifiers::NONE);
            page.handle(Msg::Key(key), NOW, runtime);
        }
    }

    #[test]
    fn enter_on_a_folder_starts_a_session_there() {
        let (mut page, calls, mut runtime) = page();
        press(&mut page, &mut runtime, &[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(calls.all(), [["new-window", "-c", "/w/web", "claude"]]);
    }

    #[test]
    fn right_opens_a_folders_sessions_and_enter_resumes_one() {
        let (mut page, calls, mut runtime) = page();
        let keys = [KeyCode::Right, KeyCode::Down, KeyCode::Enter];
        press(&mut page, &mut runtime, &keys);
        assert!(calls.ran(&["new-window", "-c", "/w/api", "claude --resume s1"]));
    }

    #[test]
    fn slash_searches_and_enter_ends_the_search() {
        let (mut page, calls, mut runtime) = page();
        let keys = [
            KeyCode::Char('/'),
            KeyCode::Char('w'),
            KeyCode::Char('e'),
            KeyCode::Char('b'),
            KeyCode::Enter,
        ];
        press(&mut page, &mut runtime, &keys);
        assert_eq!(page.picker.query, "web");
        assert!(!page.searching);
        assert_eq!(calls.all(), Vec::<Vec<String>>::new());
        let shown: Vec<_> = page.picker.shown().into_iter().map(|r| r.path).collect();
        assert_eq!(shown, ["/w/web"]);
    }

    #[test]
    fn escape_ends_the_search_and_clears_it() {
        let (mut page, _, mut runtime) = page();
        let keys = [KeyCode::Char('/'), KeyCode::Char('w'), KeyCode::Esc];
        press(&mut page, &mut runtime, &keys);
        assert_eq!((page.picker.query.as_str(), page.searching), ("", false));
    }

    #[test]
    fn q_ends_the_server_but_is_typed_while_searching() {
        let (mut page, calls, mut runtime) = page();
        press(&mut page, &mut runtime, &[KeyCode::Char('q')]);
        assert_eq!(calls.all(), [["kill-server"]]);
        let (mut page, calls, mut runtime) = self::page();
        press(
            &mut page,
            &mut runtime,
            &[KeyCode::Char('/'), KeyCode::Char('q')],
        );
        assert!(!calls.ran(&["kill-server"]));
        assert_eq!(page.picker.query, "q");
    }

    #[test]
    fn letters_outside_a_search_and_ctrl_keys_do_nothing() {
        let (mut page, calls, mut runtime) = page();
        press(&mut page, &mut runtime, &[KeyCode::Char('w')]);
        for c in ['c', 'q'] {
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
            page.handle(Msg::Key(key), NOW, &mut runtime);
        }
        assert_eq!(page.picker.query, "");
        // Never closes itself, nor its server, on Ctrl+C or Ctrl+Q
        assert_eq!(calls.all(), Vec::<Vec<String>>::new());
    }

    #[test]
    fn escape_outside_a_search_clears_the_query() {
        let (mut page, _, mut runtime) = page();
        let keys = [KeyCode::Char('/'), KeyCode::Char('w'), KeyCode::Enter];
        press(&mut page, &mut runtime, &keys);
        assert_eq!(page.picker.query, "w");
        press(&mut page, &mut runtime, &[KeyCode::Esc]);
        assert_eq!(page.picker.query, "");
    }

    #[test]
    fn a_key_typed_while_hidden_shows_the_page() {
        let (mut page, _, mut runtime) = page();
        page.hide(&mut runtime);
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(page.handle(Msg::Key(key), NOW, &mut runtime), Step::Draw);
        assert!(page.schedule.visible);
        assert_eq!(page.picker.cursor, 1);
    }

    #[test]
    fn a_page_sent_away_reads_nothing_and_never_wakes() {
        let (mut page, calls, mut runtime) = page();
        let dir = tempfile::TempDir::new().unwrap();
        let mut log = LogOnce::new("landing");
        assert!(page.watches.watch(&mut runtime, dir.path(), &mut log));
        page.schedule.labels = vec![0.0];
        page.schedule.dirty = true;
        let f11 = KeyEvent::new(KeyCode::F(11), KeyModifiers::NONE);
        assert_eq!(page.handle(Msg::Key(f11), NOW, &mut runtime), Step::Wait);
        assert_eq!(page.schedule.next_wake(NOW), None);
        assert!(page.watches.is_empty());
        assert_eq!(calls.all(), Vec::<Vec<String>>::new());
    }

    #[test]
    fn a_hidden_page_drops_changes_on_disk() {
        let (mut page, _, mut runtime) = page();
        page.hide(&mut runtime);
        let changed = Msg::Changed(PathBuf::from("/run/cst/s1.json"));
        assert_eq!(page.handle(changed, NOW, &mut runtime), Step::Wait);
        assert!(!page.schedule.visible && !page.schedule.dirty);
    }

    #[test]
    fn headless_runs_are_not_open_but_not_offered_either() {
        let record = |id: &str, headless| Record {
            id: id.to_owned(),
            headless,
            ..Record::default()
        };
        let (opened, live) = open_sessions(&[record("a", false), record("p", true)]);
        assert_eq!(opened, 1);
        assert_eq!(live, HashSet::from(["a".to_owned(), "p".to_owned()]));
    }

    #[test]
    fn labels_are_ages_resets_and_the_minute() {
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
        let got = labels(&folders(), &stats);
        assert_eq!(got, [0.0, NOW, NOW - 60.0, NOW - 9.0, NOW + 3600.0]);
    }
}
