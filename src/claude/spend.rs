//! What each model cost in a session, estimated from its transcripts.
//!
//! Claude Code reports one running cost per session, under the model of its main conversation,
//! though subagents may run on other models. The transcripts (the session's own and its
//! subagents') record each reply's model and tokens; priced with [`price`], they give the share
//! of each model, which splits the reported cost. The prices only weigh the models against each
//! other, so the reported total never changes.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::claude::transcript::transcript_path_in;
use crate::core::paths;

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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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

/// How far a transcript was read, and what its replies added up to.
#[derive(Debug, Default)]
struct Reading {
    offset: u64,
    seen: HashSet<String>,
    tokens: HashMap<String, Tokens>,
}

impl Reading {
    /// Reads the lines added since the last time; a file shorter than that is read again.
    fn catch_up(&mut self, path: &Path) {
        let Ok(mut file) = File::open(path) else {
            *self = Reading::default();
            return;
        };
        let Ok(len) = file.seek(SeekFrom::End(0)) else {
            return;
        };
        if len < self.offset {
            *self = Reading::default();
        }
        if len == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_err() {
            return;
        }
        // A line still being written waits for the next reading
        let Some(end) = bytes.iter().rposition(|&b| b == b'\n') else {
            return;
        };
        for line in bytes[..end].split(|&b| b == b'\n') {
            let Some((key, model, tokens)) = reply_of(line) else {
                continue;
            };
            if let Some(key) = key {
                if !self.seen.insert(key) {
                    continue;
                }
            }
            self.tokens.entry(model).or_default().add(tokens);
        }
        self.offset += end as u64 + 1;
    }
}

/// Estimates what each model cost in each session, reading the transcripts incrementally.
#[derive(Debug)]
pub struct ModelSpend {
    config: Option<PathBuf>,
    readings: HashMap<PathBuf, Reading>,
}

impl Default for ModelSpend {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelSpend {
    /// Returns an estimator over Claude Code's own config folder.
    pub fn new() -> Self {
        ModelSpend {
            config: None,
            readings: HashMap::new(),
        }
    }

    /// Returns an estimator over the config folder `config`.
    #[doc(hidden)]
    pub fn in_dir(config: PathBuf) -> Self {
        ModelSpend {
            config: Some(config),
            readings: HashMap::new(),
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
        let mut tokens: HashMap<String, Tokens> = HashMap::new();
        for path in session_files(&config, id, cwd) {
            let reading = self.readings.entry(path.clone()).or_default();
            reading.catch_up(&path);
            for (model, t) in &reading.tokens {
                tokens.entry(model.clone()).or_default().add(*t);
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
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::Path;

    use serde_json::json;
    use tempfile::TempDir;

    use crate::claude::spend::{display_name, price, ModelSpend, Price};
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
}
