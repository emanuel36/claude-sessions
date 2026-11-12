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

use crate::claude::hook;
use crate::claude::spend::ModelSpend;
use crate::claude::transcript::Titles;
use crate::core::state::{unix_now, Record};
use crate::term::tmux::{Cli, Runner};
use crate::ui::folders::Scan;
use crate::ui::landing;
use crate::ui::pane::{
    self, open, own_window, picker_key, App, Shell, Step, Visible, WINDOW_PAUSE,
};
use crate::ui::picker::{self, Folder, Picker, Root, RowKind};
use crate::ui::runtime::{signal, Msg, Runtime, Schedule, Screen, Signal};
use crate::ui::sidebar::Stats;
use crate::ui::usage::Reader;

/// The sessions found that the page shows at once.
const FOUND_SHOWN: usize = 10;

/// Runs the landing page in the tmux pane this process runs in, until its terminal goes away.
///
/// # Errors
///
/// Fails when the terminal or the event loop cannot be set up.
pub fn main_landing() -> anyhow::Result<()> {
    let tmux = Cli::default();
    let pane = std::env::var("TMUX_PANE").ok();
    // Out of sight in a window that is not current, as when `cst` restarts it after an
    // update; on screen when tmux cannot tell
    let current = own_window(&tmux, pane.as_deref(), WINDOW_PAUSE).map_or(true, |w| w.current);
    let mut runtime = Runtime::start().context("cannot start the event loop")?;
    let mut screen = Screen::enter().context("cannot set the terminal up")?;
    let mut app = Landing::new(Box::new(tmux), Picker::default());
    // Only this page shows the spend by model, which reads the transcripts
    app.shell.usage = Reader::new().split_models(ModelSpend::new());
    if current {
        app.show();
    }
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
    shell: Shell,
    /// The folder list and what the user did in it.
    picker: Picker,
    /// Whether the user is typing a search.
    searching: bool,
    /// How many sessions are open.
    opened: usize,
    /// The sessions running now, not offered to resume.
    live: HashSet<String>,
    scan: Scan,
    /// Reads the session records at a time; the tests give their own.
    records: Box<dyn Fn(f64) -> Vec<Record>>,
}

impl Landing {
    /// A page that runs tmux commands with `tmux`, its folders listed in `picker`; history and
    /// titles are kept on disk for the next process.
    fn new(tmux: Box<dyn Runner>, mut picker: Picker) -> Self {
        picker.found_shown = FOUND_SHOWN;
        Landing {
            tmux,
            shell: Shell::new("landing", true),
            picker,
            searching: false,
            opened: 0,
            live: HashSet::new(),
            scan: Scan::new(Titles::new(true)),
            records: Box::new(hook::live_sessions),
        }
    }

    /// Takes a message while on screen.
    fn take(&mut self, msg: Msg, now: f64) -> Step {
        match msg {
            Msg::Key(key) => match signal(&key) {
                // A window closed or opened: the open sessions may have changed
                Some(Signal::Layout) => {
                    self.shell.stale.records = true;
                    Step::Draw
                }
                Some(_) => Step::Wait,
                None if self.key(&key) => Step::Draw,
                None => Step::Wait,
            },
            Msg::Mouse(_) => Step::Wait,
            Msg::Resize => {
                self.shell.schedule.dirty = true;
                Step::Wait
            }
            Msg::Changed(path) => {
                // No transcript is watched: the page shows no session's status
                let touched = self.shell.watches.changed(&path, &[], now);
                if touched.any() {
                    self.shell.stale.add(touched);
                    self.shell.schedule.dirty = true;
                }
                Step::Wait
            }
        }
    }

    /// Takes a key the user typed; returns whether the page may have changed.
    ///
    /// Searching, Esc ends the search and clears it, and keys other than Enter go to the folder
    /// list. Enter opens a disk folder of Open another folder or a create row at once, as the
    /// Alt+n panel does; on a folder of New folder it picks the parent and the search goes on,
    /// its text cleared for the name; on an entry it opens or closes its tree and the search
    /// goes on; elsewhere it ends the search. Otherwise `/` starts a search, `q` ends the
    /// server, Esc clears the search, Backspace with nothing typed does in a tree what ← does,
    /// and the arrows and Enter go to the list, Enter opening a window.
    fn key(&mut self, key: &KeyEvent) -> bool {
        if self.searching {
            match key.code {
                KeyCode::Enter => self.enter_searching(),
                KeyCode::Esc => self.end_search(),
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
            self.end_search();
            return true;
        };
        match typed {
            picker::Key::Char('/') => self.searching = true,
            picker::Key::Char('q') => {
                self.tmux.run(&["kill-server"]);
            }
            // With nothing typed, Backspace in a tree does what Left does
            picker::Key::Backspace if self.picker.query.is_empty() && self.on_tree() => {
                self.picker.key(typed);
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

    /// Whether the selected row is an entry or a folder of its tree.
    fn on_tree(&mut self) -> bool {
        let row = self.picker.selected();
        row.is_some_and(|r| matches!(r.kind, RowKind::Root | RowKind::Dir))
    }

    /// Enter while searching: see [`Landing::key`].
    fn enter_searching(&mut self) {
        let row = self.picker.selected();
        match row.as_ref().map(|r| (r.kind, r.root)) {
            Some((RowKind::Root, _)) => {
                self.picker.key(picker::Key::Enter);
            }
            Some((RowKind::Dir, Some(Root::Open)) | (RowKind::Create | RowKind::Hit, _)) => {
                if let Some(row) = row {
                    open(self.tmux.as_ref(), &row, &self.picker.folders);
                }
                self.end_search();
            }
            Some((RowKind::Dir, Some(Root::New))) => {
                self.picker.key(picker::Key::Enter);
            }
            // The text stays, filtering the list; the walk read for it does not
            _ => {
                self.searching = false;
                self.picker.disk.forget_search();
            }
        }
    }

    /// Ends the search and clears it, with what it searched below and the walk it read.
    fn end_search(&mut self) {
        self.searching = false;
        self.picker.end_search();
        self.picker.disk.forget_search();
    }

    /// Reads again what is stale.
    fn reload(&mut self, now: f64) {
        let stale = std::mem::take(&mut self.shell.stale);
        if stale.records {
            let records = (self.records)(now);
            (self.opened, self.live) = open_sessions(&records);
        }
        let totals_due = self.shell.read_stats(&stale, now, true);
        if stale.records || totals_due {
            self.load_folders();
        }
        // The page lives for hours, and a session started or ended may have made folders;
        // the statistics' minute ticks keep the folders read. A search keeps its walk until it
        // ends, so typing on does not read the whole tree again; text left after it ended
        // reads its walk anew
        if stale.records || stale.history {
            self.picker.disk.forget_folders();
            if !self.searching {
                self.picker.disk.forget_search();
            }
        }
    }

    /// Reads the history's folders again.
    fn load_folders(&mut self) {
        // The cursor is an index: the rows may change under it, so it follows the row instead
        let identity = |r: &picker::Row| (r.kind, r.path.clone(), r.id.clone());
        let before = self.picker.selected().map(|r| identity(&r));
        let costs = self.shell.stats.totals.as_ref().map(|t| &t.per_session);
        let generation = self.shell.history.generation();
        self.picker.folders = self.scan.load(
            self.shell.history.sessions(),
            generation,
            costs.unwrap_or(&Default::default()),
            &self.live,
        );
        self.picker.sessions = self.scan.searchable(&self.live);
        if let Some(before) = before {
            let at = self
                .picker
                .shown()
                .iter()
                .position(|r| identity(r) == before);
            self.picker.cursor = at.unwrap_or(self.picker.cursor);
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
                &self.shell.stats,
                now,
                area.width,
                area.height,
            );
            frame.render_widget(Paragraph::new(lines), area);
        });
        self.shell.drawn(drawn, now);
    }
}

impl Visible for Landing {
    fn shell(&self) -> &Shell {
        &self.shell
    }

    fn shell_mut(&mut self) -> &mut Shell {
        &mut self.shell
    }
}

impl App for Landing {
    fn schedule(&self) -> &Schedule {
        &self.shell.schedule
    }

    fn handle(&mut self, msg: Msg, now: f64, runtime: &mut Runtime) -> Step {
        pane::gated(self, msg, runtime, |page, msg, _| page.take(msg, now))
    }

    fn update(&mut self, now: f64, runtime: &mut Runtime, screen: &mut Screen) {
        if !self.shell.prepare(now, runtime) {
            return;
        }
        self.reload(now);
        self.shell.schedule.labels = labels(&self.picker.folders, &self.shell.stats);
        self.draw(now, screen);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};
    use std::path::PathBuf;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::claude::history::HistoryReader;
    use crate::claude::statusline::Quota;
    use crate::claude::transcript::Titles;
    use crate::core::state::Record;
    use crate::term::tmux::fake::FakeTmux;
    use crate::ui::folders::Scan;
    use crate::ui::landing_app::{labels, open_sessions, Landing};
    use crate::ui::pane::{App, LogOnce, Step, Visible};
    use crate::ui::picker::{Folder, Picker, Root, RowKind, Session};
    use crate::ui::runtime::{Msg, Runtime};
    use crate::ui::search::Searchable;
    use crate::ui::sidebar::Stats;
    use crate::ui::usage::Reader;

    const NOW: f64 = 1_700_000_000.0;

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
    fn page() -> (Landing, FakeTmux, Runtime) {
        let calls = FakeTmux::silent();
        let mut picker = Picker::new(Box::new(|_| false));
        picker.folders = folders();
        let mut page = Landing::new(Box::new(calls.clone()), picker);
        page.shell.schedule.visible = true;
        (page, calls, Runtime::headless().unwrap())
    }

    fn press(page: &mut Landing, runtime: &mut Runtime, codes: &[KeyCode]) {
        for &code in codes {
            let key = KeyEvent::new(code, KeyModifiers::NONE);
            page.handle(Msg::Key(key), NOW, runtime);
        }
    }

    /// [`page`] with its trees in a tempdir home holding `projects/` and `music/`.
    fn home_page() -> (tempfile::TempDir, String, Landing, FakeTmux, Runtime) {
        let t = tempfile::tempdir().unwrap();
        for dir in ["projects", "music"] {
            std::fs::create_dir(t.path().join(dir)).unwrap();
        }
        let home = t.path().to_string_lossy().into_owned();
        let (mut page, calls, runtime) = page();
        page.picker.home = home.clone();
        (t, home, page, calls, runtime)
    }

    /// Starts a search and types `text`.
    fn search(page: &mut Landing, runtime: &mut Runtime, text: &str) {
        press(page, runtime, &[KeyCode::Char('/')]);
        let keys: Vec<KeyCode> = text.chars().map(KeyCode::Char).collect();
        press(page, runtime, &keys);
    }

    #[test]
    fn enter_while_searching_opens_a_disk_folder_at_once() {
        let (_t, home, mut page, calls, mut runtime) = home_page();
        press(&mut page, &mut runtime, &[KeyCode::Down, KeyCode::Enter]);
        search(&mut page, &mut runtime, "pro");
        assert_eq!(page.picker.selected().unwrap().kind, RowKind::Dir);
        press(&mut page, &mut runtime, &[KeyCode::Enter]);
        let dir = format!("{home}/projects");
        assert_eq!(
            calls.calls(),
            [["new-window", "-c", dir.as_str(), "claude"]]
        );
        assert_eq!((page.picker.query.as_str(), page.searching), ("", false));
        assert_eq!(page.picker.anchor, None);
    }

    #[test]
    fn enter_while_searching_on_a_create_row_creates_and_starts() {
        let (t, home, mut page, calls, mut runtime) = home_page();
        press(&mut page, &mut runtime, &[KeyCode::Down, KeyCode::Enter]);
        search(&mut page, &mut runtime, "notes");
        assert_eq!(page.picker.selected().unwrap().kind, RowKind::Create);
        press(&mut page, &mut runtime, &[KeyCode::Enter]);
        let dir = format!("{home}/notes");
        assert_eq!(
            calls.calls(),
            [["new-window", "-c", dir.as_str(), "claude"]]
        );
        assert!(t.path().join("notes").is_dir());
        assert!(!page.searching);
    }

    #[test]
    fn enter_on_a_new_folder_match_picks_the_parent_and_keeps_searching() {
        let (_t, home, mut page, calls, mut runtime) = home_page();
        press(&mut page, &mut runtime, &[KeyCode::Enter]);
        search(&mut page, &mut runtime, "pro");
        let picked = page.picker.selected().unwrap();
        assert_eq!((picked.kind, picked.root), (RowKind::Dir, Some(Root::New)));
        press(&mut page, &mut runtime, &[KeyCode::Enter]);
        assert!(page.searching);
        assert_eq!(page.picker.query, "");
        let parent = format!("{home}/projects");
        assert!(page.picker.dirs.contains(&(Root::New, parent)));
        assert_eq!(calls.calls(), Vec::<Vec<String>>::new());
    }

    /// `n` sessions of the folder `path` about LinkedIn.
    fn linkedin_sessions(n: u32, path: &str) -> Vec<Searchable> {
        (0..n)
            .map(|i| {
                let (id, title) = (format!("h{i}"), format!("LinkedIn {i}"));
                let last = NOW - f64::from(i) * 60.0;
                Searchable::new(id, title, path.into(), last, false, true, &[])
            })
            .collect()
    }

    #[test]
    fn enter_while_searching_on_a_hit_resumes_at_once() {
        let (mut page, calls, mut runtime) = page();
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("deleted").to_string_lossy().into_owned();
        page.picker.sessions = linkedin_sessions(1, &dir);
        search(&mut page, &mut runtime, "linkedin");
        assert_eq!(page.picker.selected().unwrap().kind, RowKind::Hit);
        press(&mut page, &mut runtime, &[KeyCode::Enter]);
        assert_eq!(
            calls.calls(),
            [["new-window", "-c", dir.as_str(), "claude --resume h0"]]
        );
        assert!(t.path().join("deleted").is_dir());
        assert_eq!((page.picker.query.as_str(), page.searching), ("", false));
    }

    #[test]
    fn the_landing_shows_ten_hits() {
        let (mut page, _, mut runtime) = page();
        page.picker.sessions = linkedin_sessions(12, "/w/cv");
        search(&mut page, &mut runtime, "linkedin");
        let shown = page.picker.shown();
        assert_eq!(shown.iter().filter(|r| r.kind == RowKind::Hit).count(), 10);
    }

    #[test]
    fn a_reload_fills_the_sessions_the_search_looks_in() {
        let (t, _, mut page, _, _) = home_page();
        let (config, state) = (t.path().join("config"), t.path().join("state"));
        std::fs::create_dir_all(&config).unwrap();
        let project = t.path().join("projects");
        let line = |at: u64, text: &str| {
            serde_json::json!({
                "sessionId": "s9", "project": project, "timestamp": at, "display": text
            })
        };
        let text = format!(
            "{}\n{}\n",
            line(1000, "first"),
            line(2000, "about LinkedIn")
        );
        std::fs::write(config.join("history.jsonl"), text).unwrap();
        page.scan = Scan::in_dir(Titles::new(false), config.clone());
        page.shell.history = HistoryReader::with_dirs(false, config, state);
        page.shell.usage = Reader::in_dir(t.path().join("journal"));
        page.records = Box::new(|_| Vec::new());
        page.shell.stale.history = true;
        page.reload(NOW);
        let hits = crate::ui::search::hits(&page.picker.sessions, "linkedin");
        assert_eq!(hits.len(), 1);
        assert_eq!(page.picker.sessions[hits[0].index].id, "s9");
    }

    #[test]
    fn a_reload_keeps_the_selected_hit() {
        let (t, _, mut page, _, mut runtime) = home_page();
        let (config, state) = (t.path().join("config"), t.path().join("state"));
        std::fs::create_dir_all(&config).unwrap();
        let project = t.path().join("projects");
        page.scan = Scan::in_dir(Titles::new(false), config.clone());
        page.shell.history = HistoryReader::with_dirs(false, config.clone(), state);
        page.shell.usage = Reader::in_dir(t.path().join("journal"));
        page.records = Box::new(|_| Vec::new());
        let write = |ids: &[(&str, u64)]| {
            let lines: Vec<String> = ids
                .iter()
                .map(|(id, at)| {
                    serde_json::json!({
                        "sessionId": id, "project": project, "timestamp": at,
                        "display": format!("LinkedIn {id}")
                    })
                    .to_string()
                })
                .collect();
            std::fs::write(config.join("history.jsonl"), lines.join("\n") + "\n").unwrap();
        };
        write(&[("s1", 1000), ("s2", 2000)]);
        page.shell.stale.history = true;
        page.reload(NOW);
        search(&mut page, &mut runtime, "linkedin");
        while page.picker.selected().map(|r| r.id) != Some("s1".into()) {
            press(&mut page, &mut runtime, &[KeyCode::Down]);
        }
        write(&[("s1", 1000), ("s2", 2000), ("s3", 3000)]);
        page.shell.stale.history = true;
        page.reload(NOW);
        assert_eq!(page.picker.selected().unwrap().id, "s1");
    }

    #[test]
    fn only_a_records_or_history_reload_drops_the_folder_cache() {
        let (t, _, mut page, _, mut runtime) = home_page();
        // History, journal, records and config of its own: nothing outside the tempdir is read
        let (config, state) = (t.path().join("config"), t.path().join("state"));
        page.scan = Scan::in_dir(Titles::new(false), config.clone());
        page.shell.history = HistoryReader::with_dirs(false, config, state);
        page.shell.usage = Reader::in_dir(t.path().join("journal"));
        page.records = Box::new(|_| Vec::new());
        press(&mut page, &mut runtime, &[KeyCode::Down, KeyCode::Enter]);
        let listed = |page: &mut Landing, name: &str| {
            let shown = page.picker.shown();
            shown
                .iter()
                .any(|r| r.kind == RowKind::Dir && r.label == format!("{name}/"))
        };
        assert!(!listed(&mut page, "later"));
        std::fs::create_dir(t.path().join("later")).unwrap();
        assert!(!listed(&mut page, "later"), "read once and kept");
        // The statistics' minute ticks: the folders read are kept
        page.reload(NOW);
        page.reload(NOW + 60.0);
        assert!(!listed(&mut page, "later"));
        page.shell.stale.history = true;
        page.reload(NOW + 61.0);
        assert!(listed(&mut page, "later"));
        std::fs::create_dir(t.path().join("after")).unwrap();
        page.shell.stale.records = true;
        page.reload(NOW + 62.0);
        assert!(listed(&mut page, "after"));
    }

    #[test]
    fn a_reload_during_a_search_keeps_the_walk_until_the_search_ends() {
        let (t, _, mut page, _, mut runtime) = home_page();
        let (config, state) = (t.path().join("config"), t.path().join("state"));
        page.scan = Scan::in_dir(Titles::new(false), config.clone());
        page.shell.history = HistoryReader::with_dirs(false, config, state);
        page.shell.usage = Reader::in_dir(t.path().join("journal"));
        page.records = Box::new(|_| Vec::new());
        press(&mut page, &mut runtime, &[KeyCode::Down, KeyCode::Enter]);
        let found = |page: &mut Landing| -> Vec<String> {
            let shown = page.picker.shown();
            let hits = shown.into_iter().filter(|r| r.kind == RowKind::Dir);
            hits.map(|r| r.label).collect()
        };
        search(&mut page, &mut runtime, "pro");
        assert_eq!(found(&mut page), ["projects"]);
        std::fs::create_dir(t.path().join("prolog")).unwrap();
        page.shell.stale.history = true;
        page.reload(NOW);
        assert_eq!(found(&mut page), ["projects"], "the walk is kept");
        press(&mut page, &mut runtime, &[KeyCode::Esc]);
        page.picker.cursor = 1;
        search(&mut page, &mut runtime, "pro");
        let mut hits = found(&mut page);
        hits.sort();
        assert_eq!(
            hits,
            ["projects", "prolog"],
            "read anew once the search ended"
        );
    }

    #[test]
    fn a_search_ended_on_a_history_row_keeps_no_walk_past_a_reload() {
        let (t, _, mut page, _, mut runtime) = home_page();
        let (config, state) = (t.path().join("config"), t.path().join("state"));
        page.scan = Scan::in_dir(Titles::new(false), config.clone());
        page.shell.history = HistoryReader::with_dirs(false, config, state);
        page.shell.usage = Reader::in_dir(t.path().join("journal"));
        page.records = Box::new(|_| Vec::new());
        press(&mut page, &mut runtime, &[KeyCode::Down, KeyCode::Enter]);
        search(&mut page, &mut runtime, "w");
        while page.picker.selected().unwrap().kind != RowKind::Folder {
            press(&mut page, &mut runtime, &[KeyCode::Down]);
        }
        press(&mut page, &mut runtime, &[KeyCode::Enter]);
        assert!(!page.searching);
        let listed = |page: &mut Landing| {
            let shown = page.picker.shown();
            shown
                .iter()
                .any(|r| r.kind == RowKind::Dir && r.label == "wiki")
        };
        assert!(!listed(&mut page));
        std::fs::create_dir(t.path().join("wiki")).unwrap();
        page.shell.stale.history = true;
        page.reload(NOW);
        assert!(listed(&mut page), "the ended search's walk is read anew");
    }

    #[test]
    fn escape_while_searching_forgets_where_the_search_started() {
        let (_t, _, mut page, _, mut runtime) = home_page();
        press(&mut page, &mut runtime, &[KeyCode::Enter]);
        search(&mut page, &mut runtime, "pro");
        assert!(page.picker.anchor.is_some());
        press(&mut page, &mut runtime, &[KeyCode::Esc]);
        assert_eq!(page.picker.anchor, None);
        page.picker.cursor = 4;
        assert_eq!(page.picker.selected().unwrap().kind, RowKind::Folder);
        let keys = [KeyCode::Char('/'), KeyCode::Backspace];
        press(&mut page, &mut runtime, &keys);
        assert_eq!(page.picker.cursor, 4);
    }

    #[test]
    fn backspace_outside_a_search_closes_like_left() {
        let (_t, _, mut page, _, mut runtime) = home_page();
        press(&mut page, &mut runtime, &[KeyCode::Enter]);
        assert!(page.picker.open.contains(&Root::New));
        press(&mut page, &mut runtime, &[KeyCode::Down, KeyCode::Right]);
        assert_eq!(page.picker.dirs.len(), 1);
        // On a tree folder, then on the entry
        press(&mut page, &mut runtime, &[KeyCode::Backspace]);
        assert!(page.picker.dirs.is_empty());
        press(&mut page, &mut runtime, &[KeyCode::Up, KeyCode::Backspace]);
        assert!(!page.picker.open.contains(&Root::New));
    }

    #[test]
    fn enter_on_an_entry_while_searching_toggles_it() {
        let (_t, _, mut page, calls, mut runtime) = home_page();
        press(
            &mut page,
            &mut runtime,
            &[KeyCode::Char('/'), KeyCode::Enter],
        );
        assert!(page.picker.open.contains(&Root::New));
        assert!(page.searching);
        press(&mut page, &mut runtime, &[KeyCode::Enter]);
        assert!(!page.picker.open.contains(&Root::New));
        assert!(page.searching);
        assert_eq!(calls.calls(), Vec::<Vec<String>>::new());
    }

    #[test]
    fn enter_on_a_folder_starts_a_session_there() {
        let (mut page, calls, mut runtime) = page();
        let keys = [KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter];
        press(&mut page, &mut runtime, &keys);
        assert_eq!(calls.calls(), [["new-window", "-c", "/w/web", "claude"]]);
    }

    #[test]
    fn right_opens_a_folders_sessions_and_enter_resumes_one() {
        let (mut page, calls, mut runtime) = page();
        let keys = [
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Right,
            KeyCode::Down,
            KeyCode::Enter,
        ];
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
        assert_eq!(calls.calls(), Vec::<Vec<String>>::new());
        let shown = page.picker.shown().into_iter();
        let history: Vec<_> = shown.filter(|r| r.root.is_none()).map(|r| r.path).collect();
        assert_eq!(history, ["/w/web"]);
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
        assert_eq!(calls.calls(), [["kill-server"]]);
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
        assert_eq!(calls.calls(), Vec::<Vec<String>>::new());
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
        assert!(page.shell.schedule.visible);
        assert_eq!(page.picker.cursor, 1);
    }

    #[test]
    fn a_page_sent_away_reads_nothing_and_never_wakes() {
        let (mut page, calls, mut runtime) = page();
        let dir = tempfile::TempDir::new().unwrap();
        let mut log = LogOnce::new("landing");
        assert!(page.shell.watches.watch(&mut runtime, dir.path(), &mut log));
        page.shell.schedule.labels = vec![0.0];
        page.shell.schedule.dirty = true;
        let f11 = KeyEvent::new(KeyCode::F(11), KeyModifiers::NONE);
        assert_eq!(page.handle(Msg::Key(f11), NOW, &mut runtime), Step::Wait);
        assert_eq!(page.shell.schedule.next_wake(NOW), None);
        assert!(page.shell.watches.is_empty());
        assert_eq!(calls.calls(), Vec::<Vec<String>>::new());
    }

    #[test]
    fn a_hidden_page_drops_changes_on_disk() {
        let (mut page, _, mut runtime) = page();
        page.hide(&mut runtime);
        let changed = Msg::Changed(PathBuf::from("/run/cst/s1.json"));
        assert_eq!(page.handle(changed, NOW, &mut runtime), Step::Wait);
        assert!(!page.shell.schedule.visible && !page.shell.schedule.dirty);
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
