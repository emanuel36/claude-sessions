//! The disk folders the folder list opens as a tree, and the search below one of them.
//!
//! Folders are read once per panel and kept: the panel lives a few seconds, and reading again on
//! every key would make typing lag on a large home.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::Path;

/// How deep a search goes below the folder it starts from.
pub const DEPTH: usize = 6;
/// The most folders a search looks at, so a huge tree cannot stall the panel.
pub const SCAN_LIMIT: usize = 40_000;
/// The most matches a search lists.
pub const HITS: usize = 50;

/// How recently a folder was used, as a sort key: folders with a Claude session in or below
/// them first, newest session first; then the rest, newest change on disk first.
pub type Use = (u8, std::cmp::Reverse<i64>);

/// The sort key of `path`, given when a Claude session last ran in or below it.
pub fn usage(path: &str, session: Option<f64>) -> Use {
    match session {
        // Whole seconds are enough to order folders
        #[allow(clippy::cast_possible_truncation)]
        Some(last) => (0, std::cmp::Reverse(last as i64)),
        None => {
            let changed = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
            (1, std::cmp::Reverse(changed))
        }
    }
}

/// When a Claude session last ran in or below each folder, from the history's folders.
pub fn sessions_below<'a>(folders: impl Iterator<Item = (&'a str, f64)>) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    for (path, last) in folders {
        let mut at = Some(Path::new(path));
        while let Some(dir) = at {
            let entry = out
                .entry(dir.to_string_lossy().into_owned())
                .or_insert(last);
            *entry = entry.max(last);
            at = dir.parent();
        }
    }
    out
}

/// A folder a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Its path relative to the folder searched, with `/` between components.
    pub rel: String,
    /// How many components `rel` has.
    pub depth: usize,
    /// Byte offsets in `rel` of the matched characters.
    pub hits: Vec<usize>,
}

/// A folder found by a walk: its path relative to the walk's start, and its depth.
type Found = (String, usize);

/// The folders read from disk, by folder, and the searches made below them.
#[derive(Debug, Default)]
pub struct Disk {
    children: RefCell<HashMap<(String, bool), Vec<String>>>,
    /// By folder, its sort key from its change on disk.
    changed: RefCell<HashMap<String, Use>>,
    below: RefCell<HashMap<(String, bool), Vec<Found>>>,
    /// By folder searched and lowercased text, the matches ranked.
    hits: RefCell<HashMap<(String, String), Vec<Hit>>>,
}

/// Tells whether a folder is left out: hidden ones unless asked for, and build trees.
fn skipped(name: &str, hidden: bool) -> bool {
    (!hidden && name.starts_with('.')) || name == "node_modules" || name == "target"
}

/// The subfolders of `dir`, by name, sorted ignoring case. Symlinks to folders are listed
/// only when `links` is set, so a walk never follows one. An unreadable `dir` has none.
fn read(dir: &str, hidden: bool, links: bool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| {
            if links {
                e.path().is_dir()
            } else {
                e.file_type().is_ok_and(|t| t.is_dir())
            }
        })
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| !skipped(name, hidden))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// Joins a folder and a relative path.
pub fn join(dir: &str, rel: &str) -> String {
    Path::new(dir).join(rel).to_string_lossy().into_owned()
}

impl Disk {
    /// Forgets everything read, so the next call reads the disk again.
    pub fn clear(&self) {
        self.forget_folders();
        self.forget_search();
    }

    /// Forgets each folder's subfolders and change on disk, keeping the searches' walks.
    pub fn forget_folders(&self) {
        self.children.borrow_mut().clear();
        self.changed.borrow_mut().clear();
    }

    /// Forgets the searches' walks and matches.
    pub fn forget_search(&self) {
        self.below.borrow_mut().clear();
        self.hits.borrow_mut().clear();
    }

    /// The sort key of `path`, given when a Claude session last ran in or below it; its change
    /// on disk is read once and kept.
    pub fn usage(&self, path: &str, session: Option<f64>) -> Use {
        if session.is_some() {
            return usage(path, session);
        }
        *self
            .changed
            .borrow_mut()
            .entry(path.to_owned())
            .or_insert_with(|| usage(path, None))
    }

    /// The subfolders of `dir`, hidden ones left out.
    pub fn children(&self, dir: &str) -> Vec<String> {
        let key = (dir.to_owned(), false);
        self.children
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| read(dir, false, true))
            .clone()
    }

    /// Every folder below `dir`, `DEPTH` levels at most, breadth first.
    fn below(&self, dir: &str, hidden: bool) -> Vec<Found> {
        let key = (dir.to_owned(), hidden);
        if let Some(found) = self.below.borrow().get(&key) {
            return found.clone();
        }
        let mut found = Vec::new();
        let mut queue = VecDeque::from([(String::new(), 0)]);
        while let Some((rel, depth)) = queue.pop_front() {
            if depth == DEPTH || found.len() >= SCAN_LIMIT {
                continue;
            }
            for name in read(&join(dir, &rel), hidden, false) {
                let child = if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                };
                found.push((child.clone(), depth + 1));
                queue.push_back((child, depth + 1));
            }
        }
        self.below.borrow_mut().insert(key, found.clone());
        found
    }

    /// The folders below `dir` whose name holds `query`, ignoring case: shallower first, then
    /// by `order` (smaller first), then the same name, names starting with it, and the rest.
    /// Ranked once per folder and text, and kept.
    pub fn search(&self, dir: &str, query: &str, order: &dyn Fn(&str) -> Use) -> Vec<Hit> {
        let wanted = query.trim().to_lowercase();
        if wanted.is_empty() {
            return Vec::new();
        }
        let key = (dir.to_owned(), wanted);
        if let Some(hits) = self.hits.borrow().get(&key) {
            return hits.clone();
        }
        let hits = self.rank(dir, &key.1, order);
        self.hits.borrow_mut().insert(key, hits.clone());
        hits
    }

    /// The matches of [`Disk::search`] for the lowercased text `wanted`, ranked.
    fn rank(&self, dir: &str, wanted: &str, order: &dyn Fn(&str) -> Use) -> Vec<Hit> {
        let mut ranked: Vec<(Use, u8, Hit)> = self
            .below(dir, wanted.starts_with('.'))
            .into_iter()
            .filter_map(|(rel, depth)| {
                let start = rel.rfind('/').map_or(0, |i| i + 1);
                let name = rel[start..].to_lowercase();
                // Lowercasing can change byte lengths; ranks and marks then fall back to none
                let at = name.find(wanted)?;
                let rank = if name == wanted {
                    0
                } else if at == 0 {
                    1
                } else {
                    2
                };
                let hits = if name.len() == rel.len() - start {
                    (start + at..start + at + wanted.len()).collect()
                } else {
                    Vec::new()
                };
                let used = order(&join(dir, &rel));
                Some((used, rank, Hit { rel, depth, hits }))
            })
            .collect();
        ranked.sort_by(|(ua, ra, a), (ub, rb, b)| {
            a.depth
                .cmp(&b.depth)
                .then(ua.cmp(ub))
                .then(ra.cmp(rb))
                .then_with(|| a.rel.to_lowercase().cmp(&b.rel.to_lowercase()))
        });
        ranked
            .into_iter()
            .take(HITS)
            .map(|(_, _, hit)| hit)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::cmp::Reverse;
    use std::fs;
    use std::path::Path;

    use crate::ui::tree::{sessions_below, usage, Disk, Hit, Use};

    fn dirs(root: &Path, names: &[&str]) {
        for n in names {
            fs::create_dir_all(root.join(n)).unwrap();
        }
    }

    fn rels(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|h| h.rel.as_str()).collect()
    }

    fn flat(_: &str) -> Use {
        (1, Reverse(0))
    }

    fn root_of(t: &tempfile::TempDir) -> String {
        t.path().to_string_lossy().into_owned()
    }

    #[test]
    fn children_leave_out_hidden_and_build_folders() {
        let t = tempfile::tempdir().unwrap();
        dirs(t.path(), &["a", "B", ".git", "node_modules", "target"]);
        fs::write(t.path().join("f"), "").unwrap();
        assert_eq!(Disk::default().children(&root_of(&t)), ["a", "B"]);
    }

    #[test]
    fn search_ignores_case_and_lists_shallow_first() {
        let t = tempfile::tempdir().unwrap();
        dirs(t.path(), &["vtp", "lta/vtp-sdk", "x/y/VTP"]);
        let hits = Disk::default().search(&root_of(&t), "Vt", &flat);
        assert_eq!(rels(&hits), ["vtp", "lta/vtp-sdk", "x/y/VTP"]);
    }

    #[test]
    fn search_orders_by_use_within_a_level() {
        let t = tempfile::tempdir().unwrap();
        dirs(t.path(), &["core-vtp", "vtp"]);
        let order = |p: &str| {
            if p.ends_with("core-vtp") {
                (0, Reverse(1))
            } else {
                (1, Reverse(0))
            }
        };
        let hits = Disk::default().search(&root_of(&t), "vtp", &order);
        assert_eq!(rels(&hits), ["core-vtp", "vtp"]);
    }

    #[test]
    fn search_finds_hidden_only_when_asked() {
        let t = tempfile::tempdir().unwrap();
        dirs(t.path(), &[".cfg"]);
        let disk = Disk::default();
        assert_eq!(rels(&disk.search(&root_of(&t), ".c", &flat)), [".cfg"]);
        assert!(disk.search(&root_of(&t), "cfg", &flat).is_empty());
    }

    #[test]
    fn search_stops_at_six_levels() {
        let t = tempfile::tempdir().unwrap();
        dirs(
            t.path(),
            &["l1/l2/l3/l4/l5/needle", "m1/m2/m3/m4/m5/m6/m7/m8/needle"],
        );
        let hits = Disk::default().search(&root_of(&t), "needle", &flat);
        assert_eq!(rels(&hits), ["l1/l2/l3/l4/l5/needle"]);
    }

    #[test]
    fn search_marks_the_matched_letters() {
        let t = tempfile::tempdir().unwrap();
        dirs(t.path(), &["lta/vtp-sdk"]);
        let hits = Disk::default().search(&root_of(&t), "sdk", &flat);
        assert_eq!(hits[0].hits, [8, 9, 10]);
    }

    #[test]
    fn an_unreadable_folder_lists_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        dirs(t.path(), &["locked/inner", "sib"]);
        let locked = t.path().join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let unreadable = fs::read_dir(&locked).is_err();
        let disk = Disk::default();
        let kids = disk.children(&locked.to_string_lossy());
        let hits = disk.search(&root_of(&t), "s", &flat);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if !unreadable {
            return; // running as root
        }
        assert!(kids.is_empty());
        assert_eq!(rels(&hits), ["sib"]);
    }

    #[test]
    fn a_symlink_loop_ends() {
        let t = tempfile::tempdir().unwrap();
        dirs(t.path(), &["a"]);
        std::os::unix::fs::symlink("..", t.path().join("a/loop")).unwrap();
        let hits = Disk::default().search(&root_of(&t), "a", &flat);
        assert_eq!(rels(&hits), ["a"]);
    }

    #[test]
    fn a_name_changing_length_when_lowercased_matches() {
        let t = tempfile::tempdir().unwrap();
        dirs(t.path(), &["İstanbul"]);
        let hits = Disk::default().search(&root_of(&t), "stan", &flat);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel, "İstanbul");
    }

    #[test]
    fn a_second_identical_search_reuses_the_ranking() {
        let t = tempfile::tempdir().unwrap();
        dirs(t.path(), &["vtp", "lta/vtp-sdk"]);
        let calls = Cell::new(0);
        let order = |p: &str| {
            calls.set(calls.get() + 1);
            flat(p)
        };
        let disk = Disk::default();
        let first = disk.search(&root_of(&t), "vtp", &order);
        let after_first = calls.get();
        assert!(after_first > 0);
        assert_eq!(disk.search(&root_of(&t), " VTP ", &order), first);
        assert_eq!(calls.get(), after_first, "ranked once per folder and text");
    }

    #[test]
    fn forgetting_the_folders_keeps_the_walk_until_the_search_is_forgotten() {
        let t = tempfile::tempdir().unwrap();
        dirs(t.path(), &["vtp"]);
        let disk = Disk::default();
        let root = root_of(&t);
        assert_eq!(rels(&disk.search(&root, "vt", &flat)), ["vtp"]);
        assert_eq!(disk.children(&root), ["vtp"]);
        dirs(t.path(), &["vt2"]);
        disk.forget_folders();
        assert_eq!(disk.children(&root), ["vt2", "vtp"]);
        assert_eq!(rels(&disk.search(&root, "vt", &flat)), ["vtp"], "walk kept");
        disk.forget_search();
        assert_eq!(rels(&disk.search(&root, "vt", &flat)), ["vt2", "vtp"]);
    }

    #[test]
    fn a_folders_change_on_disk_is_read_once_until_forgotten() {
        let t = tempfile::tempdir().unwrap();
        let path = root_of(&t);
        let at = |secs| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let touch = |secs| {
            fs::File::open(&path)
                .unwrap()
                .set_modified(at(secs))
                .unwrap()
        };
        touch(1_000);
        let disk = Disk::default();
        assert_eq!(disk.usage(&path, None), (1, Reverse(1_000)));
        touch(2_000);
        assert_eq!(disk.usage(&path, None), (1, Reverse(1_000)), "kept");
        assert_eq!(disk.usage(&path, Some(5.0)), (0, Reverse(5)));
        disk.forget_folders();
        assert_eq!(disk.usage(&path, None), (1, Reverse(2_000)));
    }

    #[test]
    fn sessions_below_reaches_every_ancestor() {
        let map = sessions_below([("/w/a/b", 5.0), ("/w/c", 9.0)].into_iter());
        assert_eq!(map["/w"], 9.0);
        assert_eq!(map["/w/a"], 5.0);
    }

    #[test]
    fn usage_puts_sessions_before_disk_changes() {
        assert!(usage("/p", Some(1.0)) < usage("/nonexistent-q", None));
    }
}
