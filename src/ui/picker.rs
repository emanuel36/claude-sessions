//! The folder list that starts a session: folders by last use, each opening its last sessions.
//!
//! The Alt+n panel shows it under a search line; the landing page shows it under its header.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::rc::Rc;

use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};
use serde::Deserialize;

use crate::core::paths;
use crate::shell;
use crate::ui::line::{cell_len, cells, ljust, plain, push, rjust, styled, truncate, width_of};
use crate::ui::tree::{self, Disk};
use crate::ui::words::{
    self, CLAUDE, FAINT, GREEN, INACTIVE, INK, PERMISSION, RULE, SOFT, STRONG, SUBTLE,
};

/// How many of a folder's sessions open under it at once; the rest scroll into view.
pub const SHOWN: usize = 5;
/// The narrowest the list draws well: the landing page keeps this much beside its statistics,
/// and the golden panels draw at it.
pub const WIDTH: usize = 48;
/// The cells a session's age takes.
const AGE_WIDTH: usize = 4;
/// The cells a session's cost takes.
const SPEND_WIDTH: usize = 8;
/// Marks the last line on show when the list goes on below it.
const MORE: char = '▼';

/// An earlier session of a folder, to resume.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Session {
    /// The session id.
    pub id: String,
    /// Its title, or its first prompt.
    pub title: String,
    /// When it was last used, in seconds since the epoch.
    pub last: f64,
    /// What it cost over the last 7 days, if anything.
    #[serde(default)]
    pub cost: Option<f64>,
}

/// A folder Claude ran in: when it was last used, its sessions and its week's spend.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Folder {
    /// The folder's path, with no trailing slash.
    pub path: String,
    /// Its last path component, or the path itself for `/`.
    pub name: String,
    /// When a session last used it, in seconds since the epoch.
    pub last: f64,
    /// How many sessions ran in it, the open ones included.
    #[serde(default)]
    pub count: u32,
    /// What its sessions cost over the last 7 days.
    #[serde(default)]
    pub spend: f64,
    /// Its sessions that are not open now, newest first.
    #[serde(default)]
    pub sessions: Vec<Session>,
}

/// An entry to the disk, at the top of the list; each opens its own tree of the home folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Root {
    /// Picks the parent of a folder to create.
    New,
    /// Opens a folder Claude never ran in.
    Open,
}

/// The entries to the disk, in list order.
const ROOTS: [Root; 2] = [Root::New, Root::Open];

/// By folder, when a Claude session last ran in or below it.
type Recent = HashMap<String, f64>;

/// What a [`Recent`] was read from: how many history folders, and the bits of the newest one's
/// last use.
type History = (usize, u64);

/// What a line of the list is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowKind {
    /// A typed folder path, to start a session in.
    Path,
    /// A folder Claude ran in.
    #[default]
    Folder,
    /// One of a folder's sessions.
    Session,
    /// An entry to the disk, opening its folders as a tree.
    Root,
    /// A folder on disk, in an entry's tree.
    Dir,
    /// A folder to create, then start a session in.
    Create,
}

/// A line of the list: an entry to the disk, a disk folder, a typed path, a folder, or one of a
/// folder's sessions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Row {
    /// What the row is.
    pub kind: RowKind,
    /// The typed path, or the folder's path.
    pub path: String,
    /// The folder's index in the folders, for folder and session rows.
    pub folder: Option<usize>,
    /// The session's index in its folder's sessions, for session rows.
    pub session: Option<usize>,
    /// Whether a session is the last of its folder on show, drawn with the closing branch.
    pub end: bool,
    /// For entry and disk rows: the entry they belong to.
    pub root: Option<Root>,
    /// For a disk folder: the tree lines before its name; for an entry: its hint.
    pub branch: String,
    /// What an entry or disk row shows: the entry's name, a folder's name with a `/`, or the
    /// path a create row makes, the home folder shown as `~`.
    pub label: String,
    /// The byte offsets in `label` of the characters a search matched.
    pub hits: Vec<usize>,
}

/// A key the list takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Moves up, or scrolls a folder's sessions back.
    Up,
    /// Moves down, or scrolls a folder's sessions on.
    Down,
    /// Closes a folder, from it or from one of its sessions; in a tree, closes an open folder
    /// or entry, else moves to the folder or entry holding the row.
    Left,
    /// Opens a folder's sessions.
    Right,
    /// Opens the selected row.
    Enter,
    /// Deletes the search's last character.
    Backspace,
    /// Types a character into the search.
    Char(char),
}

/// What the user did in the list: the search, the selected row and the open folders.
pub struct Picker {
    /// The folders, last used first.
    pub folders: Vec<Folder>,
    /// What the user typed.
    pub query: String,
    /// The index of the selected row among those shown.
    pub cursor: usize,
    /// The paths of the folders showing their sessions.
    pub expanded: BTreeSet<String>,
    /// By folder path, how many of its sessions are scrolled past.
    pub scroll: HashMap<String, usize>,
    /// Tells whether a typed path (with `~` expanded) is a folder.
    pub is_dir: Box<dyn Fn(&str) -> bool>,
    /// The home folder, where the trees start.
    pub home: String,
    /// The folders read from disk.
    pub disk: Disk,
    /// The entries showing their tree.
    pub open: BTreeSet<Root>,
    /// The tree folders showing their subfolders, by entry.
    pub dirs: BTreeSet<(Root, String)>,
    /// While typing in a tree: the entry and the folder searched below.
    pub anchor: Option<(Root, String)>,
    /// Where the cursor was when the first character was typed in a tree, to go back to once
    /// the text is cleared.
    typed_at: usize,
    /// By folder, when a Claude session last ran in or below it, with the history it was read
    /// from.
    recent: RefCell<(Option<History>, Rc<Recent>)>,
}

impl Default for Picker {
    /// An empty picker that looks on disk, `~` expanded, for typed folders.
    fn default() -> Self {
        Self::new(Box::new(|path| Path::new(&expand_user(path)).is_dir()))
    }
}

impl Picker {
    /// An empty picker that asks `is_dir` whether a typed path is a folder.
    pub fn new(is_dir: Box<dyn Fn(&str) -> bool>) -> Self {
        Self {
            folders: Vec::new(),
            query: String::new(),
            cursor: 0,
            expanded: BTreeSet::new(),
            scroll: HashMap::new(),
            is_dir,
            home: paths::home(&paths::process_env)
                .to_string_lossy()
                .into_owned(),
            disk: Disk::default(),
            open: BTreeSet::new(),
            dirs: BTreeSet::new(),
            anchor: None,
            typed_at: 0,
            recent: RefCell::default(),
        }
    }

    /// The folder searched below under `root`, while the text typed in its tree is not blank.
    fn searched(&self, root: Root) -> Option<&str> {
        let (anchored, dir) = self.anchor.as_ref()?;
        (*anchored == root && !self.query.trim().is_empty()).then_some(dir.as_str())
    }

    /// An entry's row, then its tree when open, or the search below one of its folders.
    fn root_rows(&self, root: Root) -> Vec<Row> {
        let open = self.open.contains(&root);
        let searched = self.searched(root);
        // An entry's `branch` carries its hint
        let hint = match (searched, open, root) {
            (_, false, _) => String::new(),
            (Some(dir), true, _) => format!("in {}/", tilde(dir, &self.home)),
            (None, true, Root::New) => "pick the parent, then type the name".to_owned(),
            (None, true, Root::Open) => "type to search below".to_owned(),
        };
        let mut out = vec![Row {
            kind: RowKind::Root,
            path: self.home.clone(),
            root: Some(root),
            branch: hint,
            label: match root {
                Root::New => "New folder…",
                Root::Open => "Open another folder…",
            }
            .to_owned(),
            ..Row::default()
        }];
        match searched {
            _ if !open => {}
            Some(dir) => self.search_rows(root, dir, &mut out),
            None => self.tree_rows(root, &self.home, "", &mut out),
        }
        out
    }

    /// The folders below `dir` whose name holds the typed text, and the row creating a folder
    /// of that name while nothing of exactly that path is on disk; text holding a `/` only offers
    /// the create row. Text starting with `~` or `/`, or with a `.` or `..` part, names no
    /// folder below `dir` and offers no create row. Under the new-folder entry the create row
    /// comes first, else last.
    ///
    /// # Arguments
    /// * `root`: the entry searched in.
    /// * `dir`: the folder searched below.
    /// * `out`: where the rows go.
    fn search_rows(&self, root: Root, dir: &str, out: &mut Vec<Row>) {
        let text = self.query.trim();
        let hits = if text.contains('/') {
            Vec::new()
        } else {
            let recent = self.recent();
            self.disk.search(dir, text, &|path| {
                self.disk.usage(path, recent.get(path).copied())
            })
        };
        // Only a name relative to `dir` stays below it: no `~`, `/`, `.` or `..` lead out
        let relative =
            !text.starts_with(['~', '/']) && text.split('/').all(|c| c != "." && c != "..");
        // A trailing `/` names the same folder
        let name = text.trim_end_matches('/');
        // Anything there, hidden or left out of the tree, a file or a link, is never created over
        // (an error other than not found, as a file in the path, counts as something there)
        let free = || {
            let there = std::fs::symlink_metadata(tree::join(dir, name));
            there.is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        };
        let create = (relative && !name.is_empty() && free()).then(|| Row {
            kind: RowKind::Create,
            path: tree::join(dir, name),
            root: Some(root),
            label: tilde(&tree::join(dir, name), &self.home),
            ..Row::default()
        });
        let create_last = root == Root::Open && create.is_some();
        if root == Root::New {
            out.extend(create.clone());
        }
        let count = hits.len();
        out.extend(hits.into_iter().enumerate().map(|(i, hit)| {
            let last = i + 1 == count && !create_last;
            Row {
                kind: RowKind::Dir,
                path: tree::join(dir, &hit.rel),
                root: Some(root),
                branch: if last { "└ " } else { "├ " }.to_owned(),
                label: hit.rel,
                hits: hit.hits,
                ..Row::default()
            }
        }));
        if root == Root::Open {
            out.extend(create);
        }
    }

    /// The subfolders of `dir` in an entry's tree, last used first, each followed by its own
    /// when open.
    ///
    /// # Arguments
    /// * `root`: the entry the tree belongs to.
    /// * `dir`: the folder whose subfolders to list.
    /// * `prefix`: the tree lines of the levels above.
    /// * `out`: where the rows go.
    fn tree_rows(&self, root: Root, dir: &str, prefix: &str, out: &mut Vec<Row>) {
        let recent = self.recent();
        let mut names = self.disk.children(dir);
        names.sort_by_cached_key(|name| {
            let path = tree::join(dir, name);
            self.disk.usage(&path, recent.get(&path).copied())
        });
        let count = names.len();
        for (i, name) in names.into_iter().enumerate() {
            let last = i + 1 == count;
            let path = tree::join(dir, &name);
            out.push(Row {
                kind: RowKind::Dir,
                path: path.clone(),
                root: Some(root),
                branch: format!("{prefix}{}", if last { "└ " } else { "├ " }),
                label: format!("{name}/"),
                ..Row::default()
            });
            if self.dirs.contains(&(root, path.clone())) {
                let deeper = format!("{prefix}{}", if last { "  " } else { "│ " });
                self.tree_rows(root, &path, &deeper, out);
            }
        }
    }

    /// When a Claude session last ran in or below each folder, read again only when the
    /// history's folder count or its newest session changes.
    fn recent(&self) -> Rc<Recent> {
        let newest = self.folders.iter().map(|f| f.last).fold(f64::MIN, f64::max);
        let key = Some((self.folders.len(), newest.to_bits()));
        let mut cached = self.recent.borrow_mut();
        if cached.0 != key {
            let folders = self.folders.iter().map(|f| (f.path.as_str(), f.last));
            *cached = (key, Rc::new(tree::sessions_below(folders)));
        }
        Rc::clone(&cached.1)
    }

    /// The entry and the folder that text typed on `row` searches below: the folder under the
    /// cursor if it is open, else the one holding it (the home folder on an open entry).
    /// `None` on a history row or a closed entry: the text then only filters the history.
    pub fn anchor_of(&self, row: &Row) -> Option<(Root, String)> {
        let root = row.root?;
        let dir = match row.kind {
            RowKind::Root if self.open.contains(&root) => self.home.clone(),
            RowKind::Dir if self.dirs.contains(&(root, row.path.clone())) => row.path.clone(),
            RowKind::Dir => Path::new(&row.path)
                .parent()
                .map_or_else(|| self.home.clone(), |p| p.to_string_lossy().into_owned()),
            _ => return None,
        };
        Some((root, dir))
    }

    /// Puts the cursor on the first row on show that `pick` accepts, if any.
    fn select(&mut self, pick: impl Fn(&Row) -> bool) {
        if let Some(i) = self.shown().iter().position(pick) {
            self.cursor = i;
        }
    }

    /// Opens an entry and every folder between the home folder and `path`, so its tree shows
    /// `path`.
    fn reveal(&mut self, root: Root, path: &str) {
        self.open.insert(root);
        let home = Path::new(&self.home);
        let mut at = Path::new(path).parent();
        while let Some(dir) = at.filter(|d| d.starts_with(home) && *d != home) {
            self.dirs.insert((root, dir.to_string_lossy().into_owned()));
            at = dir.parent();
        }
    }

    /// Left in a tree: closes an open folder, else moves to the folder holding it, or to the
    /// entry; on an entry, closes its tree and drops the folders read, so it opens anew.
    fn close(&mut self, root: Root, row: &Row) {
        if row.kind == RowKind::Root {
            self.open.remove(&root);
            self.disk.clear();
            return;
        }
        if self.dirs.remove(&(root, row.path.clone())) {
            return;
        }
        let parent = Path::new(&row.path).parent().unwrap_or(Path::new(""));
        if parent == Path::new(&self.home) {
            self.select(|r| r.kind == RowKind::Root && r.root == Some(root));
        } else {
            let parent = parent.to_string_lossy().into_owned();
            self.select(|r| r.kind == RowKind::Dir && r.root == Some(root) && r.path == parent);
        }
    }

    /// The rows on show: the entries, each followed by its tree when open, then the history.
    /// The cursor is kept on one of them.
    pub fn shown(&mut self) -> Vec<Row> {
        let mut out: Vec<Row> = ROOTS.iter().flat_map(|&r| self.root_rows(r)).collect();
        out.extend(rows(
            &self.folders,
            &self.query,
            &self.expanded,
            self.is_dir.as_ref(),
            &self.scroll,
        ));
        self.cursor = self.cursor.min(out.len().saturating_sub(1));
        out
    }

    /// The selected row, or `None` when nothing is listed.
    pub fn selected(&mut self) -> Option<Row> {
        let shown = self.shown();
        shown.into_iter().nth(self.cursor)
    }

    /// Applies a key; returns the row to open on Enter, else `None`.
    pub fn key(&mut self, key: Key) -> Option<Row> {
        let row = self.selected();
        let kind = row.as_ref().map(|r| r.kind);
        let root = row.as_ref().and_then(|r| r.root);
        match (key, root) {
            (Key::Enter, Some(root)) if kind == Some(RowKind::Root) => {
                if self.open.remove(&root) {
                    self.disk.clear();
                } else {
                    self.open.insert(root);
                }
            }
            // In the new-folder tree, Enter on a folder picks it as the parent: it opens there,
            // the cursor on it, ready for the name
            (Key::Enter, Some(Root::New)) if kind == Some(RowKind::Dir) => {
                if let Some(row) = row {
                    self.end_search();
                    self.reveal(Root::New, &row.path);
                    self.dirs.insert((Root::New, row.path.clone()));
                    self.select_dir(Root::New, &row.path);
                }
            }
            (Key::Right, Some(root)) if kind == Some(RowKind::Root) => {
                self.open.insert(root);
            }
            // On a match, opens the tree down to it and the match itself, and leaves the search
            (Key::Right, Some(root))
                if kind == Some(RowKind::Dir) && self.searched(root).is_some() =>
            {
                if let Some(row) = row {
                    self.end_search();
                    self.reveal(root, &row.path);
                    self.dirs.insert((root, row.path.clone()));
                    self.select_dir(root, &row.path);
                }
            }
            (Key::Right, Some(root)) if kind == Some(RowKind::Dir) => {
                if let Some(row) = row {
                    self.dirs.insert((root, row.path));
                }
            }
            (Key::Left, Some(root)) | (Key::Backspace, Some(root))
                if self.query.is_empty() && matches!(kind, Some(RowKind::Root | RowKind::Dir)) =>
            {
                if let Some(row) = row {
                    self.close(root, &row);
                }
            }
            _ => return self.history_key(key, row),
        }
        self.shown();
        None
    }

    /// Applies a key the entries and trees leave to the history rows; returns the row to open
    /// on Enter, else `None`.
    fn history_key(&mut self, key: Key, row: Option<Row>) -> Option<Row> {
        match key {
            Key::Enter => return row,
            Key::Up | Key::Down => {
                if !self.scrolls(row.as_ref(), key) {
                    self.cursor = if key == Key::Down {
                        self.cursor + 1
                    } else {
                        self.cursor.saturating_sub(1)
                    };
                }
            }
            Key::Right => {
                if let Some(row) = row.filter(|r| r.kind == RowKind::Folder) {
                    self.expanded.insert(row.path);
                }
            }
            Key::Left => {
                // From a session, back to its folder, which then closes
                let history = |r: &Row| matches!(r.kind, RowKind::Folder | RowKind::Session);
                if let Some(row) = row.filter(history) {
                    self.expanded.remove(&row.path);
                    self.scroll.remove(&row.path);
                    let shown = self.shown();
                    if let Some(i) = shown
                        .iter()
                        .position(|r| r.kind == RowKind::Folder && r.path == row.path)
                    {
                        self.cursor = i;
                    }
                }
            }
            Key::Backspace | Key::Char(_) => self.typed(key, row.as_ref()),
        }
        self.shown();
        None
    }

    /// Types a character into the search, or deletes its last one.
    ///
    /// The first character decides what the text searches, from the row under the cursor
    /// ([`Picker::anchor_of`]); clearing the text returns to the tree as it was.
    fn typed(&mut self, key: Key, row: Option<&Row>) {
        if let Key::Char(c) = key {
            if self.query.is_empty() {
                self.anchor = row.and_then(|r| self.anchor_of(r));
                self.typed_at = self.cursor;
            }
            self.query.push(c);
        } else {
            self.query.pop();
            if self.query.is_empty() && self.anchor.take().is_some() {
                self.cursor = self.typed_at;
                return;
            }
        }
        self.after_typing();
    }

    /// The cursor after a key typed into the search: on the typed path's row when listed, else
    /// as [`Picker::tree_cursor`] puts it in a searched tree; with no tree searched, on the first
    /// history row.
    fn after_typing(&mut self) {
        let shown = self.shown();
        // A typed path that exists is what the text names, wherever it was typed
        let at = shown
            .iter()
            .position(|r| r.kind == RowKind::Path)
            .or_else(|| match self.anchor.as_ref().map(|(root, _)| *root) {
                None => shown.iter().position(|r| r.root.is_none()),
                Some(root) => self.tree_cursor(root, &shown),
            });
        if let Some(i) = at {
            self.cursor = i;
        }
    }

    /// Where the cursor goes in `root`'s search: on a match one level down whose name starts
    /// with the text (ignoring case), else on the first row below the entry (the create row
    /// under the new-folder entry, the first match under the other), else on the entry. `None`
    /// while the text is blank.
    fn tree_cursor(&self, root: Root, shown: &[Row]) -> Option<usize> {
        self.searched(root)?;
        let text = self.query.trim().to_lowercase();
        let mine = |r: &Row| r.root == Some(root) && r.kind != RowKind::Root;
        // A match's label is its path below the folder searched: one level down holds no `/`
        let near = |r: &Row| {
            mine(r)
                && r.kind == RowKind::Dir
                && !r.label.contains('/')
                && r.label.to_lowercase().starts_with(&text)
        };
        let entry = |r: &Row| r.kind == RowKind::Root && r.root == Some(root);
        shown
            .iter()
            .position(near)
            .or_else(|| shown.iter().position(mine))
            .or_else(|| shown.iter().position(entry))
    }

    /// Leaves the search: the text and what it searched below are cleared.
    pub fn end_search(&mut self) {
        self.query.clear();
        self.anchor = None;
    }

    /// Puts the cursor on the folder `path` of an entry's tree.
    fn select_dir(&mut self, root: Root, path: &str) {
        self.select(|r| r.kind == RowKind::Dir && r.root == Some(root) && r.path == path);
    }

    /// Scrolls a folder's sessions when the cursor would leave them with more that way.
    ///
    /// Returns whether it scrolled, in which case the cursor stays on its line.
    fn scrolls(&mut self, row: Option<&Row>, key: Key) -> bool {
        let Some(row) = row.filter(|r| r.kind == RowKind::Session) else {
            return false;
        };
        let (Some(folder), Some(session)) = (row.folder, row.session) else {
            return false;
        };
        let count = self.folders.get(folder).map_or(0, |f| f.sessions.len());
        let start = self.scroll.get(&row.path).copied().unwrap_or(0);
        if key == Key::Down && row.end && start + SHOWN < count {
            self.scroll.insert(row.path.clone(), start + 1);
            return true;
        }
        if key == Key::Up && start > 0 && session == start {
            self.scroll.insert(row.path.clone(), start - 1);
            return true;
        }
        false
    }
}

/// Tells whether a folder's path holds every word of the query, ignoring case.
fn matches(folder: &Folder, query: &str) -> bool {
    let path = folder.path.to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| path.contains(word))
}

/// The list's rows: a typed path first, then the folders that match, with sessions below.
///
/// # Arguments
/// * `folders`: the folders, last used first.
/// * `query`: what the user typed.
/// * `expanded`: the paths of the folders showing their sessions.
/// * `is_dir`: tells whether a typed path (with `~` expanded) is a folder.
/// * `scroll`: by folder path, how many of its sessions are scrolled past.
pub fn rows(
    folders: &[Folder],
    query: &str,
    expanded: &BTreeSet<String>,
    is_dir: &dyn Fn(&str) -> bool,
    scroll: &HashMap<String, usize>,
) -> Vec<Row> {
    let mut out = Vec::new();
    let typed = query.trim();
    if (typed.starts_with('/') || typed.starts_with('~')) && is_dir(typed) {
        out.push(Row {
            kind: RowKind::Path,
            path: typed.to_owned(),
            folder: None,
            session: None,
            end: false,
            ..Row::default()
        });
    }
    for (index, folder) in folders.iter().enumerate() {
        if !matches(folder, query) {
            continue;
        }
        out.push(Row {
            kind: RowKind::Folder,
            path: folder.path.clone(),
            folder: Some(index),
            session: None,
            end: false,
            ..Row::default()
        });
        if expanded.contains(&folder.path) {
            let count = folder.sessions.len();
            let start = scroll.get(&folder.path).copied().unwrap_or(0).min(count);
            let stop = (start + SHOWN).min(count);
            out.extend((start..stop).map(|session| Row {
                kind: RowKind::Session,
                path: folder.path.clone(),
                folder: Some(index),
                session: Some(session),
                end: session + 1 == stop,
                ..Row::default()
            }));
        }
    }
    out
}

/// The rule between two folders.
fn rule(width: usize) -> Line<'static> {
    let text = format!(" {}", "─".repeat(width.saturating_sub(1)));
    styled(&[(&text, RULE)])
}

/// A folder's two lines: name and last use, then its sessions and the week's spend.
fn folder_lines(
    folder: &Folder,
    expanded: &BTreeSet<String>,
    now: f64,
    width: usize,
) -> [Line<'static>; 2] {
    let arrow = if expanded.contains(&folder.path) {
        " ▾ "
    } else {
        " ▸ "
    };
    let age = format!("{} ago", words::age(folder.last, now));
    let age_cells = width_of(&age);
    let mut first = styled(&[(arrow, SOFT)]);
    let name = words::fit(&folder.name, width.saturating_sub(4 + age_cells));
    push(&mut first, name, STRONG);
    let gap = width.saturating_sub(cell_len(&first) + age_cells);
    push(&mut first, " ".repeat(gap), INK);
    push(&mut first, age, SOFT);
    let plural = if folder.count == 1 { "" } else { "s" };
    let spend = if folder.spend == 0.0 {
        String::new()
    } else {
        format!(" · {} 7d", words::money(Some(folder.spend)))
    };
    let second = format!("   {} session{plural}{spend}", folder.count);
    [first, styled(&[(&second, SOFT)])]
}

/// One of a folder's last sessions: its title, when it was used and what it cost.
///
/// A row that is not a session of `folders` draws as an empty line.
fn session_line(row: &Row, folders: &[Folder], now: f64, width: usize) -> Line<'static> {
    let folder = row.folder.and_then(|f| folders.get(f));
    let Some((folder, (index, session))) = folder.and_then(|folder| {
        let index = row.session?;
        Some((folder, (index, folder.sessions.get(index)?)))
    }) else {
        return Line::default();
    };
    // The closing branch only under the folder's very last session: more may scroll into view
    let branch = if index + 1 == folder.sessions.len() {
        "└ ".to_owned()
    } else if row.end {
        format!("{MORE} ")
    } else {
        "├ ".to_owned()
    };
    let mut line = styled(&[("     ", INK), (&branch, FAINT)]);
    let tail = ljust(&words::age(session.last, now), AGE_WIDTH)
        + &rjust(&words::money(session.cost), SPEND_WIDTH);
    let title = width.saturating_sub(cell_len(&line) + width_of(&tail) + 1);
    push(
        &mut line,
        ljust(&words::fit(&session.title, title), title + 1),
        INK,
    );
    push(&mut line, tail, SOFT);
    line
}

/// Cuts a line longer than `width` with a grey ellipsis.
fn cut(mut line: Line<'static>, width: usize) -> Line<'static> {
    if cell_len(&line) > width {
        truncate(&mut line, width.saturating_sub(1));
        push(&mut line, "…", FAINT);
    }
    line
}

/// An entry to the disk: a grey arrow, a green `+` for a new folder, its name in bold and, when
/// open, its hint in grey.
fn root_line(row: &Row, width: usize) -> Line<'static> {
    // Only an open entry has a hint
    let open = !row.branch.is_empty();
    let mut line = styled(&[(if open { " ▾ " } else { " ▸ " }, SOFT)]);
    if row.root == Some(Root::New) {
        push(&mut line, "+ ", GREEN);
    }
    push(&mut line, row.label.clone(), STRONG);
    if open {
        push(&mut line, "  ", INK);
        push(&mut line, row.branch.clone(), INACTIVE);
    }
    cut(line, width)
}

/// A folder of an entry's tree, or a match of a search: its tree lines, then its name, a match's
/// parent folders in grey and the letters the search matched in Claude's orange.
fn dir_line(row: &Row, width: usize) -> Line<'static> {
    let mut line = styled(&[("   ", INK), (&row.branch, SUBTLE)]);
    // A match's label is its path below the folder searched: the name is after the last `/`
    let name_at = row
        .label
        .trim_end_matches('/')
        .rfind('/')
        .map_or(0, |i| i + 1);
    let room = width.saturating_sub(cell_len(&line));
    let fits = width_of(&row.label) <= room;
    let mut used = 0;
    let mut label: Vec<Span<'static>> = Vec::new();
    // Marks by byte offset, as the search gives them
    for (at, c) in row.label.char_indices() {
        if !fits && used + cells(c) > room.saturating_sub(1) {
            break;
        }
        used += cells(c);
        let style = if row.hits.contains(&at) {
            CLAUDE.add_modifier(Modifier::BOLD)
        } else if at < name_at || (c == '/' && at + 1 == row.label.len()) {
            INACTIVE
        } else {
            INK
        };
        match label.last_mut() {
            Some(span) if span.style == style => span.content.to_mut().push(c),
            _ => label.push(Span::styled(c.to_string(), style)),
        }
    }
    line.spans.extend(label);
    if !fits {
        push(&mut line, "…", FAINT);
    }
    line
}

/// A folder to create, then start a session in.
fn create_line(row: &Row, width: usize) -> Line<'static> {
    let text = format!("create {} and start", row.label);
    cut(styled(&[("   + ", GREEN), (&text, INK)]), width)
}

/// Every row's lines, with a rule under the entries and between folders, and the row each line
/// belongs to. With `nothing` set, a last line says nothing matches.
fn row_lines(
    shown: &[Row],
    folders: &[Folder],
    expanded: &BTreeSet<String>,
    nothing: bool,
    now: f64,
    width: usize,
) -> (Vec<Line<'static>>, Vec<Option<usize>>) {
    let mut lines = Vec::new();
    let mut owners = Vec::new();
    for (i, row) in shown.iter().enumerate() {
        let after_entries = i > 0 && shown[i - 1].root.is_some() && row.root.is_none();
        if !lines.is_empty() && (row.kind == RowKind::Folder || after_entries) {
            lines.push(rule(width));
            owners.push(None);
        }
        let drawn: Vec<Line<'static>> = match row.kind {
            RowKind::Path => {
                let open = words::fit(&format!("open {}", row.path), width.saturating_sub(5));
                vec![styled(&[("   + ", GREEN), (&open, INK)])]
            }
            RowKind::Root => vec![root_line(row, width)],
            RowKind::Dir => vec![dir_line(row, width)],
            RowKind::Create => vec![create_line(row, width)],
            RowKind::Session => vec![session_line(row, folders, now, width)],
            RowKind::Folder => match row.folder.and_then(|f| folders.get(f)) {
                Some(folder) => folder_lines(folder, expanded, now, width).into(),
                None => Vec::new(),
            },
        };
        owners.extend(std::iter::repeat_n(Some(i), drawn.len()));
        lines.extend(drawn);
    }
    if nothing {
        if !lines.is_empty() {
            lines.push(rule(width));
            owners.push(None);
        }
        lines.push(styled(&[("   nothing matches", FAINT)]));
        owners.push(None);
    }
    (lines, owners)
}

/// Drops a line's first character, keeping the styles of the rest.
fn drop_first(line: &mut Line<'static>) {
    if let Some(span) = line.spans.iter_mut().find(|s| !s.content.is_empty()) {
        let mut chars = span.content.chars();
        chars.next();
        span.content = chars.as_str().to_owned().into();
    }
}

/// The rows around the selected one, in `room` lines at most, after a gutter column that holds
/// a lavender pointer on the selected row's first line; the selected row's text is lavender too.
///
/// # Arguments
/// * `picker`: the list: its folders, selected row, open folders and search.
/// * `shown`: the rows from [`Picker::shown`].
/// * `now`: current time as a Unix timestamp.
/// * `room`: lines available.
/// * `width`: columns available.
pub fn list_lines(
    picker: &Picker,
    shown: &[Row],
    now: f64,
    room: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let cursor = picker.cursor;
    let typed = !picker.query.trim().is_empty();
    // A match: a history row, or a folder or create row of a searched tree
    let found = shown.iter().any(|r| match (r.kind, r.root) {
        (_, None) | (RowKind::Create, _) => true,
        (RowKind::Dir, Some(root)) => picker.searched(root).is_some(),
        _ => false,
    });
    // The gutter takes a column of the width
    let (lines, owners) = row_lines(
        shown,
        &picker.folders,
        &picker.expanded,
        typed && !found,
        now,
        width.saturating_sub(1),
    );
    let total = lines.len();
    let picked: Vec<usize> = owners
        .iter()
        .enumerate()
        .filter(|(_, owner)| **owner == Some(cursor))
        .map(|(n, _)| n)
        .collect();
    // An open folder brings its sessions on show along with it
    let folder = shown.get(cursor).and_then(|r| r.folder);
    let block_end = owners
        .iter()
        .rposition(|owner| {
            owner
                .and_then(|i| shown[i].folder)
                .is_some_and(|f| Some(f) == folder)
        })
        .or(picked.last().copied());
    // Scrolls just enough to keep every line of the selected row, and of its folder's open
    // sessions where they fit, in view
    let top = match (picked.first(), block_end) {
        (Some(&first), Some(last)) => (last + 1)
            .saturating_sub(room)
            .min(total.saturating_sub(room))
            .min(first),
        _ => 0,
    };
    let end = total.min(top + room);
    let mut out = Vec::with_capacity(end.saturating_sub(top));
    for (n, mut line) in lines.into_iter().enumerate().take(end).skip(top) {
        // A session line's branch already marks its folder's own more below
        if n + 1 == end && end < total && plain(&line).chars().nth(5) != Some(MORE) {
            // Its first column is always blank: room for the mark that more lies below
            drop_first(&mut line);
            line.spans.insert(0, Span::styled(MORE.to_string(), FAINT));
        }
        // Claude Code's menus: the text in lavender, no bar; grey parts stay grey
        if picked.contains(&n) {
            for span in &mut line.spans {
                let text = !span.content.trim().is_empty();
                if text && matches!(span.style.fg, None | Some(Color::White)) {
                    span.style.fg = PERMISSION.fg;
                }
            }
        }
        let gutter = if picked.first() == Some(&n) {
            Span::styled("❯", PERMISSION.add_modifier(Modifier::BOLD))
        } else {
            Span::raw(" ")
        };
        line.spans.insert(0, gutter);
        out.push(line);
    }
    out
}

/// The keys a screen takes, each in Claude's orange before what it does, if anything.
///
/// A line too long for `width` is cut with an ellipsis.
pub fn keys_line(keys: &[(&str, &str)], width: usize) -> Line<'static> {
    let mut line = Line::from(Span::raw(" "));
    for (i, (key, does)) in keys.iter().enumerate() {
        if i > 0 {
            push(&mut line, " · ", FAINT);
        }
        push(&mut line, *key, CLAUDE);
        if !does.is_empty() {
            push(&mut line, format!(" {does}"), INK);
        }
    }
    cut(line, width)
}

/// What the search box shows while nothing is typed: with a tree open, it also offers to name a
/// new folder.
pub fn placeholder(picker: &Picker) -> &'static str {
    if picker.open.is_empty() {
        "Search folders…"
    } else {
        "Search folders, or type a new folder's name…"
    }
}

/// The search in a rounded box, as Claude Code's prompt: three lines of `width` columns.
///
/// # Arguments
/// * `query`: what the user typed; text too long for the box keeps its end.
/// * `placeholder`: what the box shows in grey while `query` is empty.
/// * `focused`: whether keys type into it, shown by a cursor.
/// * `width`: columns the box takes, its border included.
pub fn search_box(
    query: &str,
    placeholder: &str,
    focused: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    let cursor = if focused { "▌" } else { "" };
    let mut middle = styled(&[("│", SOFT), (" ❯ ", CLAUDE)]);
    // What the text may take: the borders, the prompt, the cursor and a space before the right
    // border take the rest
    let room = inner.saturating_sub(4 + width_of(cursor));
    if query.is_empty() {
        push(&mut middle, cursor, INK);
        push(&mut middle, words::fit(placeholder, room), INACTIVE);
    } else {
        let mut used = 0;
        let kept: Vec<char> = query
            .chars()
            .rev()
            .take_while(|&c| {
                used += cells(c);
                used <= room
            })
            .collect();
        push(&mut middle, kept.into_iter().rev().collect::<String>(), INK);
        push(&mut middle, cursor, INK);
    }
    let gap = (inner + 1).saturating_sub(cell_len(&middle));
    push(&mut middle, " ".repeat(gap), INK);
    push(&mut middle, "│", SOFT);
    let border = "─".repeat(inner);
    vec![
        styled(&[(&format!("╭{border}╮"), SOFT)]),
        middle,
        styled(&[(&format!("╰{border}╯"), SOFT)]),
    ]
}

/// What the keys do on the selected row, as `(key, what it does)`, ending with Esc. ← in a
/// tree is offered only while nothing is typed, as it does nothing then.
///
/// # Arguments
/// * `row`: the selected row, if any.
/// * `picker`: what is open and typed, to tell whether → opens or ← closes.
pub fn hints(row: Option<&Row>, picker: &Picker) -> Vec<(&'static str, &'static str)> {
    let left = picker.query.is_empty();
    let mut keys = match row {
        Some(r) => match (r.kind, r.root) {
            (RowKind::Root, Some(root)) if picker.open.contains(&root) => {
                vec![(if left { "Enter/←" } else { "Enter" }, "close")]
            }
            (RowKind::Root, _) => vec![("Enter/→", "open")],
            (RowKind::Dir, Some(Root::New)) if left => {
                vec![("Enter", "pick as parent"), ("←", "back")]
            }
            (RowKind::Dir, Some(Root::New)) => vec![("Enter", "pick as parent")],
            (RowKind::Dir, Some(root)) if picker.dirs.contains(&(root, r.path.clone())) => {
                let mut keys = vec![("Enter", "start here")];
                keys.extend(left.then_some(("←", "close")));
                keys
            }
            (RowKind::Dir, _) => vec![("Enter", "start here"), ("→", "open")],
            (RowKind::Create, _) => vec![("Enter", "create & start")],
            (RowKind::Folder, _) if picker.expanded.contains(&r.path) => {
                vec![("Enter", "new session"), ("←", "close")]
            }
            (RowKind::Folder, _) => vec![("Enter", "new session"), ("→", "sessions")],
            (RowKind::Session, _) => vec![("Enter", "resume")],
            (RowKind::Path, _) => vec![("Enter", "start here")],
        },
        None => Vec::new(),
    };
    keys.push(("Esc", "cancel"));
    keys
}

/// Hints in Claude Code's form, `{key} to {does}` in grey joined by ` · `.
///
/// A line too long for `width` is cut with an ellipsis.
pub fn hints_line(keys: &[(&str, &str)], width: usize) -> Line<'static> {
    let mut line = Line::from(Span::raw(" "));
    for (i, (key, does)) in keys.iter().enumerate() {
        if i > 0 {
            push(&mut line, " · ", SUBTLE);
        }
        push(&mut line, format!("{key} to {does}"), INACTIVE);
    }
    cut(line, width)
}

/// The Alt+n panel as `height` lines of `width` columns: the search box, a blank line, the list
/// and what the keys do on the selected row.
pub fn screen(picker: &mut Picker, now: f64, width: u16, height: u16) -> Vec<Line<'static>> {
    let shown = picker.shown();
    let height = usize::from(height);
    // A cell to spare on the right, as the landing page leaves
    let width = usize::from(width).saturating_sub(1);
    let mut lines = search_box(&picker.query, placeholder(picker), true, width);
    lines.push(Line::default());
    lines.extend(list_lines(
        picker,
        &shown,
        now,
        height.saturating_sub(6),
        width,
    ));
    while lines.len() < height.saturating_sub(1) {
        lines.push(Line::default());
    }
    lines.push(hints_line(&hints(shown.get(picker.cursor), picker), width));
    lines
        .into_iter()
        .map(|line| {
            if plain(&line).is_empty() {
                Line::raw(" ")
            } else {
                line
            }
        })
        .take(height)
        .collect()
}

/// A path with a leading `~` or `~/` turned into the home folder, as `os.path.expanduser`
/// does; `~user` forms are left as they are.
fn expand_user(path: &str) -> String {
    let rest = match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest,
        _ => return path.to_owned(),
    };
    let home = paths::home(&paths::process_env);
    let expanded = format!("{}{rest}", home.to_string_lossy().trim_end_matches('/'));
    if expanded.is_empty() {
        "/".to_owned()
    } else {
        expanded
    }
}

/// `path` with the home folder `home` shown as `~`.
fn tilde(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => path.to_owned(),
    }
}

/// The tmux `new-window` arguments that open a row: resume its session, or start one.
pub fn command(row: &Row, folders: &[Folder]) -> Vec<String> {
    let session = row
        .folder
        .and_then(|f| folders.get(f))
        .zip(row.session)
        .and_then(|(folder, s)| folder.sessions.get(s));
    match session {
        Some(session) if row.kind == RowKind::Session => vec![
            "-c".to_owned(),
            row.path.clone(),
            format!("claude --resume {}", shell::quote(&session.id)),
        ],
        _ => vec!["-c".to_owned(), expand_user(&row.path), "claude".to_owned()],
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::fs::{self, File};
    use std::time::{Duration, UNIX_EPOCH};

    use ratatui::style::Modifier;
    use ratatui::text::Line;

    use crate::ui::line::{cell_len, cells};
    use crate::ui::picker::{
        command, dir_line, hints, hints_line, keys_line, list_lines, placeholder, rows, screen,
        search_box, session_line, tilde, Folder, Key, Picker, Root, Row, RowKind, Session, WIDTH,
    };
    use crate::ui::words::{CLAUDE, GREEN, INACTIVE, INK, PERMISSION, SOFT, SUBTLE};

    const NOW: f64 = 1_700_000_000.0;

    fn sessions() -> Vec<Session> {
        (0..7)
            .map(|i| Session {
                id: format!("s{i}"),
                title: format!("task {i}"),
                last: NOW - 3600.0 * f64::from(i),
                cost: (i == 0).then_some(1.5),
            })
            .collect()
    }

    fn folders() -> Vec<Folder> {
        vec![
            Folder {
                path: "/w/api".into(),
                name: "api".into(),
                last: NOW - 60.0,
                count: 41,
                spend: 12.5,
                sessions: sessions(),
            },
            Folder {
                path: "/w/web".into(),
                name: "web".into(),
                last: NOW - 7200.0,
                count: 1,
                spend: 0.0,
                sessions: sessions()[..2].to_vec(),
            },
        ]
    }

    fn picker(is_dir: bool) -> Picker {
        let mut p = Picker::new(Box::new(move |_| is_dir));
        p.folders = folders();
        p
    }

    /// Moves the cursor past the two entries, onto the first history row.
    fn past_entries(p: &mut Picker) {
        p.key(Key::Down);
        p.key(Key::Down);
    }

    fn set(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    fn plain(line: &Line) -> String {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        text.trim_end().to_owned()
    }

    fn no_dirs(_: &str) -> bool {
        false
    }

    /// A picker over `folders`, the cursor on row `cursor`, the folders in `expanded` open.
    fn list(folders: &[Folder], cursor: usize, expanded: &BTreeSet<String>) -> Picker {
        let mut p = Picker::new(Box::new(no_dirs));
        p.folders = folders.to_vec();
        p.cursor = cursor;
        p.expanded = expanded.clone();
        p
    }

    fn session_ids(shown: &[Row], folders: &[Folder]) -> Vec<String> {
        shown
            .iter()
            .filter(|r| r.kind == RowKind::Session)
            .map(|r| {
                let (f, s) = (r.folder.unwrap(), r.session.unwrap());
                folders[f].sessions[s].id.clone()
            })
            .collect()
    }

    #[test]
    fn rows_list_folders_and_open_a_folders_last_five_sessions() {
        let fs = folders();
        let shown = rows(&fs, "", &set(&["/w/api"]), &no_dirs, &HashMap::new());
        let kinds: Vec<_> = shown.iter().map(|r| (r.kind, r.path.as_str())).collect();
        let mut want = vec![(RowKind::Folder, "/w/api")];
        want.extend([(RowKind::Session, "/w/api"); 5]);
        want.push((RowKind::Folder, "/w/web"));
        assert_eq!(kinds, want);
        assert_eq!(session_ids(&shown, &fs), ["s0", "s1", "s2", "s3", "s4"]);
        let ends: Vec<_> = shown
            .iter()
            .filter(|r| r.kind == RowKind::Session)
            .map(|r| r.end)
            .collect();
        assert_eq!(ends, [false, false, false, false, true]);
    }

    #[test]
    fn a_folders_sessions_scroll_past_the_last_and_back() {
        let mut p = picker(false);
        past_entries(&mut p);
        p.key(Key::Right);
        for _ in 0..7 {
            p.key(Key::Down);
        }
        // Two steps past the fifth session scroll by two; the cursor stays on the last line
        assert_eq!(p.cursor, 7);
        assert_eq!(p.scroll, HashMap::from([("/w/api".to_owned(), 2)]));
        let shown = p.shown();
        assert_eq!(
            session_ids(&shown, &p.folders),
            ["s2", "s3", "s4", "s5", "s6"]
        );
        let selected = p.selected().unwrap();
        let last = plain(&session_line(&selected, &p.folders, NOW, WIDTH));
        assert!(last.starts_with("     └ task 6"), "{last}");
        // With nothing older left, the cursor moves on to the next folder
        p.key(Key::Down);
        assert_eq!(p.selected().unwrap().path, "/w/web");
        // Back up to the first session on show, then one more scrolls back by one
        for _ in 0..6 {
            p.key(Key::Up);
        }
        assert_eq!(p.cursor, 3);
        assert_eq!(p.scroll, HashMap::from([("/w/api".to_owned(), 1)]));
        p.key(Key::Left);
        assert!(p.scroll.is_empty() && p.expanded.is_empty());
    }

    #[test]
    fn a_folder_stays_closed_until_expanded() {
        let shown = rows(&folders(), "", &set(&[]), &no_dirs, &HashMap::new());
        let kinds: Vec<_> = shown.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [RowKind::Folder, RowKind::Folder]);
    }

    #[test]
    fn typing_filters_by_every_word_and_a_typed_folder_comes_first() {
        let fs = folders();
        let none = HashMap::new();
        let paths: Vec<_> = rows(&fs, "W E", &set(&[]), &no_dirs, &none)
            .into_iter()
            .map(|r| r.path)
            .collect();
        assert_eq!(paths, ["/w/web"]);
        let shown = rows(&fs, "~/x", &set(&[]), &|_| true, &none);
        assert_eq!(
            (shown[0].kind, shown[0].path.as_str()),
            (RowKind::Path, "~/x")
        );
        assert!(rows(&fs, "/nope", &set(&[]), &no_dirs, &none).is_empty());
    }

    #[test]
    fn screen_draws_two_lines_per_folder_and_the_open_sessions() {
        let mut p = picker(false);
        // Python's test draws rows for "" under the query "ap"; here the rows follow the
        // query, and "w" keeps both folders
        p.query = "w".into();
        p.expanded = set(&["/w/api"]);
        past_entries(&mut p);
        let lines = screen(&mut p, NOW, WIDTH as u16 + 1, 19);
        assert_eq!(lines.len(), 19);
        let text: Vec<String> = lines.iter().map(plain).collect();
        for t in &text {
            assert!(t.chars().map(cells).sum::<usize>() <= WIDTH, "{t}");
        }
        // The search box, then a blank line, then the list in a gutter for the pointer
        assert!(text[0].starts_with('╭') && text[2].starts_with('╰'));
        assert!(text[1].starts_with("│ ❯ w▌"), "{}", text[1]);
        assert_eq!(text[3], "");
        // The two entries come first, closed, a rule under them
        assert_eq!(text[4], "  ▸ + New folder…");
        assert_eq!(text[5], "  ▸ Open another folder…");
        assert_eq!(text[6].trim(), "─".repeat(WIDTH - 2));
        assert!(text[7].starts_with("❯ ▾ api") && text[7].ends_with("1m ago"));
        assert_eq!(text[8], "    41 sessions · $12.50 7d");
        assert!(text[9].starts_with("      ├ task 0") && text[9].ends_with("$1.50"));
        assert!(text[11].starts_with("      ├ task 2") && text[11].contains("2h"));
        // More sessions scroll into view below: a mark, no closing branch yet
        assert!(text[13].starts_with("      ▼ task 4"), "{}", text[13]);
        assert_eq!(text[14].trim(), "─".repeat(WIDTH - 2));
        // No spend this week: no spend part
        assert!(text[15].starts_with("  ▸ web"));
        assert_eq!(text[16], "    1 session");
        // The hints of the open folder, cut at this narrow width
        assert_eq!(text[18], " Enter to new session · ← to close · Esc to can…");
    }

    #[test]
    fn the_list_scrolls_to_keep_the_selected_row_in_view() {
        let many: Vec<Folder> = (0..30)
            .map(|i| Folder {
                path: format!("/w/f{i}"),
                name: format!("f{i}"),
                last: NOW - f64::from(i),
                count: 1,
                spend: 0.0,
                sessions: Vec::new(),
            })
            .collect();
        let none = set(&[]);
        let shown = rows(&many, "", &none, &no_dirs, &HashMap::new());
        let lines: Vec<String> = list_lines(&list(&many, 25, &none), &shown, NOW, 8, WIDTH)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(lines.len(), 8);
        assert!(lines.iter().any(|x| x.contains(" f25")));
        assert_eq!(lines[7], " ▼  1 session");
        assert!(!lines.iter().any(|x| x.contains(" f0")));
        // At the bottom of the list nothing more lies below
        let end = list_lines(&list(&many, 29, &none), &shown, NOW, 8, WIDTH);
        assert_eq!(plain(&end[7]), "    1 session");
    }

    #[test]
    fn opening_a_folder_low_in_the_list_brings_its_sessions_into_view() {
        let mut many: Vec<Folder> = (0..8)
            .map(|i| Folder {
                path: format!("/w/f{i}"),
                name: format!("f{i}"),
                last: NOW - f64::from(i),
                count: 1,
                spend: 0.0,
                sessions: Vec::new(),
            })
            .collect();
        many[5].sessions = sessions();
        let open = set(&["/w/f5"]);
        let shown = rows(&many, "", &open, &no_dirs, &HashMap::new());
        let lines: Vec<String> = list_lines(&list(&many, 5, &open), &shown, NOW, 16, WIDTH)
            .iter()
            .map(plain)
            .collect();
        assert!(lines.iter().any(|x| x.starts_with("❯ ▾ f5")), "{lines:?}");
        let tasks: Vec<_> = lines.iter().filter(|x| x.contains("task ")).collect();
        assert_eq!(tasks.len(), 5, "{lines:?}");
    }

    #[test]
    fn nothing_matching_says_so() {
        let mut p = list(&[], 0, &set(&[]));
        p.query = "zz".into();
        let lines = list_lines(&p, &[], NOW, 8, WIDTH);
        assert_eq!(plain(&lines[0]), "    nothing matches");
    }

    #[test]
    fn nothing_matches_shows_only_once_something_is_typed() {
        let mut p = Picker::new(Box::new(no_dirs));
        let says = |p: &mut Picker| {
            screen(p, NOW, WIDTH as u16 + 1, 19)
                .iter()
                .any(|l| plain(l).contains("nothing matches"))
        };
        assert!(!says(&mut p));
        p.query = " ".into();
        assert!(!says(&mut p));
        p.query = "zz".into();
        assert!(says(&mut p));
    }

    /// Whether the panel says nothing matches.
    fn says_nothing_matches(p: &mut Picker) -> bool {
        screen(p, NOW, WIDTH as u16 + 1, 40)
            .iter()
            .any(|l| plain(l).contains("nothing matches"))
    }

    #[test]
    fn nothing_matches_only_when_no_row_at_all_matches() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Down);
        p.key(Key::Right);
        // A folder on disk matches, no history folder does
        type_text(&mut p, "Music");
        assert!(!says_nothing_matches(&mut p));
        // Neither a folder on disk nor a history folder, and no create row
        let (_t, mut p, _) = home_picker();
        p.key(Key::Down);
        p.key(Key::Right);
        type_text(&mut p, "~zz");
        assert!(!below(&mut p, Root::Open)
            .iter()
            .any(|(k, _)| *k != RowKind::Dir));
        assert!(says_nothing_matches(&mut p));
        // A tree open but not searched: its folders match nothing
        let (_t, mut p, _) = home_picker();
        p.key(Key::Down);
        p.key(Key::Right);
        p.key(Key::Down);
        p.key(Key::Down);
        p.key(Key::Down);
        p.key(Key::Down);
        assert_eq!(p.selected().unwrap().kind, RowKind::Folder);
        type_text(&mut p, "zz");
        assert_eq!(p.anchor, None);
        assert!(says_nothing_matches(&mut p));
    }

    #[test]
    fn the_picker_moves_opens_and_types() {
        let mut p = picker(false);
        past_entries(&mut p);
        assert_eq!(p.key(Key::Right), None);
        assert!(p.expanded.contains("/w/api"));
        p.key(Key::Down);
        let opened = p.key(Key::Enter).unwrap();
        assert_eq!(opened.session, Some(0));
        assert_eq!(p.folders[0].sessions[0].id, "s0");
        p.key(Key::Left);
        assert_eq!(p.cursor, 2);
        assert!(p.expanded.is_empty());
        p.key(Key::Char('w'));
        assert_eq!((p.query.as_str(), p.cursor), ("w", 2));
        let selected = p.selected().unwrap();
        assert_eq!(command(&selected, &p.folders), ["-c", "/w/api", "claude"]);
        p.key(Key::Backspace);
        assert_eq!(p.query, "");
    }

    #[test]
    fn left_returns_to_the_folder_even_when_a_typed_path_matches_it() {
        let mut p = picker(true);
        for c in "/w/api".chars() {
            p.key(Key::Char(c));
        }
        for key in [Key::Down, Key::Right, Key::Down, Key::Left] {
            p.key(key);
        }
        assert_eq!(p.selected().unwrap().kind, RowKind::Folder);
        assert!(p.expanded.is_empty());
    }

    #[test]
    fn command_quotes_session_ids() {
        let mut fs = folders();
        fs[0].sessions[0].id = "a b".into();
        let row = Row {
            kind: RowKind::Session,
            path: "/w/api".into(),
            folder: Some(0),
            session: Some(0),
            end: false,
            ..Row::default()
        };
        assert_eq!(
            command(&row, &fs),
            ["-c", "/w/api", "claude --resume 'a b'"]
        );
    }

    #[test]
    fn command_expands_the_home_of_a_typed_path() {
        let row = Row {
            kind: RowKind::Path,
            path: "~/x".into(),
            folder: None,
            session: None,
            end: false,
            ..Row::default()
        };
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            command(&row, &[]),
            ["-c".to_owned(), format!("{home}/x"), "claude".to_owned()]
        );
    }

    /// A picker whose home is a tempdir holding `Documents/`, `workspace/vtp/` and `Music/`,
    /// all changed at the same time so the tree lists them by name.
    fn home_picker() -> (tempfile::TempDir, Picker, String) {
        let t = tempfile::tempdir().unwrap();
        for dir in ["Documents", "workspace/vtp", "Music"] {
            fs::create_dir_all(t.path().join(dir)).unwrap();
        }
        let when = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        for dir in ["Documents", "workspace", "workspace/vtp", "Music"] {
            File::open(t.path().join(dir))
                .unwrap()
                .set_modified(when)
                .unwrap();
        }
        let home = t.path().to_string_lossy().into_owned();
        let mut p = picker(false);
        p.home = home.clone();
        (t, p, home)
    }

    /// The label and branch of every tree row under `root`.
    fn tree(p: &mut Picker, root: Root) -> Vec<(String, String)> {
        p.shown()
            .into_iter()
            .filter(|r| r.kind == RowKind::Dir && r.root == Some(root))
            .map(|r| (r.label, r.branch))
            .collect()
    }

    fn pairs(want: &[(&str, &str)]) -> Vec<(String, String)> {
        want.iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn closing_an_entry_drops_the_folder_cache() {
        let (t, mut p, _) = home_picker();
        let listed = |p: &mut Picker| tree(p, Root::Open).iter().any(|(l, _)| l == "Later/");
        // Left closes the entry, as Enter does
        for close in [Key::Left, Key::Enter] {
            p.cursor = 1;
            p.key(Key::Right);
            assert!(!listed(&mut p));
            let made = t.path().join("Later");
            fs::create_dir(&made).unwrap();
            assert!(!listed(&mut p), "read once and kept while open");
            p.key(close);
            p.key(Key::Right);
            assert!(listed(&mut p), "{close:?}");
            p.key(Key::Left);
            fs::remove_dir(&made).unwrap();
        }
    }

    #[test]
    fn the_two_entries_come_first_and_the_cursor_starts_on_new_folder() {
        let (_t, mut p, _) = home_picker();
        let shown = p.shown();
        let first: Vec<_> = shown[..2].iter().map(|r| (r.kind, r.root)).collect();
        assert_eq!(
            first,
            [
                (RowKind::Root, Some(Root::New)),
                (RowKind::Root, Some(Root::Open))
            ]
        );
        assert_eq!(shown[2].kind, RowKind::Folder);
        assert_eq!(p.cursor, 0);
    }

    #[test]
    fn right_opens_an_entry_as_a_tree_of_the_home_folder() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Down);
        p.key(Key::Right);
        assert_eq!(
            tree(&mut p, Root::Open),
            pairs(&[("Documents/", "├ "), ("Music/", "├ "), ("workspace/", "└ ")])
        );
        assert!(tree(&mut p, Root::New).is_empty());
    }

    #[test]
    fn the_tree_puts_folders_with_sessions_first() {
        let (_t, mut p, home) = home_picker();
        p.folders.push(Folder {
            path: format!("{home}/workspace/vtp"),
            name: "vtp".into(),
            last: NOW,
            count: 1,
            spend: 0.0,
            sessions: Vec::new(),
        });
        p.key(Key::Down);
        p.key(Key::Right);
        assert_eq!(tree(&mut p, Root::Open)[0].0, "workspace/");
    }

    #[test]
    fn a_newer_session_in_a_listed_folder_reorders_the_tree() {
        let (_t, mut p, home) = home_picker();
        for (name, last) in [("Music", NOW), ("workspace/vtp", NOW - 60.0)] {
            p.folders.push(Folder {
                path: format!("{home}/{name}"),
                name: name.into(),
                last,
                count: 1,
                spend: 0.0,
                sessions: Vec::new(),
            });
        }
        p.key(Key::Down);
        p.key(Key::Right);
        assert_eq!(tree(&mut p, Root::Open)[0].0, "Music/");
        // A reload: the same folders, one used again since
        let count = p.folders.len();
        let vtp = p.folders.last_mut().unwrap();
        vtp.last = NOW + 60.0;
        assert_eq!(p.folders.len(), count);
        assert_eq!(tree(&mut p, Root::Open)[0].0, "workspace/");
    }

    #[test]
    fn a_second_shown_reads_no_folder_change_again() {
        let (t, mut p, _) = home_picker();
        p.key(Key::Down);
        p.key(Key::Right);
        assert_eq!(tree(&mut p, Root::Open)[0].0, "Documents/");
        // Were the change read again, Music would now come first
        let later = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let music = File::open(t.path().join("Music")).unwrap();
        music.set_modified(later).unwrap();
        assert_eq!(tree(&mut p, Root::Open)[0].0, "Documents/");
        p.key(Key::Left);
        p.key(Key::Right);
        assert_eq!(
            tree(&mut p, Root::Open)[0].0,
            "Music/",
            "read anew once reopened"
        );
    }

    #[test]
    fn right_on_a_folder_opens_it_and_left_closes_it() {
        let (_t, mut p, home) = home_picker();
        for key in [Key::Down, Key::Right, Key::Down, Key::Down, Key::Down] {
            p.key(key);
        }
        assert_eq!(p.selected().unwrap().label, "workspace/");
        p.key(Key::Right);
        let next = &p.shown()[p.cursor + 1];
        assert_eq!(
            (next.label.as_str(), next.branch.as_str()),
            ("vtp/", "  └ ")
        );
        assert_eq!(next.path, format!("{home}/workspace/vtp"));
        p.key(Key::Left);
        assert_eq!(p.selected().unwrap().label, "workspace/");
        assert!(!tree(&mut p, Root::Open).iter().any(|(l, _)| l == "vtp/"));
        p.key(Key::Left);
        let entry = p.selected().unwrap();
        assert_eq!((entry.kind, entry.root), (RowKind::Root, Some(Root::Open)));
        assert!(p.open.contains(&Root::Open));
        // Backspace with nothing typed does what Left does
        for key in [Key::Down, Key::Down, Key::Down, Key::Right, Key::Backspace] {
            p.key(key);
        }
        assert!(p.dirs.is_empty());
        p.key(Key::Backspace);
        assert_eq!(p.selected().unwrap().kind, RowKind::Root);
        p.key(Key::Backspace);
        assert!(p.open.is_empty());
    }

    #[test]
    fn enter_on_an_entry_toggles_it() {
        let (_t, mut p, _) = home_picker();
        assert_eq!(p.key(Key::Enter), None);
        assert_eq!(p.open, BTreeSet::from([Root::New]));
        assert_eq!(tree(&mut p, Root::New).len(), 3);
        assert_eq!(p.key(Key::Enter), None);
        assert!(p.open.is_empty());
        assert!(tree(&mut p, Root::New).is_empty());
    }

    #[test]
    fn enter_on_an_open_another_folder_row_returns_it() {
        let (_t, mut p, home) = home_picker();
        for key in [Key::Down, Key::Right, Key::Down, Key::Down] {
            p.key(key);
        }
        let row = p.key(Key::Enter).unwrap();
        assert_eq!(
            (row.kind, row.root, row.path.as_str()),
            (
                RowKind::Dir,
                Some(Root::Open),
                format!("{home}/Music").as_str()
            )
        );
        assert_eq!(command(&row, &p.folders)[1], format!("{home}/Music"));
    }

    #[test]
    fn enter_on_a_new_folder_row_picks_it_as_parent() {
        let (_t, mut p, home) = home_picker();
        p.key(Key::Right);
        p.key(Key::Down);
        assert_eq!(p.key(Key::Enter), None);
        let documents = format!("{home}/Documents");
        assert_eq!(p.dirs, BTreeSet::from([(Root::New, documents.clone())]));
        let row = p.selected().unwrap();
        assert_eq!((p.cursor, row.path), (1, documents));
    }

    /// Types `text` into the search, one key per character.
    fn type_text(p: &mut Picker, text: &str) {
        for c in text.chars() {
            p.key(Key::Char(c));
        }
    }

    /// Opens `root` and, in its tree, `Documents/`, leaving the cursor on it.
    fn open_documents(p: &mut Picker, root: Root) {
        if root == Root::Open {
            p.key(Key::Down);
        }
        p.key(Key::Right);
        p.key(Key::Down);
        assert_eq!(p.selected().unwrap().label, "Documents/");
        p.key(Key::Right);
    }

    /// The kind and path of every row under `root` but its entry row.
    fn below(p: &mut Picker, root: Root) -> Vec<(RowKind, String)> {
        p.shown()
            .into_iter()
            .filter(|r| r.root == Some(root) && r.kind != RowKind::Root)
            .map(|r| (r.kind, r.path))
            .collect()
    }

    #[test]
    fn typing_on_a_closed_entry_filters_the_history() {
        let (_t, mut p, _) = home_picker();
        type_text(&mut p, "api");
        assert_eq!(p.anchor, None);
        let history: Vec<_> = p
            .shown()
            .into_iter()
            .filter(|r| r.root.is_none())
            .map(|r| r.path)
            .collect();
        assert_eq!(history, ["/w/api"]);
        assert_eq!(p.selected().unwrap().path, "/w/api");
    }

    #[test]
    fn typing_in_the_tree_searches_below_the_current_folder() {
        let (_t, mut p, home) = home_picker();
        for key in [Key::Down, Key::Right, Key::Down, Key::Down, Key::Down] {
            p.key(key);
        }
        assert_eq!(p.selected().unwrap().label, "workspace/");
        p.key(Key::Right);
        type_text(&mut p, "VT");
        let workspace = format!("{home}/workspace");
        assert_eq!(p.anchor, Some((Root::Open, workspace.clone())));
        let shown = p.shown();
        let entry = shown
            .iter()
            .find(|r| r.kind == RowKind::Root && r.root == Some(Root::Open))
            .unwrap();
        assert_eq!(entry.branch, format!("in {}/", tilde(&workspace, &p.home)));
        assert_eq!(entry.branch, "in ~/workspace/");
        let first = shown.iter().position(|r| r.kind == RowKind::Dir).unwrap();
        assert_eq!(shown[first].label, "vtp");
        assert_eq!(shown[first].path, format!("{workspace}/vtp"));
        assert_eq!(p.cursor, first);
    }

    #[test]
    fn a_new_name_offers_create_last_under_open_and_first_under_new() {
        for root in [Root::Open, Root::New] {
            let (t, mut p, home) = home_picker();
            fs::create_dir(t.path().join("Documents/old-meu-projeto")).unwrap();
            open_documents(&mut p, root);
            type_text(&mut p, "Meu-Projeto");
            let create = (RowKind::Create, format!("{home}/Documents/Meu-Projeto"));
            let hit = (RowKind::Dir, format!("{home}/Documents/old-meu-projeto"));
            let selected = p.selected().unwrap();
            if root == Root::Open {
                assert_eq!(below(&mut p, root), [hit, create]);
                assert_eq!(selected.kind, RowKind::Dir);
            } else {
                assert_eq!(below(&mut p, root), [create.clone(), hit]);
                assert_eq!((selected.kind, selected.path), create);
            }
        }
    }

    #[test]
    fn typing_a_path_in_an_open_tree_selects_the_path_row() {
        let (t, _, home) = home_picker();
        let music = t.path().join("Music").to_string_lossy().into_owned();
        for typed in [music.clone(), "~/Music".to_owned()] {
            let (_t, mut p, _) = home_picker();
            p.home = home.clone();
            let at = home.clone();
            p.is_dir = Box::new(move |path| {
                let path = path
                    .strip_prefix('~')
                    .map_or(path.to_owned(), |r| format!("{at}{r}"));
                std::path::Path::new(&path).is_dir()
            });
            // The cursor on the open new-folder entry
            p.key(Key::Enter);
            type_text(&mut p, &typed);
            let row = p.key(Key::Enter).unwrap();
            assert_eq!((row.kind, row.path), (RowKind::Path, typed));
            assert!(p.open.contains(&Root::New));
        }
    }

    #[test]
    fn create_is_not_offered_over_anything_on_disk() {
        for (made, typed) in [
            (".config", ".config"),
            ("target", "target"),
            ("a/b", "a/b"),
            ("notes", "notes"),
            ("notes", "notes/x"),
        ] {
            let (t, mut p, _) = home_picker();
            let path = t.path().join("Documents").join(made);
            if made == "notes" {
                fs::write(&path, "").unwrap();
            } else {
                fs::create_dir_all(&path).unwrap();
            }
            open_documents(&mut p, Root::New);
            type_text(&mut p, typed);
            let creates = below(&mut p, Root::New)
                .iter()
                .any(|(kind, _)| *kind == RowKind::Create);
            assert!(!creates, "{typed}");
        }
    }

    #[test]
    fn create_respects_case() {
        let (t, mut p, _) = home_picker();
        fs::create_dir(t.path().join("Documents/meu-projeto")).unwrap();
        open_documents(&mut p, Root::New);
        type_text(&mut p, "Meu-Projeto");
        let creates = |p: &mut Picker| {
            below(p, Root::New)
                .iter()
                .any(|(kind, _)| *kind == RowKind::Create)
        };
        assert!(creates(&mut p));
        for _ in 0.."Meu-Projeto".len() {
            p.key(Key::Backspace);
        }
        type_text(&mut p, "meu-projeto");
        assert!(!creates(&mut p));
    }

    #[test]
    fn text_with_a_slash_only_offers_create() {
        let (t, mut p, home) = home_picker();
        fs::create_dir(t.path().join("Documents/abc")).unwrap();
        open_documents(&mut p, Root::Open);
        type_text(&mut p, "a/b");
        assert_eq!(
            below(&mut p, Root::Open),
            [(RowKind::Create, format!("{home}/Documents/a/b"))]
        );
    }

    #[test]
    fn only_a_relative_name_offers_create() {
        for text in ["~/Documents/X", "/tmp/x", "..", "a/../b", ".", "./a", "~"] {
            let (_t, mut p, _) = home_picker();
            open_documents(&mut p, Root::New);
            type_text(&mut p, text);
            let creates = below(&mut p, Root::New)
                .iter()
                .any(|(kind, _)| *kind == RowKind::Create);
            assert!(!creates, "{text}");
        }
        let (_t, mut p, home) = home_picker();
        open_documents(&mut p, Root::New);
        type_text(&mut p, "a/b");
        assert_eq!(
            below(&mut p, Root::New),
            [(RowKind::Create, format!("{home}/Documents/a/b"))]
        );
    }

    #[test]
    fn spaces_around_the_text_are_trimmed() {
        let (_t, mut p, home) = home_picker();
        open_documents(&mut p, Root::New);
        type_text(&mut p, "  Meu-Projeto ");
        let row = p.selected().unwrap();
        assert_eq!(row.kind, RowKind::Create);
        assert_eq!(row.path, format!("{home}/Documents/Meu-Projeto"));
    }

    #[test]
    fn right_on_a_match_opens_the_tree_down_to_it() {
        let (_t, mut p, home) = home_picker();
        p.key(Key::Down);
        p.key(Key::Right);
        type_text(&mut p, "vtp");
        assert_eq!(p.anchor, Some((Root::Open, home.clone())));
        assert_eq!(p.selected().unwrap().label, "workspace/vtp");
        p.key(Key::Right);
        assert_eq!((p.query.as_str(), &p.anchor), ("", &None));
        let workspace = format!("{home}/workspace");
        let vtp = format!("{workspace}/vtp");
        assert_eq!(
            p.dirs,
            BTreeSet::from([(Root::Open, workspace.clone()), (Root::Open, vtp)])
        );
        let row = p.selected().unwrap();
        assert_eq!(
            (row.kind, row.label.as_str(), row.path),
            (RowKind::Dir, "vtp/", format!("{workspace}/vtp"))
        );
    }

    #[test]
    fn clearing_the_text_returns_to_the_tree() {
        let (_t, mut p, _) = home_picker();
        for key in [Key::Down, Key::Right, Key::Down, Key::Down, Key::Down] {
            p.key(key);
        }
        let before = (tree(&mut p, Root::Open), p.cursor);
        type_text(&mut p, "vt");
        assert_ne!(tree(&mut p, Root::Open), before.0);
        p.key(Key::Backspace);
        p.key(Key::Backspace);
        assert_eq!((p.query.as_str(), &p.anchor), ("", &None));
        assert_eq!((tree(&mut p, Root::Open), p.cursor), before);
        assert_eq!(p.selected().unwrap().label, "workspace/");
    }

    #[test]
    fn the_selected_row_gets_a_lavender_pointer_not_a_bar() {
        let mut p = picker(false);
        past_entries(&mut p);
        let shown = p.shown();
        let lines = list_lines(&p, &shown, NOW, 20, WIDTH);
        let at = lines.iter().position(|l| plain(l).contains("api")).unwrap();
        let first = &lines[at].spans[0];
        assert_eq!(first.content, "❯");
        assert_eq!(first.style, PERMISSION.add_modifier(Modifier::BOLD));
        // The name takes the pointer's colour; the other rows keep an empty gutter
        let name = lines[at].spans.iter().find(|s| s.content == "api").unwrap();
        assert_eq!(name.style.fg, PERMISSION.fg);
        assert!(lines
            .iter()
            .enumerate()
            .all(|(n, l)| n == at || plain(l).starts_with(' ')));
        let bars = lines
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.style.bg.is_some());
        assert!(!bars, "{lines:?}");
        assert!(lines.iter().all(|l| cell_len(l) <= WIDTH));
    }

    #[test]
    fn an_open_entry_shows_its_hint() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Down);
        p.key(Key::Right);
        let shown = p.shown();
        let lines = list_lines(&p, &shown, NOW, 20, WIDTH);
        assert_eq!(plain(&lines[0]), "  ▸ + New folder…");
        assert_eq!(
            plain(&lines[1]),
            "❯ ▾ Open another folder…  type to search below"
        );
        let hint = lines[1].spans.last().unwrap();
        assert_eq!(
            (hint.content.as_ref(), hint.style),
            ("type to search below", INACTIVE)
        );
        let plus = lines[0].spans.iter().find(|s| s.content == "+ ").unwrap();
        assert_eq!(plus.style, GREEN);
    }

    #[test]
    fn a_match_greys_its_parent_and_marks_its_letters() {
        let row = Row {
            kind: RowKind::Dir,
            branch: "├ ".into(),
            label: "lta/vtp-sdk".into(),
            hits: vec![8, 9, 10],
            ..Row::default()
        };
        let line = dir_line(&row, WIDTH);
        assert_eq!(plain(&line), "   ├ lta/vtp-sdk");
        let style_of = |text: &str| {
            let span = line.spans.iter().find(|s| s.content == text);
            span.unwrap_or_else(|| panic!("{text} in {line:?}")).style
        };
        assert_eq!(style_of("├ "), SUBTLE);
        assert_eq!(style_of("lta/"), INACTIVE);
        assert_eq!(style_of("vtp-"), INK);
        assert_eq!(style_of("sdk"), CLAUDE.add_modifier(Modifier::BOLD));
    }

    #[test]
    fn a_long_match_is_cut_and_keeps_its_marks() {
        let row = Row {
            kind: RowKind::Dir,
            branch: "└ ".into(),
            label: format!("{}/ábc", "x".repeat(60)),
            hits: vec![61],
            ..Row::default()
        };
        let line = dir_line(&row, 20);
        assert_eq!(cell_len(&line), 20);
        assert!(plain(&line).ends_with('…'), "{line:?}");
        // A mark on a character past the cut is dropped, never misplaced
        assert!(line
            .spans
            .iter()
            .all(|s| s.style != CLAUDE.add_modifier(Modifier::BOLD)));
    }

    #[test]
    fn a_create_row_names_the_folder_from_home() {
        let (_t, mut p, _) = home_picker();
        open_documents(&mut p, Root::New);
        type_text(&mut p, "Meu-Projeto");
        let shown = p.shown();
        let lines = list_lines(&p, &shown, NOW, 20, WIDTH);
        let create = lines
            .iter()
            .map(plain)
            .find(|l| l.contains("create"))
            .unwrap();
        assert_eq!(create, "❯   + create ~/Documents/Meu-Projeto and start");
    }

    #[test]
    fn the_search_box_is_rounded_and_holds_the_placeholder() {
        let lines = search_box("", "Search folders…", true, WIDTH);
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(text.len(), 3);
        assert!(
            text[0].starts_with('╭') && text[0].ends_with('╮'),
            "{}",
            text[0]
        );
        assert!(text[1].starts_with("│ ❯ ▌Search folders…"), "{}", text[1]);
        assert!(text[1].ends_with('│'));
        assert!(
            text[2].starts_with('╰') && text[2].ends_with('╯'),
            "{}",
            text[2]
        );
        assert!(lines.iter().all(|l| cell_len(l) == WIDTH));
        assert_eq!(lines[0].spans[0].style, SOFT);
        assert_eq!(lines[1].spans[1].style, CLAUDE);
        // Not focused: no cursor; typed text replaces the placeholder
        assert!(plain(&search_box("", "x", false, WIDTH)[1]).starts_with("│ ❯ x "));
        assert!(plain(&search_box("ap", "x", true, WIDTH)[1]).starts_with("│ ❯ ap▌ "));
        // Text too long for the box keeps its end, where the cursor is
        let long = search_box(&"y".repeat(80), "x", true, 20);
        assert!(long.iter().all(|l| cell_len(l) == 20));
        assert!(plain(&long[1]).ends_with("yy▌ │"), "{}", plain(&long[1]));
    }

    #[test]
    fn the_placeholder_offers_a_name_once_a_tree_is_open() {
        let (_t, mut p, _) = home_picker();
        assert_eq!(placeholder(&p), "Search folders…");
        p.key(Key::Right);
        assert_eq!(
            placeholder(&p),
            "Search folders, or type a new folder's name…"
        );
    }

    /// The hints of the selected row, each as `"{key} to {does}"`.
    fn hint_texts(p: &mut Picker) -> Vec<String> {
        let row = p.selected();
        hints(row.as_ref(), p)
            .iter()
            .map(|(key, does)| format!("{key} to {does}"))
            .collect()
    }

    #[test]
    fn hints_follow_the_selected_row() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Down);
        p.key(Key::Right);
        p.key(Key::Down);
        assert_eq!(
            hint_texts(&mut p),
            ["Enter to start here", "→ to open", "Esc to cancel"]
        );
        let joined = |p: &mut Picker| plain(&hints_line(&hints(p.selected().as_ref(), p), 80));
        assert_eq!(
            joined(&mut p),
            " Enter to start here · → to open · Esc to cancel"
        );
        p.key(Key::Up);
        assert_eq!(joined(&mut p), " Enter/← to close · Esc to cancel");
        p.key(Key::Left);
        assert_eq!(joined(&mut p), " Enter/→ to open · Esc to cancel");
        p.key(Key::Right);
        p.key(Key::Down);
        type_text(&mut p, "zz-new");
        assert_eq!(p.selected().unwrap().kind, RowKind::Create);
        assert!(joined(&mut p).contains("Enter to create & start"));
        let (_t, mut p, _) = home_picker();
        p.key(Key::Right);
        p.key(Key::Down);
        assert!(joined(&mut p).contains("Enter to pick as parent · ← to back"));
        let mut p = picker(false);
        past_entries(&mut p);
        assert!(joined(&mut p).contains("Enter to new session · → to sessions"));
    }

    #[test]
    fn left_is_hinted_only_while_nothing_is_typed() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Right);
        type_text(&mut p, "Doc");
        assert_eq!(p.selected().unwrap().label, "Documents");
        assert_eq!(
            hint_texts(&mut p),
            ["Enter to pick as parent", "Esc to cancel"]
        );
        p.key(Key::Up);
        p.key(Key::Up);
        assert_eq!(p.selected().unwrap().kind, RowKind::Root);
        assert_eq!(hint_texts(&mut p), ["Enter to close", "Esc to cancel"]);
        // An open folder of the tree, the text filtering the history only
        let (_t, mut p, _) = home_picker();
        for key in [Key::Right, Key::Down, Key::Right] {
            p.key(key);
        }
        p.query = "x".into();
        assert_eq!(
            hint_texts(&mut p),
            ["Enter to pick as parent", "Esc to cancel"]
        );
        p.query.clear();
        assert_eq!(
            hint_texts(&mut p),
            ["Enter to pick as parent", "← to back", "Esc to cancel"]
        );
    }

    #[test]
    fn hints_line_is_grey_and_cut_with_an_ellipsis() {
        let keys = [("Enter", "start here"), ("Esc", "cancel")];
        let line = hints_line(&keys, 80);
        assert_eq!(plain(&line), " Enter to start here · Esc to cancel");
        let style_of = |text: &str| line.spans.iter().find(|s| s.content == text).unwrap().style;
        assert_eq!(style_of("Enter to start here"), INACTIVE);
        assert_eq!(style_of(" · "), SUBTLE);
        let cut = hints_line(&keys, 12);
        assert_eq!(plain(&cut), " Enter to s…");
    }

    #[test]
    fn keys_line_cuts_a_long_line_with_an_ellipsis() {
        let keys = [("↑↓", "move"), ("Enter", "new / resume"), ("Esc", "")];
        assert_eq!(
            plain(&keys_line(&keys, 40)),
            " ↑↓ move · Enter new / resume · Esc"
        );
        assert_eq!(plain(&keys_line(&keys, 12)), " ↑↓ move · …");
    }
}
