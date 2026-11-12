//! What each model cost in a session, estimated from its transcripts.
//!
//! Claude Code reports one running cost per session, under the model of its main conversation,
//! though subagents may run on other models. The transcripts (the session's own and its
//! subagents') record each reply's model and tokens; priced with [`price`], they give the share
//! of each model, which splits the reported cost. The prices only weigh the models against each
//! other, so the reported total never changes.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, Metadata};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::claude::transcript::transcript_path_in;
use crate::core::growing::{FileId, Position};
use crate::core::paths;

/// The file in the state folder that keeps the transcripts' readings between runs.
pub const STORED_SPEND: &str = "spend.json";

/// A model's dollars per million tokens: input, output, cache write and cache read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

const fn price_of(input: f64, output: f64, cache_read: f64) -> Price {
    // A cache write costs a quarter more than plain input
    Price {
        input,
        output,
        cache_write: input * 1.25,
        cache_read,
    }
}

/// Known models, by id without the `claude-` prefix; the first of each family is its newest,
/// which prices a model of that family the table does not know yet.
const PRICES: [(&str, Price); 13] = [
    ("fable-5-1", price_of(10.0, 50.0, 0.25)),
    ("fable-5", price_of(10.0, 50.0, 1.0)),
    ("mythos-5-1", price_of(10.0, 50.0, 0.25)),
    ("opus-5-5", price_of(4.0, 20.0, 0.2)),
    ("opus-5", price_of(5.0, 25.0, 0.5)),
    ("opus-4-8", price_of(5.0, 25.0, 0.5)),
    ("opus-4-7", price_of(5.0, 25.0, 0.5)),
    ("opus-4-6", price_of(5.0, 25.0, 0.5)),
    ("sonnet-5-5", price_of(2.0, 10.0, 0.2)),
    ("sonnet-5", price_of(2.0, 10.0, 0.2)),
    ("sonnet-4-6", price_of(3.0, 15.0, 0.3)),
    ("haiku-5-5", price_of(0.1, 0.5, 0.01)),
    ("haiku-4-5", price_of(1.0, 5.0, 0.1)),
];

/// Returns a model id without the `claude-` prefix, a `[1m]`-style tag or a date suffix.
fn base_id(model: &str) -> &str {
    let model = model.split('[').next().unwrap_or(model);
    let model = model.strip_prefix("claude-").unwrap_or(model);
    match model.rsplit_once('-') {
        Some((rest, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => rest,
        _ => model,
    }
}

/// Returns the price of a model id: its own, else the newest of its family, else none.
pub fn price(model: &str) -> Option<Price> {
    let id = base_id(model);
    if let Some((_, p)) = PRICES.iter().find(|(known, _)| *known == id) {
        return Some(*p);
    }
    let family = id.split('-').next()?;
    PRICES
        .iter()
        .find(|(known, _)| known.split('-').next() == Some(family))
        .map(|(_, p)| *p)
}

/// Returns the name Claude Code shows for a model id: `claude-opus-5-5` is `Opus 5.5`.
pub fn display_name(model: &str) -> String {
    let id = base_id(model);
    let mut parts = id.split('-');
    let Some(family) = parts.next().filter(|f| !f.is_empty()) else {
        return model.to_owned();
    };
    let version: Vec<&str> = parts.collect();
    if version.is_empty()
        || !version
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
    {
        return model.to_owned();
    }
    let mut chars = family.chars();
    let first = chars.next().map(|c| c.to_ascii_uppercase());
    let family: String = first.into_iter().chain(chars).collect();
    format!("{family} {}", version.join("."))
}

/// Tokens of one model, summed over a transcript's replies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Tokens {
    input: u64,
    output: u64,
    cache_write: u64,
    cache_read: u64,
}

impl Tokens {
    fn add(&mut self, other: Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
    }

    fn dollars(&self, p: Price) -> f64 {
        let million = 1_000_000.0;
        (self.input as f64 * p.input
            + self.output as f64 * p.output
            + self.cache_write as f64 * p.cache_write
            + self.cache_read as f64 * p.cache_read)
            / million
    }
}

#[derive(Deserialize)]
struct Line {
    message: Option<Message>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
#[serde(default)]
#[derive(Default)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
}

/// One reply of a transcript line: a key that repeats when Claude Code writes one reply over
/// several lines, its model and its tokens.
fn reply_of(line: &[u8]) -> Option<(Option<String>, String, Tokens)> {
    // Most lines carry no usage: skip them before parsing
    if !line.windows(7).any(|w| w == b"\"usage\"") {
        return None;
    }
    let parsed: Line = serde_json::from_slice(line).ok()?;
    let message = parsed.message?;
    let model = message.model?;
    let usage = message.usage?;
    let tokens = Tokens {
        input: usage.input_tokens,
        output: usage.output_tokens,
        cache_write: usage.cache_creation_input_tokens,
        cache_read: usage.cache_read_input_tokens,
    };
    let key = message
        .id
        .map(|id| format!("{id}:{}", parsed.request_id.unwrap_or_default()));
    Some((key, model, tokens))
}

/// How far a transcript was read, and what its replies added up to. This is what is kept on
/// disk between runs.
#[derive(Debug, Serialize, Deserialize)]
struct Reading {
    #[serde(flatten)]
    at: Position,
    /// The file's modification time in nanoseconds when it was last read.
    mtime: i64,
    seen: HashSet<String>,
    tokens: HashMap<String, Tokens>,
}

impl Reading {
    fn start(file: FileId) -> Self {
        Reading {
            at: Position::start(file),
            mtime: 0,
            seen: HashSet::new(),
            tokens: HashMap::new(),
        }
    }

    /// Reads the lines added to an open transcript since the last time; one replaced, shorter
    /// than that or no longer ending the reading with the same bytes is read again.
    ///
    /// Returns whether what the reading keeps changed: a touch or a line still being written
    /// changes nothing.
    fn catch_up(&mut self, file: &mut File, meta: &Metadata, id: FileId) -> bool {
        let mtime = meta
            .mtime()
            .saturating_mul(1_000_000_000)
            .saturating_add(meta.mtime_nsec());
        if self.at.file == id && meta.len() == self.at.offset && mtime == self.mtime {
            return false;
        }
        let mut changed = false;
        if !self.at.continues(file, meta, &id) {
            changed = self.at.offset != 0;
            *self = Reading::start(id);
        }
        self.mtime = mtime;
        if file.seek(SeekFrom::Start(self.at.offset)).is_err() {
            return changed;
        }
        // Line by line, so a long transcript is never held whole
        let mut lines = BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            // A line still being written waits for the next reading
            match lines.read_until(b'\n', &mut line) {
                Ok(_) if line.last() == Some(&b'\n') => {}
                _ => break,
            }
            self.add(&line[..line.len() - 1]);
            self.at.advance(&line);
            changed = true;
        }
        changed
    }

    /// Counts a line's reply, once per key.
    fn add(&mut self, line: &[u8]) {
        let Some((key, model, tokens)) = reply_of(line) else {
            return;
        };
        if let Some(key) = key {
            if !self.seen.insert(key) {
                return;
            }
        }
        self.tokens.entry(model).or_default().add(tokens);
    }
}

/// Estimates what each model cost in each session, reading the transcripts incrementally.
///
/// With `stored`, the readings are also kept on disk, so a new process reads only what the
/// transcripts added since the last one.
#[derive(Debug)]
pub struct ModelSpend {
    stored: bool,
    config: Option<PathBuf>,
    state: Option<PathBuf>,
    /// The readings by transcript; `None` until the first estimate loads the stored ones.
    readings: Option<HashMap<PathBuf, Reading>>,
    /// The transcripts read since the last save: the only ones it keeps.
    used: HashSet<PathBuf>,
    changed: bool,
}

impl Default for ModelSpend {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelSpend {
    /// Returns an estimator over Claude Code's own config folder, its readings kept in the
    /// state folder between runs.
    pub fn new() -> Self {
        ModelSpend {
            stored: true,
            config: None,
            state: None,
            readings: None,
            used: HashSet::new(),
            changed: false,
        }
    }

    /// Returns an estimator over the config folder `config` that keeps nothing on disk.
    #[doc(hidden)]
    pub fn in_dir(config: PathBuf) -> Self {
        ModelSpend {
            stored: false,
            config: Some(config),
            ..Self::new()
        }
    }

    /// Like [`ModelSpend::new`], over the config folder `config` and the state folder `state`.
    #[cfg(test)]
    pub(crate) fn in_dirs(config: PathBuf, state: PathBuf) -> Self {
        ModelSpend {
            config: Some(config),
            state: Some(state),
            ..Self::new()
        }
    }

    fn stored_path(&self) -> PathBuf {
        let state = self.state.clone().unwrap_or_else(paths::state_dir);
        state.join(STORED_SPEND)
    }

    /// The readings kept at `path`, or none when not `stored` or when the file does not parse.
    fn load(stored: bool, path: &Path) -> HashMap<PathBuf, Reading> {
        if !stored {
            return HashMap::new();
        }
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes the readings of the transcripts read since the last save to disk, if any
    /// changed; never fails.
    pub fn save(&mut self) {
        if !(self.stored && self.changed) {
            return;
        }
        let kept: HashMap<&PathBuf, &Reading> = self
            .readings
            .iter()
            .flatten()
            // A path that is not UTF-8 makes no JSON key: that transcript is read again instead
            .filter(|(path, _)| self.used.contains(*path) && path.to_str().is_some())
            .collect();
        let path = self.stored_path();
        let written = serde_json::to_vec(&kept)
            .map_err(io::Error::other)
            .and_then(|json| {
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir)?;
                }
                paths::write_atomic(&path, &json, None)
            });
        // The next process reads the transcripts again if this is lost
        if written.is_ok() {
            self.used.clear();
            self.changed = false;
        }
    }

    /// Returns a session's estimated dollars by model name, from its transcript and its
    /// subagents'. Models with no known price are left out; empty without a transcript.
    ///
    /// # Arguments
    /// * `id`: the session's id.
    /// * `cwd`: the folder it ran in, which names its transcripts' folder.
    pub fn estimate(&mut self, id: &str, cwd: &str) -> HashMap<String, f64> {
        let config = self.config.clone().unwrap_or_else(paths::config_dir);
        let (stored, stored_path) = (self.stored, self.stored_path());
        let readings = self
            .readings
            .get_or_insert_with(|| Self::load(stored, &stored_path));
        let mut tokens: HashMap<String, Tokens> = HashMap::new();
        for path in session_files(&config, id, cwd) {
            let opened = File::open(&path).and_then(|f| Ok((f.metadata()?, f)));
            let Ok((meta, mut file)) = opened else {
                // A transcript gone is not kept
                self.changed |= readings.remove(&path).is_some();
                continue;
            };
            let id = FileId::of(&path, &meta);
            let reading = readings
                .entry(path.clone())
                .or_insert_with(|| Reading::start(id.clone()));
            self.changed |= reading.catch_up(&mut file, &meta, id);
            for (model, t) in &reading.tokens {
                tokens.entry(model.clone()).or_default().add(*t);
            }
            if self.stored {
                self.used.insert(path);
            }
        }
        let mut dollars: HashMap<String, f64> = HashMap::new();
        for (model, t) in tokens {
            if let Some(p) = price(&model) {
                *dollars.entry(display_name(&model)).or_default() += t.dollars(p);
            }
        }
        dollars.retain(|_, d| *d > 0.0);
        dollars
    }
}

/// A session's transcript, and its subagents' under `<session id>/subagents/`.
fn session_files(config: &Path, id: &str, cwd: &str) -> Vec<PathBuf> {
    let main = transcript_path_in(config, id, cwd);
    let mut files = vec![main.clone()];
    let subagents = main.with_extension("").join("subagents");
    if let Ok(entries) = fs::read_dir(subagents) {
        let mut found: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .collect();
        found.sort();
        files.extend(found);
    }
    files
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    use serde_json::json;
    use tempfile::TempDir;

    use crate::claude::spend::{display_name, price, ModelSpend, Price, STORED_SPEND};
    use crate::claude::transcript::transcript_path_in;

    fn reply(id: &str, model: &str, input: u64, output: u64) -> String {
        let usage = json!({
            "input_tokens": input,
            "output_tokens": output,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0,
        });
        let message = json!({"id": id, "model": model, "usage": usage});
        json!({"type": "assistant", "requestId": format!("r-{id}"), "message": message}).to_string()
    }

    fn append(path: &Path, lines: &[String]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
    }

    #[test]
    fn names_follow_claude_code() {
        assert_eq!(display_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(display_name("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(display_name("claude-opus-4-6[1m]"), "Opus 4.6");
        assert_eq!(display_name("<synthetic>"), "<synthetic>");
    }

    #[test]
    fn an_unknown_model_takes_its_familys_newest_price() {
        let opus = price("claude-opus-5-5").unwrap();
        assert_eq!(price("claude-opus-9-9"), Some(opus));
        assert_eq!(price("claude-opus-5-5-20260101"), Some(opus));
        assert_eq!(price("gpt-whatever"), None);
        assert_eq!(
            price("claude-sonnet-5-5"),
            Some(Price {
                input: 2.0,
                output: 10.0,
                cache_write: 2.5,
                cache_read: 0.2
            })
        );
    }

    #[test]
    fn a_session_adds_its_subagents_by_model() {
        let tmp = TempDir::new().unwrap();
        let main = transcript_path_in(tmp.path(), "s", "/w/p");
        append(&main, &[reply("m1", "claude-opus-5-5", 1_000_000, 0)]);
        let sub = main.with_extension("").join("subagents/agent-a.jsonl");
        append(&sub, &[reply("m2", "claude-sonnet-5-5", 0, 1_000_000)]);
        let mut spend = ModelSpend::in_dir(tmp.path().to_path_buf());
        let got = spend.estimate("s", "/w/p");
        assert_eq!(got["Opus 5.5"], 4.0);
        assert_eq!(got["Sonnet 5.5"], 10.0);
    }

    #[test]
    fn a_reply_written_over_several_lines_counts_once() {
        let tmp = TempDir::new().unwrap();
        let main = transcript_path_in(tmp.path(), "s", "/w");
        let line = reply("m1", "claude-opus-5-5", 1_000_000, 0);
        append(&main, &[line.clone(), line, r#"{"type":"user"}"#.into()]);
        let mut spend = ModelSpend::in_dir(tmp.path().to_path_buf());
        assert_eq!(spend.estimate("s", "/w")["Opus 5.5"], 4.0);
    }

    #[test]
    fn readings_continue_and_restart_on_a_shorter_file() {
        let tmp = TempDir::new().unwrap();
        let main = transcript_path_in(tmp.path(), "s", "/w");
        append(&main, &[reply("m1", "claude-opus-5-5", 1_000_000, 0)]);
        let mut spend = ModelSpend::in_dir(tmp.path().to_path_buf());
        assert_eq!(spend.estimate("s", "/w")["Opus 5.5"], 4.0);
        // A line not finished yet waits
        let mut f = OpenOptions::new().append(true).open(&main).unwrap();
        write!(f, "{}", reply("m2", "claude-opus-5-5", 1_000_000, 0)).unwrap();
        assert_eq!(spend.estimate("s", "/w")["Opus 5.5"], 4.0);
        writeln!(f).unwrap();
        assert_eq!(spend.estimate("s", "/w")["Opus 5.5"], 8.0);
        // Replaced by a shorter file: read from the start
        fs::write(
            &main,
            format!("{}\n", reply("m3", "claude-haiku-5-5", 0, 0)),
        )
        .unwrap();
        let got = spend.estimate("s", "/w");
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn no_transcript_gives_nothing() {
        let tmp = TempDir::new().unwrap();
        let mut spend = ModelSpend::in_dir(tmp.path().to_path_buf());
        assert!(spend.estimate("s", "/w").is_empty());
    }

    fn set_mtime(path: &Path, secs: u64) {
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn stored_readings_spare_a_new_process_the_transcripts() {
        let tmp = TempDir::new().unwrap();
        let (config, state) = (tmp.path().join("config"), tmp.path().join("state"));
        let main = transcript_path_in(&config, "s", "/w");
        append(&main, &[reply("m1", "claude-opus-5-5", 1_000_000, 0)]);
        set_mtime(&main, 5);
        let mut spend = ModelSpend::in_dirs(config.clone(), state.clone());
        assert_eq!(spend.estimate("s", "/w")["Opus 5.5"], 4.0);
        spend.save();
        // The transcript changes behind the same size and mtime: only a stored reading says Opus
        fs::write(
            &main,
            format!("{}\n", reply("m1", "claude-opus-5-1", 1_000_000, 0)),
        )
        .unwrap();
        set_mtime(&main, 5);
        let mut again = ModelSpend::in_dirs(config.clone(), state.clone());
        assert_eq!(again.estimate("s", "/w")["Opus 5.5"], 4.0);
        // Not stored, the same process reads the transcript
        let mut fresh = ModelSpend::in_dir(config);
        assert_eq!(fresh.estimate("s", "/w")["Opus 5.1"], 4.0);
    }

    #[test]
    fn a_stored_reading_goes_on_from_where_it_stopped() {
        let tmp = TempDir::new().unwrap();
        let (config, state) = (tmp.path().join("config"), tmp.path().join("state"));
        let main = transcript_path_in(&config, "s", "/w");
        let first = reply("m1", "claude-opus-5-5", 1_000_000, 0);
        append(&main, std::slice::from_ref(&first));
        let mut spend = ModelSpend::in_dirs(config.clone(), state.clone());
        assert_eq!(spend.estimate("s", "/w")["Opus 5.5"], 4.0);
        spend.save();
        // The reply's other line counts once, as it would in one reading
        append(
            &main,
            &[first, reply("m2", "claude-opus-5-5", 1_000_000, 0)],
        );
        let mut again = ModelSpend::in_dirs(config.clone(), state.clone());
        assert_eq!(again.estimate("s", "/w")["Opus 5.5"], 8.0);
        again.save();
        // Replaced by another file, as long or longer: read from the start
        let other = reply("m3", "claude-haiku-5-5", 0, 1_000_000_000);
        fs::write(&main, format!("{other}\n{other}\n{other}\n")).unwrap();
        let mut third = ModelSpend::in_dirs(config, state);
        assert_eq!(
            third.estimate("s", "/w"),
            [("Haiku 5.5".into(), 500.0)].into()
        );
    }

    #[test]
    fn stored_readings_keep_only_the_transcripts_asked_for() {
        let tmp = TempDir::new().unwrap();
        let (config, state) = (tmp.path().join("config"), tmp.path().join("state"));
        for id in ["a", "b"] {
            let main = transcript_path_in(&config, id, "/w");
            append(&main, &[reply(id, "claude-opus-5-5", 1_000_000, 0)]);
        }
        let mut spend = ModelSpend::in_dirs(config.clone(), state.clone());
        spend.estimate("a", "/w");
        spend.save();
        spend.estimate("b", "/w");
        spend.save();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(state.join(STORED_SPEND)).unwrap()).unwrap();
        let kept: Vec<&String> = saved.as_object().unwrap().keys().collect();
        let b = transcript_path_in(&config, "b", "/w");
        assert_eq!(kept, [&b.to_string_lossy().into_owned()]);
    }

    #[test]
    fn readings_that_are_not_stored_stay_off_the_disk() {
        let tmp = TempDir::new().unwrap();
        let (config, state) = (tmp.path().join("config"), tmp.path().join("state"));
        append(
            &transcript_path_in(&config, "s", "/w"),
            &[reply("m1", "claude-opus-5-5", 1, 0)],
        );
        let mut spend = ModelSpend::in_dir(config);
        spend.estimate("s", "/w");
        spend.save();
        assert!(!state.exists());
    }

    #[test]
    fn an_unreadable_stored_file_is_started_over() {
        let tmp = TempDir::new().unwrap();
        let (config, state) = (tmp.path().join("config"), tmp.path().join("state"));
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join(STORED_SPEND), b"{\"x\": [1").unwrap();
        let main = transcript_path_in(&config, "s", "/w");
        append(&main, &[reply("m1", "claude-opus-5-5", 1_000_000, 0)]);
        let mut spend = ModelSpend::in_dirs(config, state);
        assert_eq!(spend.estimate("s", "/w")["Opus 5.5"], 4.0);
    }

    fn saved_paths(state: &Path) -> Vec<String> {
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(state.join(STORED_SPEND)).unwrap()).unwrap();
        saved.as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn only_a_reading_that_counted_new_lines_is_saved_again() {
        let tmp = TempDir::new().unwrap();
        let (config, state) = (tmp.path().join("config"), tmp.path().join("state"));
        let main = transcript_path_in(&config, "s", "/w");
        append(&main, &[reply("m1", "claude-opus-5-5", 1_000_000, 0)]);
        let mut spend = ModelSpend::in_dirs(config, state.clone());
        spend.estimate("s", "/w");
        spend.save();
        fs::remove_file(state.join(STORED_SPEND)).unwrap();
        // A touch, then a line still being written: nothing new to keep
        set_mtime(&main, 7);
        spend.estimate("s", "/w");
        spend.save();
        let mut f = OpenOptions::new().append(true).open(&main).unwrap();
        write!(f, "{}", reply("m2", "claude-opus-5-5", 1_000_000, 0)).unwrap();
        spend.estimate("s", "/w");
        spend.save();
        assert!(!state.join(STORED_SPEND).exists());
        writeln!(f).unwrap();
        assert_eq!(spend.estimate("s", "/w")["Opus 5.5"], 8.0);
        spend.save();
        assert!(state.join(STORED_SPEND).exists());
    }

    #[test]
    fn a_transcript_replaced_behind_the_same_size_and_mtime_is_read_again() {
        let tmp = TempDir::new().unwrap();
        let (config, state) = (tmp.path().join("config"), tmp.path().join("state"));
        let main = transcript_path_in(&config, "s", "/w");
        append(&main, &[reply("m1", "claude-opus-5-5", 1_000_000, 0)]);
        set_mtime(&main, 5);
        let mut spend = ModelSpend::in_dirs(config.clone(), state.clone());
        spend.estimate("s", "/w");
        spend.save();
        // Another file moved in its place: same size and mtime, another inode
        let other = main.with_extension("new");
        append(&other, &[reply("m1", "claude-opus-5-1", 1_000_000, 0)]);
        set_mtime(&other, 5);
        fs::rename(&other, &main).unwrap();
        let mut again = ModelSpend::in_dirs(config, state);
        assert_eq!(again.estimate("s", "/w")["Opus 5.1"], 4.0);
    }

    #[test]
    fn missing_transcripts_are_not_kept() {
        let tmp = TempDir::new().unwrap();
        let (config, state) = (tmp.path().join("config"), tmp.path().join("state"));
        let main = transcript_path_in(&config, "a", "/w");
        append(&main, &[reply("m1", "claude-opus-5-5", 1, 0)]);
        let mut spend = ModelSpend::in_dirs(config, state.clone());
        spend.estimate("a", "/w");
        spend.estimate("ghost", "/w");
        spend.save();
        assert_eq!(saved_paths(&state), [main.to_string_lossy()]);
    }

    #[test]
    fn a_path_that_is_not_utf8_does_not_keep_the_others_off_the_disk() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let tmp = TempDir::new().unwrap();
        let (config, state) = (tmp.path().join("config"), tmp.path().join("state"));
        let main = transcript_path_in(&config, "s", "/w");
        append(&main, &[reply("m1", "claude-opus-5-5", 1_000_000, 0)]);
        let sub = main
            .with_extension("")
            .join("subagents")
            .join(OsStr::from_bytes(b"agent-\xff.jsonl"));
        append(&sub, &[reply("m2", "claude-sonnet-5-5", 0, 1_000_000)]);
        let mut spend = ModelSpend::in_dirs(config, state.clone());
        assert_eq!(spend.estimate("s", "/w")["Sonnet 5.5"], 10.0);
        spend.save();
        assert_eq!(saved_paths(&state), [main.to_string_lossy()]);
    }
}
