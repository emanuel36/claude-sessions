//! Session transcripts: where they live, the title Claude Code gave a session, and whether the
//! last turn was an Esc interrupt or still awaits its first reply.
//!
//! The public functions that need Claude Code's config folder are thin wrappers over `*_in`
//! ones that take it, so unit tests use a temporary directory.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::core::paths;

/// The title is looked for in this many bytes at the end of a transcript.
pub const TITLE_TAIL_BYTES: u64 = 512 * 1024;
/// How a turn ended is looked for in this many bytes at the end of a transcript: enough for
/// the line of a long pasted prompt.
pub const TURN_TAIL_BYTES: u64 = 256 * 1024;
/// What Claude Code writes when the user presses Esc.
pub const INTERRUPT_MARKER: &str = "[Request interrupted by user";
/// Where the titles found in the transcripts are kept between runs, in the state folder.
pub const TITLES_FILE: &str = "titles.json";
/// A title the user set with /rename wins over the one Claude Code made up.
const TITLE_KEYS: [&str; 2] = ["customTitle", "aiTitle"];

/// Returns where Claude Code, with its config folder `config`, keeps a session's transcript:
/// one folder per project path.
pub(crate) fn transcript_path_in(config: &Path, id: &str, cwd: &str) -> PathBuf {
    let folder: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    config
        .join("projects")
        .join(folder)
        .join(format!("{id}.jsonl"))
}

/// Returns the last `n` bytes of a file, or all of it when it is shorter.
///
/// # Errors
///
/// Fails if the file cannot be opened or read.
pub fn tail(path: &Path, n: u64) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let len = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(len.saturating_sub(n)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// How a transcript ends, as far as an Esc interrupt goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    /// The last user entry is the Esc interrupt marker.
    Interrupted,
    /// The last user entry is a prompt with no reply after it: Esc before the first reply
    /// leaves no marker, and a turn still waiting for its first reply looks the same.
    AwaitsReply,
    /// A reply, a tool result, or nothing readable.
    Other,
}

/// Tells how the transcript ends; only its last [`TURN_TAIL_BYTES`] are read.
///
/// An assistant entry after the last user entry means the session went on.
pub fn turn_end(path: &Path) -> TurnEnd {
    let Ok(chunk) = tail(path, TURN_TAIL_BYTES) else {
        return TurnEnd::Other;
    };
    for line in chunk.split(|&b| b == b'\n').rev() {
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("user") if is_interrupt(entry.get("message")) => return TurnEnd::Interrupted,
            Some("user")
                if entry.get("isMeta").and_then(Value::as_bool) != Some(true)
                    && is_prompt(entry.get("message")) =>
            {
                return TurnEnd::AwaitsReply
            }
            Some("user" | "assistant") => return TurnEnd::Other,
            _ => {}
        }
    }
    TurnEnd::Other
}

/// What Claude Code writes in place of a prompt for a local command and its output.
const LOCAL_COMMAND_TAGS: [&str; 3] = ["<command-", "<local-command-", "<bash-"];

/// Tells whether a user message is something the user typed: text, not a tool result, a local
/// command nor the interrupt marker.
fn is_prompt(message: Option<&Value>) -> bool {
    match message.and_then(|m| m.get("content")) {
        Some(Value::String(text)) => {
            !text.trim().is_empty()
                && !text.starts_with(INTERRUPT_MARKER)
                && !LOCAL_COMMAND_TAGS.iter().any(|tag| text.starts_with(tag))
        }
        Some(Value::Array(blocks)) => {
            !blocks.is_empty()
                && !is_interrupt(message)
                && blocks
                    .iter()
                    .all(|block| block.get("type").and_then(Value::as_str) != Some("tool_result"))
        }
        _ => false,
    }
}

/// Tells whether a user message is the marker itself, not a tool result that quotes it.
fn is_interrupt(message: Option<&Value>) -> bool {
    match message.and_then(|m| m.get("content")) {
        Some(Value::String(text)) => text.starts_with(INTERRUPT_MARKER),
        Some(Value::Array(blocks)) => blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("text")
                && block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.starts_with(INTERRUPT_MARKER))
        }),
        _ => false,
    }
}

/// Returns the latest title in a transcript's tail, preferring one the user set.
///
/// The last occurrence of each key that holds a valid, non-empty JSON string counts; the tail
/// may start or end in the middle of a line, so those cut ones are skipped.
pub fn find_title(chunk: &[u8]) -> String {
    for key in TITLE_KEYS {
        if let Some(title) = last_string(chunk, key) {
            if !title.is_empty() {
                return title;
            }
        }
    }
    String::new()
}

/// The last string value of `key` in a chunk of JSON lines that parses.
///
/// Searched from the end: a long transcript holds the key many times, and only the last counts.
fn last_string(chunk: &[u8], key: &str) -> Option<String> {
    let marker = format!("\"{key}\":\"");
    let marker = marker.as_bytes();
    let mut end = chunk.len();
    while let Some(at) = rfind(&chunk[..end], marker) {
        end = at;
        let start = at + marker.len();
        let Some(len) = string_rest(&chunk[start..]) else {
            continue;
        };
        // The text between the quotes, with its own quotes put back, is a JSON string
        let literal = &chunk[start - 1..start + len + 1];
        if let Ok(title) = serde_json::from_slice::<String>(literal) {
            return Some(title);
        }
    }
    None
}

/// The length of the rest of a JSON string after its opening quote, up to its closing one.
///
/// `None` when the chunk ends first, or a backslash is followed by a line break.
fn string_rest(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'"' => return Some(i),
            b'\\' => match bytes.get(i + 1) {
                Some(&next) if next != b'\n' => i += 2,
                _ => return None,
            },
            _ => i += 1,
        }
    }
    None
}

/// Returns where the last occurrence of `needle` starts in `haystack`; an empty needle is
/// found at the end, as with [`str::rfind`].
///
/// Scans back a block of starts at a time for the needle's second and second-to-last bytes, a
/// test the compiler vectorises, and compares whole windows only in a block that has that pair.
fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    const BLOCK: usize = 32;
    let n = needle.len();
    if n == 0 {
        return Some(haystack.len());
    }
    if n < 3 || haystack.len() < n {
        return haystack.windows(n).rposition(|window| window == needle);
    }
    // Not the ends: the keys searched for start and end with `"`, which JSON is full of
    let (second, penultimate) = (needle[1], needle[n - 2]);
    // One past the last place a needle can start
    let mut end = haystack.len() - n + 1;
    while end > 0 {
        let start = end.saturating_sub(BLOCK);
        let seconds = &haystack[start + 1..end + 1];
        let penultimates = &haystack[start + n - 2..end + n - 2];
        let pair = seconds
            .iter()
            .zip(penultimates)
            .fold(false, |seen, (&a, &b)| {
                seen | ((a == second) & (b == penultimate))
            });
        if pair {
            if let Some(at) = (start..end)
                .rev()
                .find(|&at| &haystack[at..at + n] == needle)
            {
                return Some(at);
            }
        }
        end = start;
    }
    None
}

/// Returns a session's title from its transcript, empty when it has none or cannot be read.
pub fn title_of(path: &Path) -> String {
    tail(path, TITLE_TAIL_BYTES)
        .map(|chunk| find_title(&chunk))
        .unwrap_or_default()
}

/// Tells whether a title was written to a transcript after its first `*seen` bytes, and moves
/// `*seen` to the end of the file; only the bytes added since are read.
///
/// A transcript that shrank, or cannot be read, counts as untitled.
pub fn titled_since(path: &Path, seen: &mut u64) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(len) = file.seek(SeekFrom::End(0)) else {
        return false;
    };
    let from = if len < *seen { len } else { *seen };
    *seen = len;
    let mut added = Vec::new();
    let read = file
        .seek(SeekFrom::Start(from))
        .and_then(|_| file.read_to_end(&mut added));
    read.is_ok()
        && TITLE_KEYS
            .iter()
            .any(|key| contains(&added, key.as_bytes()))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    rfind(haystack, needle).is_some()
}

/// Session titles, read from the transcripts again only when one changes.
///
/// With `stored`, they are also kept on disk, so a new process reads only the transcripts that
/// changed since the last one.
#[derive(Debug)]
pub struct Titles {
    stored: bool,
    state: Option<PathBuf>,
    /// Each session's transcript mtime in nanoseconds (`None`: no transcript) and its title.
    seen: Option<HashMap<String, (Option<i64>, String)>>,
    /// The sessions asked for since the last save: the only ones it keeps.
    used: HashSet<String>,
    changed: bool,
}

impl Titles {
    /// Starts with no titles read; `stored` keeps them in the state folder between runs.
    pub fn new(stored: bool) -> Self {
        Self {
            stored,
            state: None,
            seen: None,
            used: HashSet::new(),
            changed: false,
        }
    }

    /// Like [`Titles::new`], keeping the titles in `state` instead of the state folder.
    #[cfg(test)]
    pub(crate) fn in_dir(stored: bool, state: PathBuf) -> Self {
        Self {
            state: Some(state),
            ..Self::new(stored)
        }
    }

    fn state_dir(&self) -> PathBuf {
        self.state.clone().unwrap_or_else(paths::state_dir)
    }

    /// The titles kept on disk, or none when not stored or when the file does not parse.
    fn load(&self) -> HashMap<String, (Option<i64>, String)> {
        if !self.stored {
            return HashMap::new();
        }
        let saved = fs::read(self.state_dir().join(TITLES_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HashMap<String, Value>>(&bytes).ok());
        let Some(saved) = saved else {
            return HashMap::new();
        };
        saved
            .into_iter()
            .filter_map(|(id, value)| {
                let [mtime, title] = value.as_array()?.as_slice() else {
                    return None;
                };
                let mtime = match mtime {
                    Value::Null => None,
                    other => Some(other.as_i64()?),
                };
                Some((id, (mtime, title.as_str()?.to_owned())))
            })
            .collect()
    }

    /// Returns a session's title from its transcript, empty if it has none yet.
    ///
    /// The transcript is read only when its modification time differs from the last reading.
    pub fn get(&mut self, id: &str, path: &Path) -> String {
        if self.seen.is_none() {
            self.seen = Some(self.load());
        }
        if self.stored {
            self.used.insert(id.to_owned());
        }
        let mtime = fs::metadata(path).ok().map(|m| {
            m.mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(m.mtime_nsec())
        });
        let seen = self.seen.get_or_insert_with(HashMap::new);
        if let Some((cached, title)) = seen.get(id) {
            if *cached == mtime {
                return title.clone();
            }
        }
        let title = if mtime.is_some() {
            title_of(path)
        } else {
            String::new()
        };
        seen.insert(id.to_owned(), (mtime, title.clone()));
        self.changed = true;
        title
    }

    /// Writes the titles asked for since the last save to disk, if any changed; never fails.
    pub fn save(&mut self) {
        if !(self.stored && self.changed) {
            return;
        }
        let kept: HashMap<&String, (&Option<i64>, &String)> = self
            .seen
            .iter()
            .flatten()
            .filter(|(id, _)| self.used.contains(*id))
            .map(|(id, (mtime, title))| (id, (mtime, title)))
            .collect();
        let dir = self.state_dir();
        let written = serde_json::to_vec(&kept)
            .map_err(io::Error::other)
            .and_then(|json| {
                fs::create_dir_all(&dir)?;
                paths::write_atomic(&dir.join(TITLES_FILE), &json, None)
            });
        if written.is_ok() {
            self.used.clear();
            self.changed = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    use serde_json::json;
    use tempfile::TempDir;

    use crate::claude::transcript::{
        find_title, rfind, tail, title_of, titled_since, transcript_path_in, turn_end, Titles,
        TurnEnd, TITLES_FILE,
    };

    fn set_mtime(path: &Path, secs: u64) {
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    fn write_transcript(path: &Path, entries: &[serde_json::Value]) {
        let text: String = entries.iter().map(|e| format!("{e}\n")).collect();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn transcript_path_follows_claude_code_folder_names() {
        let path = transcript_path_in(Path::new("/c"), "abc", "/home/me/projects/app");
        assert_eq!(
            path,
            Path::new("/c/projects/-home-me-projects-app/abc.jsonl")
        );
    }

    #[test]
    fn transcript_path_replaces_every_non_alphanumeric_char() {
        let path = transcript_path_in(Path::new("/c"), "x", "/a b/c.d_e/ç");
        assert_eq!(path, Path::new("/c/projects/-a-b-c-d-e--/x.jsonl"));
    }

    /// Where the last `needle` starts in `haystack`, comparing every window.
    fn naive_rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() {
            return Some(haystack.len());
        }
        haystack
            .windows(needle.len())
            .rposition(|window| window == needle)
    }

    #[test]
    fn rfind_finds_an_empty_needle_at_the_end() {
        assert_eq!(rfind(b"", b""), Some(0));
        assert_eq!(rfind(b"abc", b""), Some(3));
    }

    proptest::proptest! {
        #[test]
        fn rfind_finds_the_last_occurrence(
            haystack in "[ab\"]{0,80}",
            needle in "[ab\"]{1,6}",
        ) {
            let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
            proptest::prop_assert_eq!(rfind(haystack, needle), naive_rfind(haystack, needle));
        }

        #[test]
        fn rfind_finds_long_needles(
            haystack in "[ab\"]{0,200}",
            needle in "[ab\"]{30,40}|a{10,40}|\"[ab]{8,38}\"",
        ) {
            let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
            proptest::prop_assert_eq!(rfind(haystack, needle), naive_rfind(haystack, needle));
        }

        #[test]
        fn rfind_finds_a_needle_taken_from_the_haystack(
            haystack in "[ab\"]{1,200}",
            from in 0usize..200,
            len in 1usize..60,
        ) {
            let haystack = haystack.as_bytes();
            let from = from % haystack.len();
            let needle = &haystack[from..(from + len).min(haystack.len())];
            let found = rfind(haystack, needle);
            proptest::prop_assert!(found >= Some(from));
            proptest::prop_assert_eq!(found, naive_rfind(haystack, needle));
        }
    }

    #[test]
    fn a_title_the_user_set_wins() {
        let tail = b"{\"type\":\"ai-title\",\"aiTitle\":\"Old\"}\n\
            {\"type\":\"ai-title\",\"aiTitle\":\"Fix \\\"x\\\"\"}\n{\"type\":\"user\"}\n";
        assert_eq!(find_title(tail), "Fix \"x\"");
        let mut with_custom = tail.to_vec();
        with_custom.extend_from_slice(b"{\"type\":\"custom-title\",\"customTitle\":\"Mine\"}");
        assert_eq!(find_title(&with_custom), "Mine");
        assert_eq!(find_title(b"{\"aiTitle\":\"cut"), "");
    }

    #[test]
    fn the_last_title_that_parses_wins() {
        let tail = b"{\"aiTitle\":\"Good\"}\n{\"aiTitle\":\"bad \\q\"}\n{\"customTitle\":\"\"}\n";
        assert_eq!(find_title(tail), "Good");
    }

    #[test]
    fn title_with_escaped_quotes_and_unicode() {
        let tail = br#"{"aiTitle":"say \"h\u00e9llo\" \u2603 caf\u00e9 \ud83d\ude00 end\\"}"#;
        assert_eq!(
            find_title(tail),
            "say \"héllo\" \u{2603} café \u{1F600} end\\"
        );
        assert_eq!(find_title("{\"aiTitle\":\"ção ✓\"}".as_bytes()), "ção ✓");
    }

    #[test]
    fn a_title_cut_by_the_tail_does_not_run_into_the_next_line() {
        // The unterminated string would reach the quote on the next line; the newline in it
        // is not valid JSON, so it is not a title
        let tail = b"{\"aiTitle\":\"cut\n{\"x\":\"y\"}\n";
        assert_eq!(find_title(tail), "");
    }

    #[test]
    fn tail_reads_the_last_bytes_of_a_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("f");
        fs::write(&path, b"0123456789").unwrap();
        assert_eq!(tail(&path, 4).unwrap(), b"6789");
        assert_eq!(tail(&path, 100).unwrap(), b"0123456789");
        assert!(tail(&tmp.path().join("missing"), 4).is_err());
    }

    #[test]
    fn title_of_reads_the_tail_and_gives_empty_for_a_missing_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.jsonl");
        fs::write(&path, b"{\"aiTitle\":\"Hello\"}\n").unwrap();
        assert_eq!(title_of(&path), "Hello");
        assert_eq!(title_of(&tmp.path().join("missing")), "");
    }

    #[test]
    fn the_marker_inside_a_tool_result_is_not_an_interrupt() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.jsonl");
        let result =
            json!({"type": "tool_result", "content": "grep: [Request interrupted by user]"});
        write_transcript(
            &path,
            &[json!({"type": "user", "message": {"content": [result]}})],
        );
        assert!(turn_end(&path) != TurnEnd::Interrupted);
    }

    #[test]
    fn an_interrupt_in_a_text_block_counts() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.jsonl");
        let block = json!({"type": "text", "text": "[Request interrupted by user for tool use]"});
        write_transcript(
            &path,
            &[json!({"type": "user", "message": {"content": [block]}})],
        );
        assert!(turn_end(&path) == TurnEnd::Interrupted);
    }

    #[test]
    fn an_interrupt_in_string_content_counts() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_transcript(
            &path,
            &[json!({"type": "user", "message": {"content": "[Request interrupted by user]"}})],
        );
        assert!(turn_end(&path) == TurnEnd::Interrupted);
    }

    #[test]
    fn an_assistant_entry_after_the_marker_means_not_interrupted() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_transcript(
            &path,
            &[
                json!({"type": "user", "message": {"content": "[Request interrupted by user]"}}),
                json!({"type": "assistant", "message": {"content": "ok"}}),
            ],
        );
        assert!(turn_end(&path) != TurnEnd::Interrupted);
    }

    #[test]
    fn a_prompt_with_no_reply_awaits_one() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.jsonl");
        let prompt = json!({"type": "user", "message": {"content": "do it"}});
        let blocks =
            json!({"type": "user", "message": {"content": [{"type": "text", "text": "x"}]}});
        let reply = json!({"type": "assistant", "message": {"content": "ok"}});
        let other = json!({"type": "api-request"});
        write_transcript(&path, &[reply.clone(), prompt.clone(), other]);
        assert!(turn_end(&path) == TurnEnd::AwaitsReply);
        write_transcript(&path, &[blocks]);
        assert!(turn_end(&path) == TurnEnd::AwaitsReply);
        write_transcript(&path, &[prompt, reply]);
        assert!(turn_end(&path) != TurnEnd::AwaitsReply);
    }

    #[test]
    fn a_long_pasted_prompt_still_awaits_a_reply() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.jsonl");
        let prompt = json!({"type": "user", "message": {"content": "x".repeat(100_000)}});
        write_transcript(&path, &[json!({"type": "assistant"}), prompt]);
        assert!(turn_end(&path) == TurnEnd::AwaitsReply);
    }

    #[test]
    fn a_tool_result_a_marker_or_a_meta_entry_awaits_no_reply() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.jsonl");
        let result = json!({"type": "tool_result", "content": "done"});
        let ends = [
            json!({"type": "user", "message": {"content": [result]}}),
            json!({"type": "user", "message": {"content": "[Request interrupted by user]"}}),
            json!({"type": "user", "isMeta": true, "message": {"content": "caveat"}}),
            json!({"type": "user", "message": {"content": "<local-command-stdout>ok"}}),
            json!({"type": "user", "message": {"content": " "}}),
        ];
        for end in ends {
            write_transcript(&path, std::slice::from_ref(&end));
            assert!(turn_end(&path) != TurnEnd::AwaitsReply, "{end}");
        }
        assert!(turn_end(&tmp.path().join("missing")) != TurnEnd::AwaitsReply);
    }

    #[test]
    fn a_missing_transcript_was_not_interrupted() {
        let tmp = TempDir::new().unwrap();
        assert!(turn_end(&tmp.path().join("missing")) != TurnEnd::Interrupted);
    }

    #[test]
    fn titles_read_again_when_the_transcript_changes() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.jsonl");
        let mut titles = Titles::in_dir(false, tmp.path().join("state"));
        assert_eq!(titles.get("a", &path), "");
        fs::write(&path, b"{\"aiTitle\":\"First\"}\n").unwrap();
        assert_eq!(titles.get("a", &path), "First");
        fs::write(
            &path,
            b"{\"aiTitle\":\"First\"}\n{\"customTitle\":\"Mine\"}\n",
        )
        .unwrap();
        set_mtime(&path, 2);
        assert_eq!(titles.get("a", &path), "Mine");
        assert_eq!(titles.get("b", &tmp.path().join("missing")), "");
    }

    #[test]
    fn titles_are_not_read_while_the_transcript_is_untouched() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.jsonl");
        fs::write(&path, b"{\"aiTitle\":\"First\"}\n").unwrap();
        set_mtime(&path, 5);
        let mut titles = Titles::in_dir(false, tmp.path().join("state"));
        assert_eq!(titles.get("a", &path), "First");
        // Different content, same mtime: the cache answers
        fs::write(&path, b"{\"aiTitle\":\"Other\"}\n").unwrap();
        set_mtime(&path, 5);
        assert_eq!(titles.get("a", &path), "First");
    }

    #[test]
    fn stored_titles_spare_a_new_process_the_transcripts() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path().join("state");
        let path = tmp.path().join("t.jsonl");
        fs::write(&path, b"{\"aiTitle\":\"First\"}\n").unwrap();
        set_mtime(&path, 5);
        let mut titles = Titles::in_dir(true, state.clone());
        assert_eq!(titles.get("a", &path), "First");
        titles.save();
        // The transcript changes behind the same mtime: only a stored title still says First
        fs::write(&path, b"{\"aiTitle\":\"Other\"}\n").unwrap();
        set_mtime(&path, 5);
        let mut again = Titles::in_dir(true, state);
        assert_eq!(again.get("a", &path), "First");
        // A transcript that changed is read again
        set_mtime(&path, 6);
        assert_eq!(again.get("a", &path), "Other");
    }

    #[test]
    fn titles_that_are_not_stored_stay_off_the_disk() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path().join("state");
        let mut titles = Titles::in_dir(false, state.clone());
        titles.get("a", &tmp.path().join("missing"));
        titles.save();
        assert!(!state.join(TITLES_FILE).exists());
    }

    #[test]
    fn stored_titles_keep_only_the_sessions_asked_for() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path().join("state");
        let missing = tmp.path().join("missing");
        let mut titles = Titles::in_dir(true, state.clone());
        titles.get("a", &missing);
        titles.save();
        titles.get("b", &missing);
        titles.save();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(state.join(TITLES_FILE)).unwrap()).unwrap();
        assert_eq!(saved, json!({"b": [null, ""]}));
    }

    #[test]
    fn an_unreadable_titles_file_is_started_over() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::write(state.join(TITLES_FILE), b"[1, 2").unwrap();
        let path = tmp.path().join("t.jsonl");
        fs::write(&path, b"{\"aiTitle\":\"First\"}\n").unwrap();
        let mut titles = Titles::in_dir(true, state);
        assert_eq!(titles.get("a", &path), "First");
    }

    #[test]
    fn titled_since_sees_only_what_was_appended() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.jsonl");
        fs::write(&path, b"{\"aiTitle\":\"First\"}\n").unwrap();
        let mut seen = fs::metadata(&path).unwrap().len();
        // Nothing new, and a title already seen, do not count
        assert!(!titled_since(&path, &mut seen));
        let mut text = fs::read(&path).unwrap();
        text.extend_from_slice(b"{\"type\":\"user\"}\n");
        fs::write(&path, &text).unwrap();
        assert!(!titled_since(&path, &mut seen));
        text.extend_from_slice(b"{\"type\":\"custom-title\",\"customTitle\":\"Mine\"}\n");
        fs::write(&path, &text).unwrap();
        assert!(titled_since(&path, &mut seen));
        assert_eq!(seen, text.len() as u64);
        assert!(!titled_since(&path, &mut seen));
        // A missing or shrunken transcript counts as untitled
        assert!(!titled_since(&tmp.path().join("missing"), &mut seen));
        fs::write(&path, b"{}\n").unwrap();
        assert!(!titled_since(&path, &mut seen));
        assert_eq!(seen, 3);
    }
}
