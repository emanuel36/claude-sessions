//! Claude Code's prompt history, grouped by session.
//!
//! The history only grows, so [`HistoryReader`] parses the lines added since its last reading.
//! The functions that need Claude Code's config folder or the state folder have a constructor
//! that takes them, so unit tests use temporary directories.

use std::collections::HashMap;
use std::fs::{self, File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths;

/// Claude Code's prompt history, in its config folder.
pub const HISTORY: &str = "history.jsonl";
/// Where the history read so far is kept between runs, in the state folder.
pub const STORED_HISTORY: &str = "history.json";
/// A title shows one line of a prompt at most; the rest need not be kept.
pub const PROMPT_CHARS: usize = 200;
/// The bytes before where a reading stopped, kept to tell the file is still the one read.
const MARK_BYTES: usize = 64;

/// A session from Claude Code's prompt history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistorySession {
    /// The session id.
    pub id: String,
    /// The folder of the session's latest prompt.
    pub cwd: String,
    /// When its latest prompt was typed, in seconds since the epoch.
    pub last: f64,
    /// Its first prompt that is not a command, cut to [`PROMPT_CHARS`] characters.
    pub first_prompt: String,
}

/// The sessions found so far, in the order they first appeared.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
struct Sessions {
    list: Vec<HistorySession>,
    #[serde(skip)]
    index: HashMap<String, usize>,
}

impl Sessions {
    /// Rebuilds the lookup by id, after the list was read from disk.
    fn reindex(&mut self) {
        self.index = self
            .list
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.clone(), i))
            .collect();
    }

    /// Adds the entry in a history line to its session; a line that is not one is skipped.
    fn add_line(&mut self, line: &[u8]) {
        let line = String::from_utf8_lossy(line);
        let Ok(Value::Object(entry)) = serde_json::from_str::<Value>(&line) else {
            return;
        };
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        };
        let (Some(id), Some(project)) = (text("sessionId"), text("project")) else {
            return;
        };
        let at = match self.index.get(id) {
            Some(&at) => at,
            None => {
                self.list.push(HistorySession {
                    id: id.to_owned(),
                    cwd: project.to_owned(),
                    last: 0.0,
                    first_prompt: String::new(),
                });
                self.index.insert(id.to_owned(), self.list.len() - 1);
                self.list.len() - 1
            }
        };
        let session = &mut self.list[at];
        let stamp = entry
            .get("timestamp")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            / 1000.0;
        session.last = session.last.max(stamp);
        project.clone_into(&mut session.cwd);
        let prompt = entry.get("display").and_then(Value::as_str).unwrap_or("");
        // A command such as /resume says nothing about the session
        if session.first_prompt.is_empty() && !prompt.starts_with('/') {
            session.first_prompt = prompt.chars().take(PROMPT_CHARS).collect();
        }
    }

    fn newest_first(&self) -> Vec<HistorySession> {
        let mut sorted = self.list.clone();
        // Stable, so sessions with the same time keep the order they appeared in
        sorted.sort_by(|a, b| b.last.total_cmp(&a.last));
        sorted
    }
}

/// Groups history lines by session, most recent first.
pub fn read_history(lines: &[&str]) -> Vec<HistorySession> {
    let mut sessions = Sessions::default();
    for line in lines {
        sessions.add_line(line.as_bytes());
    }
    sessions.newest_first()
}

/// Which file a reading was made of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileId {
    path: String,
    dev: u64,
    ino: u64,
}

/// What a history reading stopped at, and what it found until there. This is what is kept on
/// disk between runs.
#[derive(Debug, Serialize, Deserialize)]
struct Reading {
    file: FileId,
    offset: u64,
    /// The bytes just before `offset`.
    mark: Vec<u8>,
    sessions: Sessions,
}

impl Reading {
    fn start(file: FileId) -> Self {
        Self {
            file,
            offset: 0,
            mark: Vec::new(),
            sessions: Sessions::default(),
        }
    }

    /// Tells whether an open history is the file this reading stopped in, grown since.
    fn continues(&self, file: &mut File, meta: &Metadata, id: &FileId) -> bool {
        if self.file != *id || meta.len() < self.offset || self.mark.len() > MARK_BYTES {
            return false;
        }
        let Some(from) = self.offset.checked_sub(self.mark.len() as u64) else {
            return false;
        };
        let mut found = vec![0; self.mark.len()];
        file.seek(SeekFrom::Start(from)).is_ok()
            && file.read_exact(&mut found).is_ok()
            && found == self.mark
    }

    /// Parses the lines that are complete in `raw`, the bytes read from `offset` on.
    ///
    /// A line still being written waits for the next reading.
    fn add(&mut self, raw: &[u8]) {
        let Some(last_newline) = raw.iter().rposition(|&b| b == b'\n') else {
            return;
        };
        let complete = &raw[..=last_newline];
        // Split on newlines only: a prompt may hold other line breaks, such as U+2028
        for line in complete[..last_newline].split(|&b| b == b'\n') {
            self.sessions.add_line(line);
        }
        self.offset += complete.len() as u64;
        self.mark = complete[complete.len().saturating_sub(MARK_BYTES)..].to_vec();
    }
}

/// What tells that a history file is the same as when it was last read.
#[derive(Debug, PartialEq)]
struct Stamp {
    id: FileId,
    mtime: (i64, i64),
    size: u64,
}

/// The outcome of looking at the history file.
enum Outcome {
    /// It is as it was when last read.
    Unchanged,
    Sessions(Vec<HistorySession>),
}

/// The prompt history, read again only when its file changes.
///
/// A file replaced, shorter than what was read, or no longer ending the reading with the same
/// bytes is read again from the start.
#[derive(Debug)]
pub struct HistoryReader {
    stored: bool,
    config: Option<PathBuf>,
    state: Option<PathBuf>,
    stamp: Option<Stamp>,
    reading: Option<Reading>,
    sessions: Vec<HistorySession>,
    generation: u64,
}

impl HistoryReader {
    /// Starts with nothing read; `stored` keeps the reading in the state folder too, for a new
    /// process to start from. The screens that open often ask for it.
    pub fn new(stored: bool) -> Self {
        Self {
            stored,
            config: None,
            state: None,
            stamp: None,
            reading: None,
            sessions: Vec::new(),
            generation: 0,
        }
    }

    /// Like [`HistoryReader::new`], with the config and state folders given.
    pub fn with_dirs(stored: bool, config: PathBuf, state: PathBuf) -> Self {
        Self {
            config: Some(config),
            state: Some(state),
            ..Self::new(stored)
        }
    }

    /// Returns the parsed prompt history, newest first.
    ///
    /// Only the lines added since the last reading are parsed. A history that cannot be read
    /// is an empty one.
    pub fn load(&mut self) -> &[HistorySession] {
        match self.refresh() {
            Some(Outcome::Unchanged) => {}
            Some(Outcome::Sessions(sessions)) => self.set(sessions),
            None => {
                self.stamp = None;
                self.reading = None;
                self.set(Vec::new());
            }
        }
        &self.sessions
    }

    /// Returns what the last [`HistoryReader::load`] returned, without looking at the file.
    pub fn sessions(&self) -> &[HistorySession] {
        &self.sessions
    }

    /// Counts the readings that changed the sessions [`HistoryReader::load`] returned, so a
    /// caller can skip the work it does with them when the number is the same.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Takes the sessions of a reading, counting it when they differ from the last one's.
    fn set(&mut self, sessions: Vec<HistorySession>) {
        if sessions != self.sessions {
            self.sessions = sessions;
            self.generation += 1;
        }
    }

    fn config_dir(&self) -> PathBuf {
        self.config.clone().unwrap_or_else(paths::config_dir)
    }

    fn stored_path(&self) -> PathBuf {
        let state = self.state.clone().unwrap_or_else(paths::state_dir);
        state.join(STORED_HISTORY)
    }

    /// Reads what was added to the history; `None` when it cannot be read.
    fn refresh(&mut self) -> Option<Outcome> {
        let path = self.config_dir().join(HISTORY);
        let mut file = File::open(&path).ok()?;
        let meta = file.metadata().ok()?;
        let id = FileId {
            path: path.to_string_lossy().into_owned(),
            dev: meta.dev(),
            ino: meta.ino(),
        };
        let stamp = Stamp {
            id: id.clone(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            size: meta.len(),
        };
        if self.stamp.as_ref() == Some(&stamp) {
            return Some(Outcome::Unchanged);
        }
        let mut reading = self
            .reading
            .take()
            .or_else(|| self.load_stored())
            .filter(|r| r.continues(&mut file, &meta, &id))
            .unwrap_or_else(|| Reading::start(id));
        file.seek(SeekFrom::Start(reading.offset)).ok()?;
        let mut raw = Vec::new();
        file.read_to_end(&mut raw).ok()?;
        let before = reading.offset;
        reading.add(&raw);
        if self.stored && reading.offset != before {
            self.save(&reading);
        }
        let sessions = reading.sessions.newest_first();
        self.stamp = Some(stamp);
        self.reading = Some(reading);
        Some(Outcome::Sessions(sessions))
    }

    /// The reading kept on disk, or `None` when there is none or it does not parse.
    fn load_stored(&self) -> Option<Reading> {
        let bytes = fs::read(self.stored_path()).ok()?;
        let mut reading: Reading = serde_json::from_slice(&bytes).ok()?;
        reading.sessions.reindex();
        Some(reading)
    }

    /// Keeps a reading on disk; never fails.
    fn save(&self, reading: &Reading) {
        let Ok(json) = serde_json::to_vec(reading) else {
            return;
        };
        let state = self.state.clone().unwrap_or_else(paths::state_dir);
        // The next process starts over if this is lost
        if fs::create_dir_all(&state).is_ok() {
            let _ = paths::write_atomic(&state.join(STORED_HISTORY), &json, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::PathBuf;

    use serde_json::json;
    use tempfile::TempDir;

    use crate::claude::history::{
        read_history, HistoryReader, HistorySession, HISTORY, PROMPT_CHARS, STORED_HISTORY,
    };

    fn entry(id: &str, project: &str, t: u64, display: &str) -> String {
        json!({"sessionId": id, "project": project, "timestamp": t * 1000, "display": display})
            .to_string()
    }

    struct Env {
        tmp: TempDir,
    }

    impl Env {
        fn new() -> Self {
            Self {
                tmp: TempDir::new().unwrap(),
            }
        }

        fn config(&self) -> PathBuf {
            self.tmp.path().join("claude")
        }

        fn state(&self) -> PathBuf {
            self.tmp.path().join("state")
        }

        fn history(&self) -> PathBuf {
            self.config().join(HISTORY)
        }

        fn reader(&self, stored: bool) -> HistoryReader {
            HistoryReader::with_dirs(stored, self.config(), self.state())
        }

        /// Replaces the history with whole lines.
        fn write(&self, lines: &[String]) {
            fs::create_dir_all(self.config()).unwrap();
            let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
            fs::write(self.history(), text).unwrap();
        }

        fn append(&self, raw: &str) {
            let mut f = OpenOptions::new()
                .append(true)
                .open(self.history())
                .unwrap();
            f.write_all(raw.as_bytes()).unwrap();
        }
    }

    fn ids(sessions: &[HistorySession]) -> Vec<&str> {
        sessions.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn history_is_grouped_by_session_newest_first() {
        let lines = [
            entry("a", "/w/api", 5, "/resume"),
            entry("a", "/w/api", 10, "first"),
            entry("b", "/w/notes", 20, "hi"),
            "not json".to_owned(),
            json!({"display": "no session id", "project": "/w", "timestamp": 1}).to_string(),
            entry("a", "/w/api", 30, "second"),
        ];
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let sessions = read_history(&lines);
        let got: Vec<(&str, f64, &str)> = sessions
            .iter()
            .map(|s| (s.id.as_str(), s.last, s.first_prompt.as_str()))
            .collect();
        assert_eq!(got, [("a", 30.0, "first"), ("b", 20.0, "hi")]);
    }

    #[test]
    fn the_latest_prompt_gives_the_folder() {
        let lines = [entry("a", "/w/one", 1, "x"), entry("a", "/w/two", 2, "y")];
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(read_history(&lines)[0].cwd, "/w/two");
    }

    #[test]
    fn a_long_first_prompt_is_cut() {
        let line = entry("a", "/w/api", 10, &"é".repeat(1000));
        let sessions = read_history(&[&line]);
        assert_eq!(sessions[0].first_prompt.chars().count(), PROMPT_CHARS);
    }

    #[test]
    fn a_missing_history_is_empty() {
        let env = Env::new();
        let mut reader = env.reader(false);
        assert!(reader.load().is_empty());
        assert_eq!(reader.generation(), 0);
    }

    #[test]
    fn load_reads_again_only_when_the_file_changes() {
        let env = Env::new();
        let mut reader = env.reader(false);
        assert!(reader.load().is_empty());
        env.write(&[entry("a", "/w/api", 10, "hi")]);
        assert_eq!(ids(reader.load()), ["a"]);
        env.write(&[
            entry("a", "/w/api", 10, "hi"),
            entry("b", "/w/api", 20, "hi"),
        ]);
        assert_eq!(ids(reader.load()), ["b", "a"]);
    }

    #[test]
    fn sessions_are_what_the_last_load_returned() {
        let env = Env::new();
        let mut reader = env.reader(false);
        env.write(&[entry("a", "/w/api", 10, "hi")]);
        reader.load();
        // Read on without looking at the file again
        env.write(&[]);
        assert_eq!(ids(reader.sessions()), ["a"]);
    }

    #[test]
    fn history_line_without_newline_waits() {
        let env = Env::new();
        env.write(&[entry("a", "/w/api", 10, "hi")]);
        env.append(r#"{"sessionId": "b", "project": "/w/api", "timestamp": 20000"#);
        let mut reader = env.reader(false);
        assert_eq!(ids(reader.load()), ["a"]);
        // The rest of the line arrives
        env.append(", \"display\": \"late\"}\n");
        let sessions = reader.load();
        assert_eq!(ids(sessions), ["b", "a"]);
        assert_eq!(sessions[0].first_prompt, "late");
    }

    #[test]
    fn history_splits_on_newline_only() {
        // Written raw, as Claude Code writes it: JSON leaves U+2028 unescaped
        let env = Env::new();
        env.write(&[format!(
            r#"{{"sessionId":"a","project":"/w/api","timestamp":1,"display":"one{}two"}}"#,
            '\u{2028}'
        )]);
        let mut reader = env.reader(false);
        let sessions = reader.load();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].first_prompt, "one\u{2028}two");
    }

    #[test]
    fn history_replaced_by_a_shorter_file_is_read_again() {
        let env = Env::new();
        env.write(&[
            entry("a", "/w/api", 10, "hi"),
            entry("b", "/w/api", 20, "hi"),
        ]);
        let mut reader = env.reader(false);
        assert_eq!(reader.load().len(), 2);
        env.write(&[entry("c", "/w/api", 30, "hi")]);
        assert_eq!(ids(reader.load()), ["c"]);
    }

    #[test]
    fn history_rewritten_in_place_is_read_again() {
        let env = Env::new();
        env.write(&[entry("a", "/w/api", 10, "hi")]);
        let mut reader = env.reader(false);
        assert_eq!(ids(reader.load()), ["a"]);
        // Same file, longer, but what was read is gone
        let mut f = OpenOptions::new().write(true).open(env.history()).unwrap();
        let text = format!(
            "{}\n{}\n",
            entry("b", "/w/api", 20, "hi"),
            entry("c", "/w/api", 30, "hi")
        );
        f.write_all(text.as_bytes()).unwrap();
        assert_eq!(ids(reader.load()), ["c", "b"]);
    }

    #[test]
    fn history_is_read_on_from_where_the_last_process_stopped() {
        let env = Env::new();
        env.write(&[entry("a", "/w/api", 10, "first")]);
        assert_eq!(ids(env.reader(true).load()), ["a"]);
        // The reading on disk says "kept"; only a new process that starts from it shows that
        // the first line was not parsed again
        let saved = fs::read_to_string(env.state().join(STORED_HISTORY)).unwrap();
        assert!(saved.contains("\"first\""));
        fs::write(
            env.state().join(STORED_HISTORY),
            saved.replace("\"first\"", "\"kept\""),
        )
        .unwrap();
        env.append(&format!("{}\n", entry("b", "/w/web", 20, "hi")));
        env.append(r#"{"sessionId": "c", "proj"#);
        let mut reader = env.reader(false);
        let got: Vec<(&str, &str)> = reader
            .load()
            .iter()
            .map(|s| (s.id.as_str(), s.first_prompt.as_str()))
            .collect();
        // The line still being written waits
        assert_eq!(got, [("b", "hi"), ("a", "kept")]);
    }

    #[test]
    fn a_stored_reading_of_another_file_is_not_continued() {
        let env = Env::new();
        env.write(&[entry("a", "/w/api", 10, "first")]);
        env.reader(true).load();
        let saved = fs::read_to_string(env.state().join(STORED_HISTORY)).unwrap();
        fs::write(
            env.state().join(STORED_HISTORY),
            saved.replace("\"first\"", "\"kept\""),
        )
        .unwrap();
        // Replaced by a new file, as an editor saving it would
        let other = env.config().join("other");
        fs::write(&other, format!("{}\n", entry("a", "/w/api", 10, "first"))).unwrap();
        fs::rename(&other, env.history()).unwrap();
        assert_eq!(env.reader(false).load()[0].first_prompt, "first");
    }

    #[test]
    fn an_unreadable_stored_reading_is_started_over() {
        let env = Env::new();
        env.write(&[entry("a", "/w/api", 10, "hi")]);
        fs::create_dir_all(env.state()).unwrap();
        fs::write(env.state().join(STORED_HISTORY), "{\"offset\": 3").unwrap();
        assert_eq!(ids(env.reader(true).load()), ["a"]);
    }

    #[test]
    fn history_is_kept_on_disk_only_when_asked() {
        let env = Env::new();
        env.write(&[entry("a", "/w/api", 10, "hi")]);
        env.reader(false).load();
        assert!(!env.state().join(STORED_HISTORY).exists());
        env.reader(true).load();
        assert!(env.state().join(STORED_HISTORY).exists());
    }

    #[test]
    fn generation_changes_only_when_the_sessions_do() {
        let env = Env::new();
        env.write(&[entry("a", "/w/api", 10, "hi")]);
        let mut reader = env.reader(false);
        reader.load();
        let first = reader.generation();
        assert!(first > 0);
        reader.load();
        assert_eq!(reader.generation(), first);
        // The file grows, but with a line that is not an entry
        env.append("not json\n");
        reader.load();
        assert_eq!(reader.generation(), first);
        // And with one that is
        env.append(&format!("{}\n", entry("b", "/w/api", 20, "hi")));
        reader.load();
        assert!(reader.generation() > first);
    }

    #[test]
    fn generation_changes_when_the_history_disappears() {
        let env = Env::new();
        env.write(&[entry("a", "/w/api", 10, "hi")]);
        let mut reader = env.reader(false);
        reader.load();
        let before = reader.generation();
        fs::remove_file(env.history()).unwrap();
        assert!(reader.load().is_empty());
        assert!(reader.generation() > before);
    }
}
