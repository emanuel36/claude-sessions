//! The folder list that starts a session: folders by last use, each opening its last sessions.
//!
//! The Alt+n panel shows it under a search line; the landing page shows it under its header.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::rc::Rc;

use serde::Deserialize;

use crate::core::paths;
use crate::shell;
use crate::ui::search::{self, Hit, Searchable};
use crate::ui::tree::{self, Disk};

mod draw;
pub mod keys;

pub use crate::ui::picker::draw::{
    entry_hints, hints, hints_line, list_lines, placeholder, screen, search_box, ENTRY_KEYS,
};

/// How many of a folder's sessions open under it at once; the rest scroll into view.
pub const SHOWN: usize = 5;
/// The narrowest the list draws well: the landing page keeps this much beside its statistics,
/// and the golden panels draw at it.
pub const WIDTH: usize = 48;

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

/// An entry to the disk, opened by its key; while open it heads the list with its own tree of
/// the home folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Root {
    /// Picks the parent of a folder to create.
    New,
    /// Opens a folder Claude never ran in.
    Open,
}

/// By folder, when a Claude session last ran in or below it.
type Recent = HashMap<String, f64>;

/// What a [`Recent`] was read from: how many history folders, and the bits of the newest one's
/// last use.
type History = (usize, u64);

/// A line of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A typed folder path, to start a session in.
    Path {
        /// The path as typed.
        path: String,
    },
    /// A folder Claude ran in.
    Folder {
        /// The folder's path.
        path: String,
        /// Its index in the folders.
        index: usize,
    },
    /// One of a folder's sessions.
    Session {
        /// The folder's path.
        path: String,
        /// The folder's index in the folders.
        folder: usize,
        /// The session's index in its folder's sessions.
        index: usize,
        /// The session id, which the cursor follows when the sessions move.
        id: String,
        /// Whether it is the last of its folder on show.
        end: bool,
    },
    /// An entry to the disk, opening its folders as a tree.
    Root {
        /// Which entry.
        root: Root,
        /// What it says in grey after its name.
        hint: String,
    },
    /// A folder on disk, in an entry's tree or found by its search.
    Dir {
        /// The entry it belongs to.
        root: Root,
        /// The folder's path.
        path: String,
        /// The tree lines before its name.
        branch: String,
        /// Its name with a `/`, or for a match its path below the folder searched.
        label: String,
        /// The byte offsets in `label` of the characters a search matched.
        hits: Vec<usize>,
    },
    /// A folder to create, then start a session in.
    Create {
        /// The entry it belongs to.
        root: Root,
        /// The folder's path.
        path: String,
        /// The path, the home folder shown as `~`.
        label: String,
    },
    /// A session of any folder that the typed text found.
    Hit {
        /// The session's index in the sessions searched.
        index: usize,
        /// Its folder's path.
        path: String,
        /// The session id.
        id: String,
        /// Whether it is the last on show with more found below.
        more_below: bool,
    },
}

impl Row {
    /// The entry an entry, disk or create row belongs to; `None` for a history row.
    pub(crate) fn root(&self) -> Option<Root> {
        match self {
            Row::Root { root, .. } | Row::Dir { root, .. } | Row::Create { root, .. } => {
                Some(*root)
            }
            _ => None,
        }
    }

    /// Whether `other` is the same line of the list, whatever its tree lines, hint, marks,
    /// scroll mark or place: a session is followed by its id, wherever it moved.
    pub(crate) fn same_line(&self, other: &Row) -> bool {
        match (self, other) {
            (Row::Path { path: a }, Row::Path { path: b })
            | (Row::Folder { path: a, .. }, Row::Folder { path: b, .. }) => a == b,
            (Row::Session { path: p, id: a, .. }, Row::Session { path: q, id: b, .. })
            | (Row::Hit { path: p, id: a, .. }, Row::Hit { path: q, id: b, .. }) => {
                p == q && a == b
            }
            (Row::Root { root: a, .. }, Row::Root { root: b, .. }) => a == b,
            (
                Row::Dir {
                    root: a, path: p, ..
                },
                Row::Dir {
                    root: b, path: q, ..
                },
            )
            | (
                Row::Create {
                    root: a, path: p, ..
                },
                Row::Create {
                    root: b, path: q, ..
                },
            ) => a == b && p == q,
            _ => false,
        }
    }

    /// The row's path; empty for an entry.
    #[cfg(test)]
    pub(crate) fn path(&self) -> &str {
        match self {
            Row::Root { .. } => "",
            Row::Path { path }
            | Row::Folder { path, .. }
            | Row::Session { path, .. }
            | Row::Dir { path, .. }
            | Row::Create { path, .. }
            | Row::Hit { path, .. } => path,
        }
    }
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
    /// Opens an entry to the disk, or closes it.
    Entry(Root),
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
    /// The folders read from disk, by entry: New folder's, then Open another folder's.
    disks: [Disk; 2],
    /// The entries on show, each with its tree, the last opened first.
    pub open: Vec<Root>,
    /// The tree folders showing their subfolders, by entry.
    pub dirs: BTreeSet<(Root, String)>,
    /// While typing in a tree: the entry and the folder searched below.
    pub anchor: Option<(Root, String)>,
    /// The sessions of every folder that the typed text searches; set with
    /// [`Picker::set_sessions`], which the search cache follows.
    sessions: Vec<Searchable>,
    /// How many times the sessions were set.
    sessions_gen: u64,
    /// How many of the sessions found show at once; the rest scroll into view.
    pub found_shown: usize,
    /// How many of the sessions found are scrolled past.
    found_scroll: usize,
    /// The row the cursor was on when the first character was typed in a tree, to go back to
    /// once the text is cleared.
    typed_at: Option<Row>,
    /// By folder, when a Claude session last ran in or below it, with the history it was read
    /// from.
    recent: RefCell<(Option<History>, Rc<Recent>)>,
    /// The sessions found, with a hash of the text and the sessions' generation.
    found: RefCell<(Option<u64>, Rc<[Hit]>)>,
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
            disks: Default::default(),
            open: Vec::new(),
            dirs: BTreeSet::new(),
            anchor: None,
            sessions: Vec::new(),
            sessions_gen: 0,
            found_shown: SHOWN,
            found_scroll: 0,
            typed_at: None,
            recent: RefCell::default(),
            found: RefCell::default(),
        }
    }

    /// The sessions the typed text searches.
    pub fn sessions(&self) -> &[Searchable] {
        &self.sessions
    }

    /// Replaces the sessions the typed text searches; the next search looks in them.
    pub fn set_sessions(&mut self, sessions: Vec<Searchable>) {
        self.sessions = sessions;
        self.sessions_gen += 1;
    }

    /// The folders read from disk for `root`'s tree.
    fn disk(&self, root: Root) -> &Disk {
        &self.disks[root as usize]
    }

    /// Forgets each folder's subfolders read for the trees, keeping the searches' walks.
    pub fn forget_folders(&self) {
        self.disks.iter().for_each(Disk::forget_folders);
    }

    /// Forgets the searches' walks and matches in the trees.
    pub fn forget_walks(&self) {
        self.disks.iter().for_each(Disk::forget_search);
    }

    /// The folder searched below under `root`, while the text typed in its tree is not blank.
    fn searched(&self, root: Root) -> Option<&str> {
        let (anchored, dir) = self.anchor.as_ref()?;
        (*anchored == root && !self.query.trim().is_empty()).then_some(dir.as_str())
    }

    /// An open entry's row, then its tree, or the search below one of its folders.
    fn root_rows(&self, root: Root) -> Vec<Row> {
        let searched = self.searched(root);
        let hint = match (searched, root) {
            (Some(dir), _) => format!("in {}/", tilde(dir, &self.home)),
            (None, Root::New) => "pick the parent, then type the name".to_owned(),
            (None, Root::Open) => "type to search below".to_owned(),
        };
        let mut out = vec![Row::Root { root, hint }];
        match searched {
            Some(dir) => self.search_rows(root, dir, &mut out),
            None => self.tree_rows(root, &self.recent(), &self.home, "", &mut out),
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
            let disk = self.disk(root);
            disk.search(dir, text, &|path| {
                disk.usage(path, recent.get(path).copied())
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
        let create = (relative && !name.is_empty() && free()).then(|| {
            let path = tree::join(dir, name);
            let label = tilde(&path, &self.home);
            Row::Create { root, path, label }
        });
        let create_last = root == Root::Open && create.is_some();
        if root == Root::New {
            out.extend(create.clone());
        }
        let count = hits.len();
        out.extend(hits.into_iter().enumerate().map(|(i, hit)| {
            let last = i + 1 == count && !create_last;
            Row::Dir {
                root,
                path: tree::join(dir, &hit.rel),
                branch: if last { "└ " } else { "├ " }.to_owned(),
                label: hit.rel,
                hits: hit.hits,
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
    /// * `recent`: by folder, when a Claude session last ran in or below it.
    /// * `dir`: the folder whose subfolders to list.
    /// * `prefix`: the tree lines of the levels above.
    /// * `out`: where the rows go.
    fn tree_rows(&self, root: Root, recent: &Recent, dir: &str, prefix: &str, out: &mut Vec<Row>) {
        let disk = self.disk(root);
        let children = disk.children(dir);
        let mut names: Vec<&str> = children.iter().map(String::as_str).collect();
        names.sort_by_cached_key(|name| {
            let path = tree::join(dir, name);
            disk.usage(&path, recent.get(&path).copied())
        });
        let count = names.len();
        for (i, name) in names.into_iter().enumerate() {
            let last = i + 1 == count;
            let key = (root, tree::join(dir, name));
            out.push(Row::Dir {
                root,
                path: key.1.clone(),
                branch: format!("{prefix}{}", if last { "└ " } else { "├ " }),
                label: format!("{name}/"),
                hits: Vec::new(),
            });
            if self.dirs.contains(&key) {
                let deeper = format!("{prefix}{}", if last { "  " } else { "│ " });
                self.tree_rows(root, recent, &key.1, &deeper, out);
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
    /// cursor if it is open, else the one holding it (the home folder on an entry). `None` on a
    /// history row: the text then only filters the history.
    pub fn anchor_of(&self, row: &Row) -> Option<(Root, String)> {
        let (root, dir) = match row {
            Row::Root { root, .. } => (*root, self.home.clone()),
            Row::Dir { root, path, .. } if self.dirs.contains(&(*root, path.clone())) => {
                (*root, path.clone())
            }
            Row::Dir { root, path, .. } => (
                *root,
                Path::new(path)
                    .parent()
                    .map_or_else(|| self.home.clone(), |p| p.to_string_lossy().into_owned()),
            ),
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
        if !self.open.contains(&root) {
            self.open.insert(0, root);
        }
        let home = Path::new(&self.home);
        let mut at = Path::new(path).parent();
        while let Some(dir) = at.filter(|d| d.starts_with(home) && *d != home) {
            self.dirs.insert((root, dir.to_string_lossy().into_owned()));
            at = dir.parent();
        }
    }

    /// Opens a closed entry on top of the list, the cursor on its row; text already typed then
    /// searches its tree, as if typed on its row. Closes an open one, as
    /// [`Picker::close_entry`] does.
    fn toggle(&mut self, root: Root) {
        if self.open.contains(&root) {
            let row = self.selected();
            self.close_entry(root, row.as_ref());
            return;
        }
        self.open.insert(0, root);
        let typed = !self.query.is_empty();
        if typed {
            self.anchor = Some((root, self.home.clone()));
            self.found_scroll = 0;
        }
        let shown = self.shown();
        let entry = shown.iter().position(|r| is_entry(r, root));
        if typed {
            self.typed_at = entry.and_then(|i| shown.get(i)).cloned();
        }
        let at = if typed {
            self.typing_cursor(&shown)
        } else {
            None
        };
        self.cursor = at.or(entry).unwrap_or(0);
    }

    /// Takes an entry and its tree off the list and drops the folders read for it, so it opens
    /// anew; text typed in its tree goes on filtering the history. The cursor stays on `row`,
    /// the row it was on, if still listed, else goes to the other entry, else to the first
    /// history row.
    fn close_entry(&mut self, root: Root, row: Option<&Row>) {
        self.open.retain(|&r| r != root);
        self.disk(root).clear();
        if self.anchor.as_ref().is_some_and(|(r, _)| *r == root) {
            self.anchor = None;
            self.typed_at = None;
        }
        let shown = self.shown();
        self.cursor = row
            .and_then(|row| shown.iter().position(|r| r.same_line(row)))
            .or_else(|| shown.iter().position(|r| matches!(r, Row::Root { .. })))
            .or_else(|| shown.iter().position(|r| r.root().is_none()))
            .unwrap_or(0);
    }

    /// Left in a tree: closes an open folder, else moves to the folder holding it, or to the
    /// entry; on an entry, closes it.
    fn close(&mut self, row: &Row) {
        match row {
            Row::Root { root, .. } => self.close_entry(*root, Some(row)),
            Row::Dir { root, path, .. } => {
                if self.dirs.remove(&(*root, path.clone())) {
                    return;
                }
                let parent = Path::new(path).parent().unwrap_or(Path::new(""));
                if parent == Path::new(&self.home) {
                    self.select(|r| is_entry(r, *root));
                } else {
                    self.select_dir(*root, &parent.to_string_lossy());
                }
            }
            _ => {}
        }
    }

    /// The sessions the typed text finds, ranked; none while the text is typed in a tree.
    /// Searched again only when the text or the sessions change.
    pub fn found(&self) -> Rc<[Hit]> {
        if self.anchor.is_some() || self.query.split_whitespace().next().is_none() {
            return Rc::default();
        }
        let mut state = DefaultHasher::new();
        (self.sessions_gen, &self.query).hash(&mut state);
        let key = Some(state.finish());
        let mut cached = self.found.borrow_mut();
        if cached.0 != key {
            *cached = (key, search::hits(&self.sessions, &self.query).into());
        }
        Rc::clone(&cached.1)
    }

    /// The rows of the sessions `found` that are on show.
    fn hit_rows(&self, found: &[Hit]) -> Vec<Row> {
        let count = found.len();
        let start = self.found_scroll.min(count);
        let stop = (start + self.found_shown).min(count);
        found[start..stop]
            .iter()
            .enumerate()
            .filter_map(|(i, hit)| {
                let session = self.sessions.get(hit.index)?;
                Some(Row::Hit {
                    index: hit.index,
                    path: session.path.clone(),
                    id: session.id.clone(),
                    more_below: start + i + 1 == stop && stop < count,
                })
            })
            .collect()
    }

    /// The history's rows: a typed path first, then the folders that match, with the sessions
    /// of the open ones below.
    fn history_rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        let typed = self.query.trim();
        if (typed.starts_with('/') || typed.starts_with('~')) && (self.is_dir)(typed) {
            out.push(Row::Path {
                path: typed.to_owned(),
            });
        }
        for (index, folder) in self.folders.iter().enumerate() {
            if !matches(folder, &self.query) {
                continue;
            }
            out.push(Row::Folder {
                path: folder.path.clone(),
                index,
            });
            if self.expanded.contains(&folder.path) {
                let count = folder.sessions.len();
                let start = self.scroll.get(&folder.path).map_or(0, |&s| s.min(count));
                let stop = (start + SHOWN).min(count);
                out.extend((start..stop).map(|session| Row::Session {
                    path: folder.path.clone(),
                    folder: index,
                    index: session,
                    id: folder.sessions[session].id.clone(),
                    end: session + 1 == stop,
                }));
            }
        }
        out
    }

    /// The rows on show: the open entries, each followed by its tree, then the history: a typed
    /// path, the sessions found and the folders. The cursor is kept on one of them.
    pub fn shown(&mut self) -> Vec<Row> {
        // Sessions that went away under the same text must not leave the block scrolled past them
        let found = self.found();
        let last_start = found.len().saturating_sub(self.found_shown);
        self.found_scroll = self.found_scroll.min(last_start);
        let mut out: Vec<Row> = self.open.iter().flat_map(|&r| self.root_rows(r)).collect();
        let mut history = self.history_rows();
        // The sessions found go after a typed path, before the folders
        let at = usize::from(matches!(history.first(), Some(Row::Path { .. })));
        history.splice(at..at, self.hit_rows(&found));
        out.extend(history);
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
        match (key, row.as_ref()) {
            (Key::Enter, Some(Row::Root { root, .. })) => self.close_entry(*root, row.as_ref()),
            // In the new-folder tree, Enter on a folder picks it as the parent: it opens there,
            // the cursor on it, ready for the name
            (
                Key::Enter,
                Some(Row::Dir {
                    root: Root::New,
                    path,
                    ..
                }),
            ) => {
                self.open_to(Root::New, path);
            }
            // On a match, opens the tree down to it and the match itself, and leaves the search
            (Key::Right, Some(Row::Dir { root, path, .. })) if self.searched(*root).is_some() => {
                self.open_to(*root, path);
            }
            (Key::Right, Some(Row::Dir { root, path, .. })) => {
                self.dirs.insert((*root, path.clone()));
            }
            (Key::Left | Key::Backspace, Some(row @ (Row::Root { .. } | Row::Dir { .. })))
                if self.query.is_empty() =>
            {
                self.close(row);
            }
            _ => return self.history_key(key, row),
        }
        self.shown();
        None
    }

    /// Leaves the search and opens `root`'s tree down to the folder `path` and `path` itself,
    /// the cursor on it.
    fn open_to(&mut self, root: Root, path: &str) {
        self.end_search();
        self.reveal(root, path);
        self.dirs.insert((root, path.to_owned()));
        self.select_dir(root, path);
    }

    /// Applies a key the trees leave: opens or closes an entry, else acts on the history rows;
    /// returns the row to open on Enter, else `None`.
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
                if let Some(Row::Folder { path, .. }) = row {
                    self.expanded.insert(path);
                }
            }
            Key::Left => {
                // From a session, back to its folder, which then closes
                if let Some(Row::Folder { path, .. } | Row::Session { path, .. }) = row {
                    self.expanded.remove(&path);
                    self.scroll.remove(&path);
                    self.select(|r| matches!(r, Row::Folder { path: p, .. } if *p == path));
                }
            }
            Key::Backspace | Key::Char(_) => self.typed(key, row.as_ref()),
            Key::Entry(root) => self.toggle(root),
        }
        self.shown();
        None
    }

    /// Types a character into the search, or deletes its last one.
    ///
    /// The first character decides what the text searches, from the row under the cursor
    /// ([`Picker::anchor_of`]); clearing the text returns to the tree as it was.
    fn typed(&mut self, key: Key, row: Option<&Row>) {
        self.found_scroll = 0;
        if let Key::Char(c) = key {
            if self.query.is_empty() {
                self.anchor = row.and_then(|r| self.anchor_of(r));
                self.typed_at = row.cloned();
            }
            self.query.push(c);
        } else {
            self.query.pop();
            if self.query.is_empty() && self.anchor.take().is_some() {
                // Back on that row wherever it is now: rows may have come above it since
                if let Some(at) = self.typed_at.take() {
                    self.select(|r| r.same_line(&at));
                }
                return;
            }
        }
        let shown = self.shown();
        if let Some(i) = self.typing_cursor(&shown) {
            self.cursor = i;
        }
    }

    /// Where the cursor goes after a key typed into the search: on the typed path's row when
    /// listed, else as [`Picker::tree_cursor`] puts it in a searched tree; with no tree
    /// searched, on the first folder, else the first session found, else the first history
    /// row.
    fn typing_cursor(&self, shown: &[Row]) -> Option<usize> {
        // A typed path that exists is what the text names, wherever it was typed
        let first = |pick: fn(&Row) -> bool| shown.iter().position(pick);
        first(|r| matches!(r, Row::Path { .. })).or_else(|| {
            match self.anchor.as_ref().map(|(root, _)| *root) {
                None => first(|r| matches!(r, Row::Folder { .. }))
                    .or_else(|| first(|r| matches!(r, Row::Hit { .. })))
                    .or_else(|| first(|r| r.root().is_none())),
                Some(root) => self.tree_cursor(root, shown),
            }
        })
    }

    /// Where the cursor goes in `root`'s search: on a match one level down whose name starts
    /// with the text (ignoring case), else on the first row below the entry (the create row
    /// under the new-folder entry, the first match under the other), else on the entry. `None`
    /// while the text is blank.
    fn tree_cursor(&self, root: Root, shown: &[Row]) -> Option<usize> {
        self.searched(root)?;
        let text = self.query.trim().to_lowercase();
        let mine = |r: &Row| r.root() == Some(root) && !matches!(r, Row::Root { .. });
        // A match's label is its path below the folder searched: one level down holds no `/`
        let near = |r: &Row| match r {
            Row::Dir {
                root: at, label, ..
            } => *at == root && !label.contains('/') && label.to_lowercase().starts_with(&text),
            _ => false,
        };
        shown
            .iter()
            .position(near)
            .or_else(|| shown.iter().position(mine))
            .or_else(|| shown.iter().position(|r| is_entry(r, root)))
    }

    /// Leaves the search: the text and what it searched below are cleared.
    pub fn end_search(&mut self) {
        self.query.clear();
        self.found_scroll = 0;
        self.anchor = None;
    }

    /// Puts the cursor on the folder `path` of an entry's tree.
    fn select_dir(&mut self, root: Root, path: &str) {
        self.select(
            |r| matches!(r, Row::Dir { root: at, path: p, .. } if *at == root && p == path),
        );
    }

    /// Scrolls a folder's sessions, or the sessions found, when the cursor would leave them with
    /// more that way.
    ///
    /// Returns whether it scrolled, in which case the cursor stays on its line.
    fn scrolls(&mut self, row: Option<&Row>, key: Key) -> bool {
        let (path, folder, index, end) = match row {
            Some(Row::Hit {
                index, more_below, ..
            }) => return self.scrolls_found(*index, *more_below, key),
            Some(Row::Session {
                path,
                folder,
                index,
                end,
                ..
            }) => (path, *folder, *index, *end),
            _ => return false,
        };
        let count = self.folders.get(folder).map_or(0, |f| f.sessions.len());
        let start = self.scroll.get(path).copied().unwrap_or(0);
        if key == Key::Down && end && start + SHOWN < count {
            self.scroll.insert(path.clone(), start + 1);
            return true;
        }
        if key == Key::Up && start > 0 && index == start {
            self.scroll.insert(path.clone(), start - 1);
            return true;
        }
        false
    }

    /// Scrolls the sessions found from the one at `index`, as [`Picker::scrolls`] does;
    /// `more_below` tells whether it is the last on show with more found below.
    fn scrolls_found(&mut self, index: usize, more_below: bool, key: Key) -> bool {
        if key == Key::Down && more_below {
            self.found_scroll += 1;
            return true;
        }
        let first = self.found().get(self.found_scroll).map(|hit| hit.index);
        if key == Key::Up && self.found_scroll > 0 && first == Some(index) {
            self.found_scroll -= 1;
            return true;
        }
        false
    }
}

/// Whether `row` is the row of the entry `root`.
fn is_entry(row: &Row, root: Root) -> bool {
    matches!(row, Row::Root { root: at, .. } if *at == root)
}

/// Tells whether a folder's path holds every word of the query, ignoring case.
fn matches(folder: &Folder, query: &str) -> bool {
    let path = folder.path.to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| path.contains(word))
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

/// The tmux `new-window` arguments that open a row: resume its session, or start one; `None`
/// for an entry, which opens its tree instead.
pub fn command(row: &Row) -> Option<Vec<String>> {
    let (path, id) = match row {
        Row::Hit { path, id, .. } | Row::Session { path, id, .. } => {
            (path.as_str(), Some(id.as_str()))
        }
        Row::Root { .. } => return None,
        Row::Path { path }
        | Row::Folder { path, .. }
        | Row::Dir { path, .. }
        | Row::Create { path, .. } => (path.as_str(), None),
    };
    Some(match id {
        Some(id) => vec![
            "-c".to_owned(),
            path.to_owned(),
            format!("claude --resume {}", shell::quote(id)),
        ],
        None => vec!["-c".to_owned(), expand_user(path), "claude".to_owned()],
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeSet;
    use std::fs::{self, File};
    use std::rc::Rc;
    use std::time::{Duration, UNIX_EPOCH};

    use crate::ui::picker::{command, is_entry, tilde, Folder, Key, Picker, Root, Row, Session};
    use crate::ui::search::Searchable;

    pub(super) const NOW: f64 = 1_700_000_000.0;

    pub(super) fn sessions() -> Vec<Session> {
        (0..7)
            .map(|i| Session {
                id: format!("s{i}"),
                title: format!("task {i}"),
                last: NOW - 3600.0 * f64::from(i),
                cost: (i == 0).then_some(1.5),
            })
            .collect()
    }

    pub(super) fn folders() -> Vec<Folder> {
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

    pub(super) fn picker(is_dir: bool) -> Picker {
        let mut p = Picker::new(Box::new(move |_| is_dir));
        p.folders = folders();
        p
    }

    pub(super) fn set(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    pub(super) fn no_dirs(_: &str) -> bool {
        false
    }

    /// A picker over `folders`, the cursor on row `cursor`, the folders in `expanded` open.
    pub(super) fn list(folders: &[Folder], cursor: usize, expanded: &BTreeSet<String>) -> Picker {
        let mut p = Picker::new(Box::new(no_dirs));
        p.folders = folders.to_vec();
        p.cursor = cursor;
        p.expanded = expanded.clone();
        p
    }

    pub(super) fn session_ids(shown: &[Row], folders: &[Folder]) -> Vec<String> {
        let id = |r: &Row| match r {
            Row::Session { folder, index, .. } => {
                Some(folders[*folder].sessions[*index].id.clone())
            }
            _ => None,
        };
        shown.iter().filter_map(id).collect()
    }

    /// The row of the folder `path`, at `index` in the folders.
    fn folder_row(path: &str, index: usize) -> Row {
        Row::Folder {
            path: path.into(),
            index,
        }
    }

    /// The entry `row` is the row of, if it is an entry's.
    fn entry_of(row: &Row) -> Option<Root> {
        match row {
            Row::Root { root, .. } => Some(*root),
            _ => None,
        }
    }

    /// The label of a disk row; empty for any other row.
    pub(super) fn label(row: &Row) -> &str {
        match row {
            Row::Dir { label, .. } => label,
            _ => "",
        }
    }

    #[test]
    fn rows_list_folders_and_open_a_folders_last_five_sessions() {
        let fs = folders();
        let shown = list(&fs, 0, &set(&["/w/api"])).history_rows();
        let session = |index| Row::Session {
            path: "/w/api".into(),
            folder: 0,
            index,
            id: format!("s{index}"),
            end: index == 4,
        };
        let mut want = vec![folder_row("/w/api", 0)];
        want.extend((0..5).map(session));
        want.push(folder_row("/w/web", 1));
        assert_eq!(shown, want);
        assert_eq!(session_ids(&shown, &fs), ["s0", "s1", "s2", "s3", "s4"]);
    }

    #[test]
    fn a_folder_stays_closed_until_expanded() {
        let shown = list(&folders(), 0, &set(&[])).history_rows();
        assert_eq!(shown, [folder_row("/w/api", 0), folder_row("/w/web", 1)]);
    }

    #[test]
    fn typing_filters_by_every_word_and_a_typed_folder_comes_first() {
        let history = |is_dir, query: &str| {
            let mut p = picker(is_dir);
            p.query = query.into();
            p.history_rows()
        };
        assert_eq!(history(false, "W E"), [folder_row("/w/web", 1)]);
        let shown = history(true, "~/x");
        assert_eq!(shown[0], Row::Path { path: "~/x".into() });
        assert!(history(false, "/nope").is_empty());
    }

    #[test]
    fn the_picker_moves_opens_and_types() {
        let mut p = picker(false);
        assert_eq!(p.key(Key::Right), None);
        assert!(p.expanded.contains("/w/api"));
        p.key(Key::Down);
        let opened = p.key(Key::Enter);
        assert!(matches!(opened, Some(Row::Session { index: 0, .. })));
        assert_eq!(p.folders[0].sessions[0].id, "s0");
        p.key(Key::Left);
        assert_eq!(p.cursor, 0);
        assert!(p.expanded.is_empty());
        p.key(Key::Char('w'));
        assert_eq!((p.query.as_str(), p.cursor), ("w", 0));
        let selected = p.selected().unwrap();
        assert_eq!(command(&selected).unwrap(), ["-c", "/w/api", "claude"]);
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
        assert!(matches!(p.selected(), Some(Row::Folder { .. })));
        assert!(p.expanded.is_empty());
    }

    #[test]
    fn command_quotes_session_ids() {
        let row = Row::Session {
            path: "/w/api".into(),
            folder: 0,
            index: 0,
            id: "a b".into(),
            end: false,
        };
        assert_eq!(
            command(&row).unwrap(),
            ["-c", "/w/api", "claude --resume 'a b'"]
        );
    }

    #[test]
    fn command_expands_the_home_of_a_typed_path() {
        let row = Row::Path { path: "~/x".into() };
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            command(&row).unwrap(),
            ["-c".to_owned(), format!("{home}/x"), "claude".to_owned()]
        );
    }

    /// A picker whose home is a tempdir holding `Documents/`, `workspace/vtp/` and `Music/`,
    /// all changed at the same time so the tree lists them by name.
    pub(super) fn home_picker() -> (tempfile::TempDir, Picker, String) {
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
        let tree_row = |r| match r {
            Row::Dir {
                root: at,
                label,
                branch,
                ..
            } if at == root => Some((label, branch)),
            _ => None,
        };
        p.shown().into_iter().filter_map(tree_row).collect()
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
        // Left closes the entry, as Enter and its key do
        for close in [Key::Left, Key::Enter, Key::Entry(Root::Open)] {
            p.key(Key::Entry(Root::Open));
            assert!(!listed(&mut p));
            let made = t.path().join("Later");
            fs::create_dir(&made).unwrap();
            assert!(!listed(&mut p), "read once and kept while open");
            p.key(close);
            p.key(Key::Entry(Root::Open));
            assert!(listed(&mut p), "{close:?}");
            p.key(Key::Entry(Root::Open));
            fs::remove_dir(&made).unwrap();
        }
    }

    #[test]
    fn the_entries_are_hidden_and_the_cursor_starts_on_the_first_folder() {
        let (_t, mut p, _) = home_picker();
        assert!(p.shown().iter().all(|r| r.root().is_none()));
        assert_eq!(p.selected(), Some(folder_row("/w/api", 0)));
    }

    #[test]
    fn an_entry_key_opens_the_entry_on_top_with_the_cursor_on_it() {
        for root in [Root::New, Root::Open] {
            let (_t, mut p, _) = home_picker();
            // Wherever the cursor was
            p.key(Key::Down);
            assert_eq!(p.key(Key::Entry(root)), None);
            let shown = p.shown();
            assert!(is_entry(&shown[0], root));
            assert_eq!(p.cursor, 0);
            assert_eq!(tree(&mut p, root).len(), 3);
        }
        // The last opened goes on top of the other
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::Open));
        p.key(Key::Entry(Root::New));
        assert_eq!(p.open, [Root::New, Root::Open]);
        let entries: Vec<_> = p.shown().iter().filter_map(entry_of).collect();
        assert_eq!(entries, [Root::New, Root::Open]);
        assert_eq!(p.cursor, 0);
        p.key(Key::Entry(Root::New));
        p.key(Key::Entry(Root::New));
        assert_eq!(p.open, [Root::New, Root::Open]);
    }

    #[test]
    fn an_entry_key_again_takes_the_entry_off_the_list() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::Open));
        p.key(Key::Down);
        p.key(Key::Right);
        p.key(Key::Entry(Root::Open));
        assert!(p.open.is_empty());
        assert!(p.shown().iter().all(|r| r.root().is_none()));
        assert_eq!(p.selected().unwrap().path(), "/w/api");
    }

    #[test]
    fn closing_an_entry_keeps_the_cursor_on_its_row_else_on_the_other_entry() {
        let (_t, mut p, _) = home_picker();
        // On a history row, which stays listed
        p.key(Key::Entry(Root::Open));
        p.key(Key::Entry(Root::New));
        let web = p.shown().iter().position(|r| r.path() == "/w/web").unwrap();
        p.cursor = web;
        p.key(Key::Entry(Root::Open));
        assert_eq!(p.selected().unwrap().path(), "/w/web");
        // On the closed entry's row: to the other entry
        p.key(Key::Entry(Root::Open));
        p.key(Key::Enter);
        assert_eq!(p.open, [Root::New]);
        assert!(is_entry(&p.selected().unwrap(), Root::New));
        // In its tree, with no other entry: to the first history row
        p.key(Key::Down);
        p.key(Key::Entry(Root::New));
        assert_eq!(p.selected().unwrap().path(), "/w/api");
    }

    #[test]
    fn closing_an_entry_keeps_the_folders_read_for_the_other() {
        let (t, mut p, _) = home_picker();
        let listed = |p: &mut Picker, root| tree(p, root).iter().any(|(l, _)| l == "Later/");
        p.key(Key::Entry(Root::New));
        p.key(Key::Entry(Root::Open));
        fs::create_dir(t.path().join("Later")).unwrap();
        p.key(Key::Entry(Root::Open));
        p.key(Key::Entry(Root::Open));
        assert!(listed(&mut p, Root::Open), "read anew once reopened");
        assert!(!listed(&mut p, Root::New), "kept while open");
    }

    #[test]
    fn opening_an_entry_with_text_typed_searches_its_tree() {
        let (_t, mut p, home) = home_picker();
        type_text(&mut p, "Doc");
        assert_eq!(p.anchor, None);
        p.key(Key::Entry(Root::Open));
        assert_eq!(p.anchor, Some((Root::Open, home.clone())));
        let shown = p.shown();
        let hint = "in ~/".to_owned();
        assert_eq!(
            shown[0],
            Row::Root {
                root: Root::Open,
                hint
            }
        );
        assert_eq!(label(&p.selected().unwrap()), "Documents");
        // Clearing the text goes back to the entry, as when typed on it
        for _ in 0..3 {
            p.key(Key::Backspace);
        }
        assert_eq!((p.query.as_str(), &p.anchor), ("", &None));
        assert!(is_entry(&p.selected().unwrap(), Root::Open));
        // Under New folder the text names the folder to create
        let (_t, mut p, home) = home_picker();
        type_text(&mut p, "Meu-Projeto");
        p.key(Key::Entry(Root::New));
        let create = Row::Create {
            root: Root::New,
            path: format!("{home}/Meu-Projeto"),
            label: "~/Meu-Projeto".into(),
        };
        assert_eq!(p.selected().as_ref(), Some(&create));
        // ← does nothing while text is typed in a tree
        p.key(Key::Left);
        assert_eq!(p.selected(), Some(create));
        assert_eq!(p.open, [Root::New]);
    }

    #[test]
    fn clearing_the_text_finds_its_row_after_rows_came_above_it() {
        let (_t, mut p, home) = home_picker();
        p.key(Key::Entry(Root::New));
        p.key(Key::Down);
        p.key(Key::Right);
        type_text(&mut p, "x");
        assert_eq!(p.anchor, Some((Root::New, format!("{home}/Documents"))));
        // Rows come above the tree searched
        p.open.insert(0, Root::Open);
        p.key(Key::Backspace);
        let row = p.selected().unwrap();
        let documents = format!("{home}/Documents");
        assert!(
            matches!(&row, Row::Dir { root: Root::New, path, .. } if *path == documents),
            "{row:?}"
        );
    }

    #[test]
    fn closing_the_entry_searched_leaves_the_text_to_the_history() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::Open));
        type_text(&mut p, "a");
        assert!(p.anchor.is_some());
        p.key(Key::Entry(Root::Open));
        assert_eq!((p.query.as_str(), &p.anchor), ("a", &None));
        assert_eq!(p.selected().unwrap().path(), "/w/api");
    }

    #[test]
    fn an_entry_opens_as_a_tree_of_the_home_folder() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::Open));
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
        p.key(Key::Entry(Root::Open));
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
        p.key(Key::Entry(Root::Open));
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
        p.key(Key::Entry(Root::Open));
        assert_eq!(tree(&mut p, Root::Open)[0].0, "Documents/");
        // Were the change read again, Music would now come first
        let later = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let music = File::open(t.path().join("Music")).unwrap();
        music.set_modified(later).unwrap();
        assert_eq!(tree(&mut p, Root::Open)[0].0, "Documents/");
        p.key(Key::Left);
        p.key(Key::Entry(Root::Open));
        assert_eq!(
            tree(&mut p, Root::Open)[0].0,
            "Music/",
            "read anew once reopened"
        );
    }

    #[test]
    fn right_on_a_folder_opens_it_and_left_closes_it() {
        let (_t, mut p, home) = home_picker();
        for key in [Key::Entry(Root::Open), Key::Down, Key::Down, Key::Down] {
            p.key(key);
        }
        assert_eq!(label(&p.selected().unwrap()), "workspace/");
        p.key(Key::Right);
        let next = Row::Dir {
            root: Root::Open,
            path: format!("{home}/workspace/vtp"),
            branch: "  └ ".into(),
            label: "vtp/".into(),
            hits: Vec::new(),
        };
        assert_eq!(p.shown()[p.cursor + 1], next);
        p.key(Key::Left);
        assert_eq!(label(&p.selected().unwrap()), "workspace/");
        assert!(!tree(&mut p, Root::Open).iter().any(|(l, _)| l == "vtp/"));
        p.key(Key::Left);
        assert!(is_entry(&p.selected().unwrap(), Root::Open));
        assert!(p.open.contains(&Root::Open));
        // Backspace with nothing typed does what Left does
        for key in [Key::Down, Key::Down, Key::Down, Key::Right, Key::Backspace] {
            p.key(key);
        }
        assert!(p.dirs.is_empty());
        p.key(Key::Backspace);
        assert!(matches!(p.selected(), Some(Row::Root { .. })));
        p.key(Key::Backspace);
        assert!(p.open.is_empty());
    }

    #[test]
    fn enter_on_an_entry_closes_it() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::New));
        assert_eq!(p.key(Key::Enter), None);
        assert!(p.open.is_empty());
        assert!(tree(&mut p, Root::New).is_empty());
        assert_eq!(p.selected().unwrap().path(), "/w/api");
    }

    #[test]
    fn enter_on_an_open_another_folder_row_returns_it() {
        let (_t, mut p, home) = home_picker();
        for key in [Key::Entry(Root::Open), Key::Down, Key::Down] {
            p.key(key);
        }
        let row = p.key(Key::Enter).unwrap();
        let music = format!("{home}/Music");
        assert!(
            matches!(&row, Row::Dir { root: Root::Open, path, .. } if *path == music),
            "{row:?}"
        );
        assert_eq!(command(&row).unwrap()[1], music);
    }

    #[test]
    fn enter_on_a_new_folder_row_picks_it_as_parent() {
        let (_t, mut p, home) = home_picker();
        p.key(Key::Entry(Root::New));
        p.key(Key::Down);
        assert_eq!(p.key(Key::Enter), None);
        let documents = format!("{home}/Documents");
        assert_eq!(p.dirs, BTreeSet::from([(Root::New, documents.clone())]));
        let row = p.selected().unwrap();
        assert_eq!((p.cursor, row.path()), (1, documents.as_str()));
    }

    /// Types `text` into the search, one key per character.
    pub(super) fn type_text(p: &mut Picker, text: &str) {
        for c in text.chars() {
            p.key(Key::Char(c));
        }
    }

    /// Opens `root` and, in its tree, `Documents/`, leaving the cursor on it.
    pub(super) fn open_documents(p: &mut Picker, root: Root) {
        p.key(Key::Entry(root));
        p.key(Key::Down);
        assert_eq!(label(&p.selected().unwrap()), "Documents/");
        p.key(Key::Right);
    }

    /// Every row under `root` but its entry row, as whether it creates its folder, and its path.
    fn below(p: &mut Picker, root: Root) -> Vec<(bool, String)> {
        p.shown()
            .into_iter()
            .filter(|r| r.root() == Some(root) && !is_entry(r, root))
            .map(|r| (matches!(r, Row::Create { .. }), r.path().to_owned()))
            .collect()
    }

    /// Whether a row under `root` creates its folder.
    pub(super) fn creates(p: &mut Picker, root: Root) -> bool {
        below(p, root).iter().any(|(create, _)| *create)
    }

    #[test]
    fn typing_with_no_entry_open_filters_the_history() {
        let (_t, mut p, _) = home_picker();
        type_text(&mut p, "api");
        assert_eq!(p.anchor, None);
        let shown = p.shown();
        let history: Vec<_> = shown.iter().filter(|r| r.root().is_none()).collect();
        assert_eq!(history, [&folder_row("/w/api", 0)]);
        assert_eq!(p.selected().unwrap().path(), "/w/api");
    }

    #[test]
    fn typing_in_the_tree_searches_below_the_current_folder() {
        let (_t, mut p, home) = home_picker();
        for key in [Key::Entry(Root::Open), Key::Down, Key::Down, Key::Down] {
            p.key(key);
        }
        assert_eq!(label(&p.selected().unwrap()), "workspace/");
        p.key(Key::Right);
        type_text(&mut p, "VT");
        let workspace = format!("{home}/workspace");
        assert_eq!(p.anchor, Some((Root::Open, workspace.clone())));
        let shown = p.shown();
        let entry = shown.iter().find(|r| is_entry(r, Root::Open));
        let hint = format!("in {}/", tilde(&workspace, &p.home));
        assert_eq!(hint, "in ~/workspace/");
        let root = Root::Open;
        assert_eq!(entry, Some(&Row::Root { root, hint }));
        let first = shown.iter().position(|r| matches!(r, Row::Dir { .. }));
        let first = first.unwrap();
        assert_eq!(label(&shown[first]), "vtp");
        assert_eq!(shown[first].path(), format!("{workspace}/vtp"));
        assert_eq!(p.cursor, first);
    }

    #[test]
    fn a_new_name_offers_create_last_under_open_and_first_under_new() {
        for root in [Root::Open, Root::New] {
            let (t, mut p, home) = home_picker();
            fs::create_dir(t.path().join("Documents/old-meu-projeto")).unwrap();
            open_documents(&mut p, root);
            type_text(&mut p, "Meu-Projeto");
            let create = (true, format!("{home}/Documents/Meu-Projeto"));
            let hit = (false, format!("{home}/Documents/old-meu-projeto"));
            let selected = p.selected().unwrap();
            if root == Root::Open {
                assert_eq!(below(&mut p, root), [hit, create]);
                assert!(matches!(selected, Row::Dir { .. }));
            } else {
                assert_eq!(below(&mut p, root), [create.clone(), hit]);
                let picked = (
                    matches!(selected, Row::Create { .. }),
                    selected.path().to_owned(),
                );
                assert_eq!(picked, create);
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
            p.key(Key::Entry(Root::New));
            type_text(&mut p, &typed);
            let row = p.key(Key::Enter).unwrap();
            assert_eq!(row, Row::Path { path: typed });
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
            assert!(!creates(&mut p, Root::New), "{typed}");
        }
    }

    #[test]
    fn create_respects_case() {
        let (t, mut p, _) = home_picker();
        fs::create_dir(t.path().join("Documents/meu-projeto")).unwrap();
        open_documents(&mut p, Root::New);
        type_text(&mut p, "Meu-Projeto");
        assert!(creates(&mut p, Root::New));
        for _ in 0.."Meu-Projeto".len() {
            p.key(Key::Backspace);
        }
        type_text(&mut p, "meu-projeto");
        assert!(!creates(&mut p, Root::New));
    }

    #[test]
    fn text_with_a_slash_only_offers_create() {
        let (t, mut p, home) = home_picker();
        fs::create_dir(t.path().join("Documents/abc")).unwrap();
        open_documents(&mut p, Root::Open);
        type_text(&mut p, "a/b");
        assert_eq!(
            below(&mut p, Root::Open),
            [(true, format!("{home}/Documents/a/b"))]
        );
    }

    #[test]
    fn only_a_relative_name_offers_create() {
        for text in ["~/Documents/X", "/tmp/x", "..", "a/../b", ".", "./a", "~"] {
            let (_t, mut p, _) = home_picker();
            open_documents(&mut p, Root::New);
            type_text(&mut p, text);
            assert!(!creates(&mut p, Root::New), "{text}");
        }
        let (_t, mut p, home) = home_picker();
        open_documents(&mut p, Root::New);
        type_text(&mut p, "a/b");
        assert_eq!(
            below(&mut p, Root::New),
            [(true, format!("{home}/Documents/a/b"))]
        );
    }

    #[test]
    fn spaces_around_the_text_are_trimmed() {
        let (_t, mut p, home) = home_picker();
        open_documents(&mut p, Root::New);
        type_text(&mut p, "  Meu-Projeto ");
        let create = Row::Create {
            root: Root::New,
            path: format!("{home}/Documents/Meu-Projeto"),
            label: "~/Documents/Meu-Projeto".into(),
        };
        assert_eq!(p.selected(), Some(create));
    }

    #[test]
    fn right_on_a_match_opens_the_tree_down_to_it() {
        let (_t, mut p, home) = home_picker();
        p.key(Key::Entry(Root::Open));
        type_text(&mut p, "vtp");
        assert_eq!(p.anchor, Some((Root::Open, home.clone())));
        assert_eq!(label(&p.selected().unwrap()), "workspace/vtp");
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
            (label(&row), row.path()),
            ("vtp/", format!("{workspace}/vtp").as_str())
        );
    }

    #[test]
    fn clearing_the_text_returns_to_the_tree() {
        let (_t, mut p, _) = home_picker();
        for key in [Key::Entry(Root::Open), Key::Down, Key::Down, Key::Down] {
            p.key(key);
        }
        let before = (tree(&mut p, Root::Open), p.cursor);
        type_text(&mut p, "vt");
        assert_ne!(tree(&mut p, Root::Open), before.0);
        p.key(Key::Backspace);
        p.key(Key::Backspace);
        assert_eq!((p.query.as_str(), &p.anchor), ("", &None));
        assert_eq!((tree(&mut p, Root::Open), p.cursor), before);
        assert_eq!(label(&p.selected().unwrap()), "workspace/");
    }

    /// A session of `path` to search, its title its own, used `ago` seconds before `NOW`.
    pub(super) fn searchable(
        id: &str,
        title: &str,
        path: &str,
        ago: f64,
        prompts: &[&str],
    ) -> Searchable {
        let prompts: Vec<String> = prompts.iter().map(|p| (*p).to_owned()).collect();
        let (id, title, path) = (id.to_owned(), title.to_owned(), path.to_owned());
        Searchable::new(id, title, path, NOW - ago, false, true, &prompts)
    }

    /// The test picker, searching two LinkedIn sessions of `/w/cv` and one about something else.
    pub(super) fn linkedin_picker() -> Picker {
        let mut p = picker(false);
        p.set_sessions(vec![
            searchable("b", "Banner options", "/w/cv", 60.0, &["a linkedin banner"]),
            searchable("x", "Unrelated", "/w/cv", 30.0, &["nothing here"]),
            searchable("a", "Post on LinkedIn", "/w/cv", 7200.0, &["write it"]),
        ]);
        p
    }

    /// The ids of the hit rows on show.
    fn hit_ids(p: &mut Picker) -> Vec<String> {
        p.shown().into_iter().filter_map(hit_id).collect()
    }

    /// The session id of a hit row.
    pub(crate) fn hit_id(row: Row) -> Option<String> {
        match row {
            Row::Hit { id, .. } => Some(id),
            _ => None,
        }
    }

    #[test]
    fn the_search_is_kept_until_the_text_or_the_sessions_change() {
        let mut p = linkedin_picker();
        p.query = "linkedin".into();
        let first = p.found();
        assert_eq!(first.len(), 2);
        assert!(Rc::ptr_eq(&first, &p.found()), "kept");
        // As many sessions, one of them another
        let mut sessions = p.sessions().to_vec();
        sessions[1] = searchable("y", "LinkedIn too", "/w/cv", 10.0, &[]);
        p.set_sessions(sessions);
        assert_eq!(p.found().len(), 3);
        p.query = "banner".into();
        assert_eq!(p.found().len(), 1);
    }

    #[test]
    fn a_folder_matching_by_path_keeps_the_cursor() {
        let mut p = picker(false);
        p.folders.push(Folder {
            path: "/w/vtp".into(),
            name: "vtp".into(),
            last: NOW - 9000.0,
            count: 1,
            spend: 0.0,
            sessions: Vec::new(),
        });
        p.set_sessions(vec![searchable(
            "s",
            "Build",
            "/w/api",
            60.0,
            &["fix the vtp build"],
        )]);
        type_text(&mut p, "vtp");
        assert_eq!(hit_ids(&mut p), ["s"]);
        assert_eq!(p.selected(), Some(folder_row("/w/vtp", 2)));
        assert_eq!(p.key(Key::Enter), Some(folder_row("/w/vtp", 2)));
    }

    #[test]
    fn the_block_shows_found_shown_hits_and_scrolls() {
        let mut p = picker(false);
        p.set_sessions(
            (0..7)
                .map(|i| {
                    searchable(
                        &format!("h{i}"),
                        "linkedin",
                        "/w/cv",
                        f64::from(i) * 60.0,
                        &[],
                    )
                })
                .collect(),
        );
        assert_eq!(p.found_shown, 5);
        type_text(&mut p, "linkedin");
        assert_eq!(hit_ids(&mut p), ["h0", "h1", "h2", "h3", "h4"]);
        let first = p.cursor;
        for _ in 0..4 {
            p.key(Key::Down);
        }
        assert_eq!(p.selected().and_then(hit_id).as_deref(), Some("h4"));
        // More below: the cursor stays on the last line and the block scrolls
        p.key(Key::Down);
        assert_eq!(p.cursor, first + 4);
        assert_eq!(hit_ids(&mut p), ["h1", "h2", "h3", "h4", "h5"]);
        assert_eq!(p.selected().and_then(hit_id).as_deref(), Some("h5"));
        for _ in 0..4 {
            p.key(Key::Up);
        }
        assert_eq!(
            (p.cursor, p.selected().and_then(hit_id)),
            (first, Some("h1".to_owned()))
        );
        p.key(Key::Up);
        assert_eq!(p.cursor, first);
        assert_eq!(hit_ids(&mut p), ["h0", "h1", "h2", "h3", "h4"]);
    }

    #[test]
    fn the_block_survives_hits_shrinking_under_the_same_text() {
        let mut p = picker(false);
        let all: Vec<Searchable> = (0..7)
            .map(|i| searchable(&format!("h{i}"), "linkedin", "/w/cv", f64::from(i), &[]))
            .collect();
        p.set_sessions(all.clone());
        type_text(&mut p, "linkedin");
        for _ in 0..6 {
            p.key(Key::Down);
        }
        assert_eq!(hit_ids(&mut p), ["h2", "h3", "h4", "h5", "h6"]);
        p.set_sessions(all[..2].to_vec());
        assert_eq!(hit_ids(&mut p).len(), 2);
    }

    #[test]
    fn clearing_the_text_drops_the_block() {
        let mut p = linkedin_picker();
        p.found_shown = 1;
        type_text(&mut p, "linkedin");
        p.key(Key::Down);
        assert_eq!(hit_ids(&mut p), ["b"]);
        for _ in 0.."linkedin".len() {
            p.key(Key::Backspace);
        }
        assert!(hit_ids(&mut p).is_empty());
        assert_eq!(p.found_scroll, 0);
        assert!(p.selected().is_some());
    }

    #[test]
    fn text_typed_in_a_tree_finds_no_sessions() {
        let (_t, mut p, _) = home_picker();
        p.set_sessions(linkedin_picker().sessions);
        p.key(Key::Entry(Root::Open));
        type_text(&mut p, "linkedin");
        assert!(p.anchor.is_some());
        assert!(p.found().is_empty());
        assert!(hit_ids(&mut p).is_empty());
    }

    #[test]
    fn enter_on_a_hit_resumes_it_in_its_folder() {
        let row = Row::Hit {
            index: 0,
            path: "/p".into(),
            id: "id".into(),
            more_below: false,
        };
        assert_eq!(command(&row).unwrap(), ["-c", "/p", "claude --resume id"]);
    }
}
