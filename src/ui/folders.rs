//! The folders Claude ran in, read from its history, for the Alt+n panel and the landing page.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::claude::history::HistorySession;
use crate::claude::transcript::{transcript_path_in, Titles};
use crate::core::paths;
use crate::ui::picker::{Folder, Session};

/// A session worth listing: its id, title, folder and last use.
#[derive(Debug, Clone)]
struct Found {
    id: String,
    title: String,
    path: String,
    last: f64,
}

/// The sessions worth listing, with their title and folder, kept for one history generation.
///
/// Finding titles stats every transcript; the landing page asks every second, so the scan is
/// kept until the history reader's generation changes. The titles found go to disk, for the
/// next process to start from, when `titles` is stored.
pub struct Scan {
    titles: Titles,
    config: Option<PathBuf>,
    generation: Option<u64>,
    found: Vec<Found>,
}

impl Scan {
    /// Scans with `titles`, finding transcripts in Claude Code's config folder.
    pub fn new(titles: Titles) -> Self {
        Self {
            titles,
            config: None,
            generation: None,
            found: Vec::new(),
        }
    }

    /// Like [`Scan::new`], finding transcripts under the config folder `config`.
    pub fn in_dir(titles: Titles, config: PathBuf) -> Self {
        Self {
            config: Some(config),
            ..Self::new(titles)
        }
    }

    /// Returns the folders that still exist, last used first, with their latest sessions.
    ///
    /// # Arguments
    /// * `history`: the sessions of the history reader, newest first.
    /// * `generation`: the history reader's generation; the same one is not scanned again.
    /// * `costs`: each session's spend over the last 7 days, by id.
    /// * `opened`: the ids of the sessions open now; they count, but are not offered to resume.
    pub fn load(
        &mut self,
        history: &[HistorySession],
        generation: u64,
        costs: &HashMap<String, f64>,
        opened: &HashSet<String>,
    ) -> Vec<Folder> {
        if self.generation != Some(generation) {
            self.found = self.scan(history);
            self.generation = Some(generation);
        }
        // By path, in the order first seen, so folders used at the same time keep that order
        let mut index: HashMap<&str, usize> = HashMap::new();
        let mut folders: Vec<Folder> = Vec::new();
        for s in &self.found {
            let at = *index.entry(&s.path).or_insert_with(|| {
                folders.push(Folder {
                    path: s.path.clone(),
                    name: name(&s.path),
                    last: s.last,
                    count: 0,
                    spend: 0.0,
                    sessions: Vec::new(),
                });
                folders.len() - 1
            });
            let folder = &mut folders[at];
            let cost = costs.get(&s.id).copied();
            folder.last = folder.last.max(s.last);
            folder.count += 1;
            folder.spend += cost.unwrap_or(0.0);
            if !opened.contains(&s.id) {
                folder.sessions.push(Session {
                    id: s.id.clone(),
                    title: s.title.clone(),
                    last: s.last,
                    cost,
                });
            }
        }
        for folder in &mut folders {
            folder.sessions.sort_by(|a, b| b.last.total_cmp(&a.last));
        }
        folders.sort_by(|a, b| b.last.total_cmp(&a.last));
        folders
    }

    /// The sessions with a title, its own or its first prompt, whose folder still exists.
    fn scan(&mut self, history: &[HistorySession]) -> Vec<Found> {
        let config = self.config.clone().unwrap_or_else(paths::config_dir);
        let mut found = Vec::new();
        for s in history {
            let mut title = self
                .titles
                .get(&s.id, &transcript_path_in(&config, &s.id, &s.cwd));
            if title.is_empty() {
                title.clone_from(&s.first_prompt);
            }
            // Only commands were typed: nothing to come back to
            if title.is_empty() || !Path::new(&s.cwd).is_dir() {
                continue;
            }
            let path = s.cwd.trim_end_matches('/');
            found.push(Found {
                id: s.id.clone(),
                title,
                path: if path.is_empty() { "/" } else { path }.to_owned(),
                last: s.last,
            });
        }
        self.titles.save();
        found
    }
}

/// A folder's name: its last path component, or the whole path when it has none.
fn name(path: &str) -> String {
    match path.rsplit('/').next() {
        Some(last) if !last.is_empty() => last.to_owned(),
        _ => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use crate::claude::history::HistorySession;
    use crate::claude::transcript::{transcript_path_in, Titles};
    use crate::ui::folders::Scan;

    fn session(id: &str, cwd: &Path, last: f64, prompt: &str) -> HistorySession {
        HistorySession {
            id: id.into(),
            cwd: cwd.to_string_lossy().into_owned(),
            last,
            first_prompt: prompt.into(),
            prompts: Vec::new(),
        }
    }

    /// A scan reading transcripts from `tmp/config`, and two folders `a` and `b` in `tmp`.
    fn setup() -> (TempDir, Scan) {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("a")).unwrap();
        fs::create_dir(tmp.path().join("b")).unwrap();
        let titles = Titles::in_dir(false, tmp.path().join("state"));
        let scan = Scan::in_dir(titles, tmp.path().join("config"));
        (tmp, scan)
    }

    fn no_costs() -> HashMap<String, f64> {
        HashMap::new()
    }

    #[test]
    fn load_orders_folders_by_last_use_and_skips_gone_folders() {
        let (tmp, mut scan) = setup();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        let history = [
            session("1", &a, 400.0, "one"),
            session("2", &b, 300.0, "two"),
            session("3", &b, 200.0, "three"),
            session("4", &tmp.path().join("gone"), 900.0, "lost"),
            // Only commands were typed: nothing to come back to
            session("5", &a, 50.0, ""),
        ];
        let folders = scan.load(&history, 1, &no_costs(), &HashSet::new());
        let names: Vec<_> = folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        let titles: Vec<_> = folders[1]
            .sessions
            .iter()
            .map(|s| s.title.as_str())
            .collect();
        assert_eq!(titles, ["two", "three"]);
        assert_eq!((folders[0].count, folders[1].count), (1, 2));
    }

    #[test]
    fn load_lists_a_folders_sessions_newest_first() {
        let (tmp, mut scan) = setup();
        let a = tmp.path().join("a");
        let history = [
            session("1", &a, 100.0, "old"),
            session("2", &a, 300.0, "new"),
        ];
        let folders = scan.load(&history, 1, &no_costs(), &HashSet::new());
        assert_eq!(folders.len(), 1);
        let titles: Vec<_> = folders[0]
            .sessions
            .iter()
            .map(|s| s.title.as_str())
            .collect();
        assert_eq!(titles, ["new", "old"]);
        assert_eq!(folders[0].last, 300.0);
    }

    #[test]
    fn the_folder_scan_is_kept_until_the_history_changes() {
        let (tmp, mut scan) = setup();
        let a = tmp.path().join("a");
        let history = [session("1", &a, 100.0, "one")];
        scan.load(&history, 1, &no_costs(), &HashSet::new());
        fs::remove_dir(&a).unwrap();
        // The same generation is not scanned again, but the costs are new
        let costs = HashMap::from([("1".to_owned(), 2.0)]);
        let folders = scan.load(&history, 1, &costs, &HashSet::new());
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].spend, 2.0);
        assert_eq!(folders[0].sessions[0].cost, Some(2.0));
        assert!(scan.load(&history, 2, &costs, &HashSet::new()).is_empty());
    }

    #[test]
    fn open_sessions_count_but_are_not_offered() {
        let (tmp, mut scan) = setup();
        let a = tmp.path().join("a");
        let history = [
            session("1", &a, 300.0, "open"),
            session("2", &a, 100.0, "old"),
        ];
        let costs = HashMap::from([("1".to_owned(), 1.5), ("2".to_owned(), 0.5)]);
        let opened = HashSet::from(["1".to_owned()]);
        let folders = scan.load(&history, 1, &costs, &opened);
        assert_eq!((folders[0].count, folders[0].spend), (2, 2.0));
        let ids: Vec<_> = folders[0].sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["2"]);
        assert_eq!(folders[0].last, 300.0);
    }

    #[test]
    fn a_transcript_title_wins_over_the_first_prompt() {
        let (tmp, mut scan) = setup();
        let a = tmp.path().join("a");
        let cwd = a.to_string_lossy().into_owned();
        let path = transcript_path_in(&tmp.path().join("config"), "1", &cwd);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"type\":\"x\",\"customTitle\":\"named\"}\n").unwrap();
        let history = [session("1", &a, 100.0, "prompt")];
        let folders = scan.load(&history, 1, &no_costs(), &HashSet::new());
        assert_eq!(folders[0].sessions[0].title, "named");
    }

    #[test]
    fn a_trailing_slash_is_dropped_and_the_root_keeps_its_path_as_name() {
        let (tmp, mut scan) = setup();
        let a = format!("{}/", tmp.path().join("a").display());
        let history = [
            session("1", Path::new(&a), 200.0, "here"),
            session("2", Path::new("/"), 100.0, "root"),
        ];
        let folders = scan.load(&history, 1, &no_costs(), &HashSet::new());
        assert_eq!(folders[0].path, a.trim_end_matches('/'));
        assert_eq!(folders[0].name, "a");
        assert_eq!(
            (folders[1].path.as_str(), folders[1].name.as_str()),
            ("/", "/")
        );
    }
}
