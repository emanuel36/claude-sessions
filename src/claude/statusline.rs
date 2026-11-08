//! `cst statusline`: what its input tells, recorded, and the user's own status line after it.
//!
//! The input tells a session's cost, time, model and lines, and the account's limits. Claude
//! Code sends these numbers only to the status line command, as JSON on its stdin. Any of them
//! may be missing or null, early in a session above all.
//!
//! Each public function is a thin wrapper over one that takes the directories, the journal and
//! the command, so unit tests use a temporary directory and never touch the process environment
//! or the real journal.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rustix::process::{kill_process_group, Pid, Signal};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::journal::{self, JournalEvent};
use crate::core::paths::{self, LIMITS_FILE};
use crate::core::state::Usage;
use crate::core::store::{self, Sink};
use crate::process;

/// The account's usage windows, as the status line input names them.
pub const WINDOWS: [&str; 2] = ["five_hour", "seven_day"];
/// Claude Code cancels a status line that runs past its next update anyway.
pub const CHAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// How much of a usage window is used, and when the window resets.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    /// Percent of the window used.
    pub pct: f64,
    /// Unix timestamp at which the window resets.
    pub resets_at: f64,
}

/// Walks nested objects, giving `None` where a level is missing or not an object.
fn get<'a>(data: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().try_fold(data, |value, key| value.get(*key))
}

/// A JSON number; a bool or a string is never a count or an amount here.
fn number(value: Option<&Value>) -> Option<f64> {
    value?.as_f64()
}

/// A non-empty string.
fn text(value: Option<&Value>) -> Option<&str> {
    value?.as_str().filter(|s| !s.is_empty())
}

/// Returns a session's usage from the status line input.
///
/// `cost` (USD) and `api_duration` (seconds Claude spent answering) are `None` until known;
/// `model` is the display name or else the id, `None` until known; `added` and `removed` lines
/// are 0 until known. A fraction of a line is dropped and a negative count is 0.
pub fn parse(data: &Value) -> Usage {
    let cost = |key| number(get(data, &["cost", key]));
    // A float-to-int `as` saturates, and maps a negative to 0
    let lines = |key| cost(key).map_or(0, |n| n as u64);
    Usage {
        cost: cost("total_cost_usd"),
        api_duration: cost("total_api_duration_ms").map(|ms| ms / 1000.0),
        model: text(get(data, &["model", "display_name"]))
            .or_else(|| text(get(data, &["model", "id"])))
            .map(str::to_owned),
        added: lines("total_lines_added"),
        removed: lines("total_lines_removed"),
    }
}

/// Returns the usage windows the status line input has.
///
/// A window needs both a used percentage and a reset time; `resets_at` in the input is a Unix
/// timestamp.
pub fn parse_limits(data: &Value) -> BTreeMap<String, Quota> {
    WINDOWS
        .iter()
        .filter_map(|name| {
            let field = |key| number(get(data, &["rate_limits", name, key]));
            let quota = Quota {
                pct: field("used_percentage")?,
                resets_at: field("resets_at")?,
            };
            Some(((*name).to_owned(), quota))
        })
        .collect()
}

/// Returns the saved usage windows that have not reset yet; the account's, so every session's
/// status line reads the same file in the records directory.
///
/// `now` is the current time as a Unix timestamp.
pub fn load_limits(now: f64) -> BTreeMap<String, Quota> {
    match paths::runtime_dir() {
        Ok(dir) => load_limits_in(&dir, now),
        Err(_) => BTreeMap::new(),
    }
}

pub(crate) fn load_limits_in(dir: &Path, now: f64) -> BTreeMap<String, Quota> {
    let Ok(bytes) = fs::read(dir.join(LIMITS_FILE)) else {
        return BTreeMap::new();
    };
    let Ok(saved) = serde_json::from_slice::<Value>(&bytes) else {
        return BTreeMap::new();
    };
    WINDOWS
        .iter()
        .filter_map(|name| {
            let window = saved.get(name)?;
            let quota = Quota {
                pct: number(window.get("pct"))?,
                resets_at: number(window.get("resets_at"))?,
            };
            (quota.resets_at > now).then(|| ((*name).to_owned(), quota))
        })
        .collect()
}

/// Saves usage windows; each replaces the saved one of its name, the others stay.
///
/// The limits are the account's, so every session's status line updates the same file. A
/// session that has not updated its status line for a while reports an older, lower use of the
/// same window: for a window that resets at the same time, the higher use stays.
///
/// # Errors
///
/// Fails if the records directory is unusable or the file cannot be written.
pub fn save_limits(limits: &BTreeMap<String, Quota>, now: f64) -> io::Result<()> {
    save_limits_in(&paths::runtime_dir()?, limits, now)
}

pub(crate) fn save_limits_in(
    dir: &Path,
    limits: &BTreeMap<String, Quota>,
    now: f64,
) -> io::Result<()> {
    if limits.is_empty() {
        return Ok(());
    }
    let mut saved = load_limits_in(dir, now);
    for (name, window) in limits {
        let keep_old = saved
            .get(name)
            .is_some_and(|old| old.resets_at == window.resets_at && old.pct >= window.pct);
        if !keep_old {
            saved.insert(name.clone(), *window);
        }
    }
    let data = serde_json::to_vec(&saved).map_err(io::Error::other)?;
    paths::write_atomic(&dir.join(LIMITS_FILE), &data, None)
}

/// Where `cst` keeps the user's own `statusLine` setting, to run it in a chain.
pub fn saved_statusline_path() -> PathBuf {
    paths::state_dir().join("statusline.json")
}

/// Returns the command of the user's own status line, or `None` if there is none.
pub fn saved_command() -> Option<String> {
    saved_command_in(&saved_statusline_path())
}

pub(crate) fn saved_command_in(path: &Path) -> Option<String> {
    let saved: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    text(saved.get("command")).map(str::to_owned)
}

/// Stores the account's limits, and a session's usage on its record, if it has one.
///
/// A cost that differs from the record's goes to the journal as well. Both parts are tried
/// even if the first fails.
///
/// # Errors
///
/// Fails if the records directory is unusable, or the limits or the record cannot be written.
pub fn record_usage(data: &Value, now: f64) -> io::Result<()> {
    record_usage_in(&paths::runtime_dir()?, &journal::append, data, now)
}

pub(crate) fn record_usage_in(
    dir: &Path,
    journal: Sink<'_>,
    data: &Value,
    now: f64,
) -> io::Result<()> {
    let limits = save_limits_in(dir, &parse_limits(data), now);
    let session = record_session_usage(dir, journal, data, now);
    limits.and(session)
}

fn record_session_usage(dir: &Path, journal: Sink<'_>, data: &Value, now: f64) -> io::Result<()> {
    let Some(id) = text(data.get("session_id")).filter(|id| store::is_session_id(id)) else {
        return Ok(());
    };
    // A session with no record yet needs no lock file: its next update will find the record
    if store::read_in(dir, id).is_none() {
        return Ok(());
    }
    let _lock = store::lock_in(dir, id)?;
    let Some(mut record) = store::read_in(dir, id) else {
        return Ok(());
    };
    let before = record.usage.as_ref().and_then(|u| u.cost);
    let usage = parse(data);
    record.usage = Some(usage.clone());
    store::write_in(dir, &record, journal)?;
    if usage.cost.is_some() && usage.cost != before {
        journal(&JournalEvent {
            t: now,
            id: id.to_owned(),
            cost: usage.cost,
            added: Some(usage.added),
            removed: Some(usage.removed),
            model: usage.model,
            ..JournalEvent::default()
        });
    }
    Ok(())
}

/// Prints the user's own status line, given the same input, or else a short default.
///
/// The saved command runs with `sh -c`, gets `raw` on its stdin, and has its stdout passed on
/// byte for byte; its stderr is shared. It is killed, with whatever it started, after
/// [`CHAIN_TIMEOUT`], and then nothing is printed. Without a saved command the line is the model
/// and cost, or empty until both are known.
///
/// # Errors
///
/// Fails if the command cannot start or the output cannot be written.
pub fn print_statusline(raw: &[u8], data: &Value, out: &mut dyn Write) -> io::Result<()> {
    print_statusline_with(saved_command().as_deref(), raw, data, out, CHAIN_TIMEOUT)
}

pub(crate) fn print_statusline_with(
    command: Option<&str>,
    raw: &[u8],
    data: &Value,
    out: &mut dyn Write,
    timeout: Duration,
) -> io::Result<()> {
    let Some(command) = command else {
        let usage = parse(data);
        if let (Some(model), Some(cost)) = (&usage.model, usage.cost) {
            write!(out, "{model}  ${cost:.2}")?;
        }
        return out.write_all(b"\n");
    };
    match run_chain(command, raw, timeout)? {
        Some(output) => {
            out.write_all(&output)?;
            out.flush()
        }
        None => Ok(()),
    }
}

/// Runs `sh -c command` with `raw` on its stdin; returns its stdout, or `None` if it did not
/// finish within `timeout` and was killed.
fn run_chain(command: &str, raw: &[u8], timeout: Duration) -> io::Result<Option<Vec<u8>>> {
    let mut cmd = Command::new("sh");
    // Its own process group, so a timeout can kill what the command started as well
    cmd.args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .process_group(0);
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + timeout;
    let started = match (child.stdin.take(), child.stdout.take()) {
        (Some(stdin), Some(stdout)) => feed_and_collect(stdin, stdout, raw.to_vec()),
        _ => Err(io::Error::other("no pipe to the status line command")),
    };
    let rx = match started {
        Ok(rx) => rx,
        Err(e) => {
            stop(&mut child);
            return Err(e);
        }
    };
    let output = process::collect(&mut child, &rx, timeout, deadline);
    if output.is_none() {
        stop(&mut child);
    }
    Ok(output)
}

/// Writes `input` to the child's stdin and reads its stdout, each on a thread of its own, so
/// that neither a command that ignores its input nor one that prints a lot can stall the other.
/// The threads end by themselves when the child is killed.
fn feed_and_collect(
    mut stdin: ChildStdin,
    stdout: ChildStdout,
    input: Vec<u8>,
) -> io::Result<mpsc::Receiver<Vec<u8>>> {
    process::spawn_thread(Box::new(move || {
        // The command may exit without reading its input; a broken pipe is not a failure
        let _ = stdin.write_all(&input);
    }))?;
    process::read_on_thread(stdout, &process::spawn_thread)
}

/// Kills the child's whole process group and reaps the child.
fn stop(child: &mut Child) {
    if let Some(group) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) {
        // The group may be gone already
        let _ = kill_process_group(group, Signal::KILL);
    }
    process::give_up(child);
}

/// Records what the status line input tells, then prints the status line; never fails.
///
/// Claude Code cancels this command when the next update comes: record first, print after.
/// Both parts log their failures instead of stopping the other.
pub fn run(raw: &[u8], now: f64, out: &mut dyn Write) {
    let data = match serde_json::from_slice::<Value>(raw) {
        Ok(data) if data.is_object() => {
            if let Err(e) = record_usage(&data, now) {
                paths::log_error(&format_args!("statusline: cannot record usage: {e}"));
            }
            data
        }
        Ok(_) => {
            paths::log_error(&"statusline: input is not a JSON object");
            Value::Null
        }
        Err(e) => {
            paths::log_error(&format_args!("statusline: input is not JSON: {e}"));
            Value::Null
        }
    };
    if let Err(e) = print_statusline(raw, &data, out) {
        paths::log_error(&format_args!("statusline: cannot print: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use serde_json::{json, Value};
    use tempfile::TempDir;

    use crate::claude::statusline::{
        load_limits_in, parse, parse_limits, print_statusline_with, record_usage_in,
        save_limits_in, saved_command_in, saved_statusline_path, Quota,
    };
    use crate::core::journal::JournalEvent;
    use crate::core::paths::{state_dir, LIMITS_FILE};
    use crate::core::state::{new_record, Record, Status, Usage};
    use crate::core::store::{read_in, write_in};

    /// The status line input example from the Claude Code docs, trimmed to what usage reads.
    fn full() -> Value {
        json!({
            "session_id": "abc123",
            "model": {"id": "claude-opus-4-1", "display_name": "Opus"},
            "cost": {
                "total_cost_usd": 0.01234,
                "total_duration_ms": 45000,
                "total_api_duration_ms": 2300,
                "total_lines_added": 156,
                "total_lines_removed": 23,
            },
            "rate_limits": {
                "five_hour": {"used_percentage": 23.5, "resets_at": 1738425600},
                "seven_day": {"used_percentage": 41.2, "resets_at": 1738857600},
            },
        })
    }

    fn status_input() -> Value {
        json!({
            "session_id": "s1",
            "model": {"id": "claude-opus-4-1", "display_name": "Opus"},
            "cost": {"total_cost_usd": 1.5, "total_api_duration_ms": 60000, "total_lines_added": 3},
            "rate_limits": {"five_hour": {"used_percentage": 40, "resets_at": 4_000_000_000_u64}},
        })
    }

    fn quota(pct: f64, resets_at: f64) -> Quota {
        Quota { pct, resets_at }
    }

    fn limits(entries: &[(&str, Quota)]) -> BTreeMap<String, Quota> {
        entries.iter().map(|(n, q)| ((*n).to_owned(), *q)).collect()
    }

    /// Runs `record_usage_in` and returns the journal events it wrote.
    fn record(dir: &Path, data: &Value, now: f64) -> Vec<JournalEvent> {
        let events = RefCell::new(Vec::new());
        record_usage_in(dir, &|e| events.borrow_mut().push(e.clone()), data, now).unwrap();
        events.into_inner()
    }

    fn print(command: Option<&str>, raw: &[u8], data: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        print_statusline_with(command, raw, data, &mut out, Duration::from_secs(5)).unwrap();
        out
    }

    #[test]
    fn parse_full_input() {
        assert_eq!(
            parse(&full()),
            Usage {
                cost: Some(0.01234),
                api_duration: Some(2.3),
                model: Some("Opus".into()),
                added: 156,
                removed: 23,
            }
        );
        assert_eq!(
            parse_limits(&full()),
            limits(&[
                ("five_hour", quota(23.5, 1_738_425_600.0)),
                ("seven_day", quota(41.2, 1_738_857_600.0)),
            ])
        );
    }

    #[test]
    fn parse_early_session_nulls() {
        let data =
            json!({"session_id": "abc123", "model": {"id": "claude-opus-4-1"}, "cost": null});
        assert_eq!(
            parse(&data),
            Usage {
                model: Some("claude-opus-4-1".into()),
                ..Usage::default()
            }
        );
        assert!(parse_limits(&data).is_empty());
    }

    #[test]
    fn parse_ignores_values_of_the_wrong_type() {
        let data =
            json!({"model": "Opus", "cost": {"total_cost_usd": "1.2", "total_lines_added": true}});
        assert_eq!(parse(&data), Usage::default());
        assert!(parse_limits(&json!({"rate_limits": {"five_hour": null}})).is_empty());
    }

    #[test]
    fn bool_is_not_a_number() {
        let data = json!({"cost": {"total_cost_usd": true, "total_api_duration_ms": false}});
        let usage = parse(&data);
        assert_eq!((usage.cost, usage.api_duration), (None, None));
        let window =
            json!({"rate_limits": {"five_hour": {"used_percentage": true, "resets_at": 9}}});
        assert!(parse_limits(&window).is_empty());
    }

    #[test]
    fn parse_clamps_negative_lines_and_skips_an_empty_model_name() {
        let data = json!({
            "model": {"display_name": "", "id": "m"},
            "cost": {"total_lines_added": -4, "total_lines_removed": 2.9},
        });
        let usage = parse(&data);
        assert_eq!((usage.added, usage.removed), (0, 2));
        assert_eq!(usage.model.as_deref(), Some("m"));
    }

    #[test]
    fn parse_of_a_non_object_is_empty() {
        assert_eq!(parse(&Value::Null), Usage::default());
        assert!(parse_limits(&json!([1, 2])).is_empty());
    }

    #[test]
    fn parse_limits_partial() {
        let data = json!({"rate_limits": {"seven_day": {"used_percentage": 12, "resets_at": 500}}});
        assert_eq!(
            parse_limits(&data),
            limits(&[("seven_day", quota(12.0, 500.0))])
        );
    }

    #[test]
    fn save_limits_replaces_only_the_windows_it_gets() {
        let dir = TempDir::new().unwrap();
        let five = |pct| limits(&[("five_hour", quota(pct, 200.0))]);
        save_limits_in(dir.path(), &five(10.0), 100.0).unwrap();
        let seven = limits(&[("seven_day", quota(30.0, 900.0))]);
        save_limits_in(dir.path(), &seven, 110.0).unwrap();
        save_limits_in(dir.path(), &five(15.0), 120.0).unwrap();
        assert_eq!(
            load_limits_in(dir.path(), 150.0),
            limits(&[
                ("five_hour", quota(15.0, 200.0)),
                ("seven_day", quota(30.0, 900.0)),
            ])
        );
        assert!(dir.path().join(LIMITS_FILE).is_file());
    }

    #[test]
    fn load_limits_drops_expired() {
        let dir = TempDir::new().unwrap();
        let both = limits(&[
            ("five_hour", quota(90.0, 200.0)),
            ("seven_day", quota(30.0, 900.0)),
        ]);
        save_limits_in(dir.path(), &both, 100.0).unwrap();
        assert_eq!(
            load_limits_in(dir.path(), 200.0),
            limits(&[("seven_day", quota(30.0, 900.0))])
        );
        assert!(load_limits_in(dir.path(), 900.0).is_empty());
    }

    #[test]
    fn load_limits_with_a_broken_file_is_empty() {
        let dir = TempDir::new().unwrap();
        assert!(load_limits_in(dir.path(), 0.0).is_empty());
        fs::write(dir.path().join(LIMITS_FILE), "{").unwrap();
        assert!(load_limits_in(dir.path(), 0.0).is_empty());
    }

    #[test]
    fn load_limits_drops_a_window_without_a_numeric_pct() {
        let dir = TempDir::new().unwrap();
        let saved = json!({
            "five_hour": {"pct": true, "resets_at": 900},
            "seven_day": {"pct": "40", "resets_at": 900},
        });
        fs::write(dir.path().join(LIMITS_FILE), saved.to_string()).unwrap();
        assert!(load_limits_in(dir.path(), 0.0).is_empty());
    }

    #[test]
    fn load_limits_ignores_windows_it_does_not_know() {
        let dir = TempDir::new().unwrap();
        let saved = json!({"monthly": {"pct": 1, "resets_at": 900}});
        fs::write(dir.path().join(LIMITS_FILE), saved.to_string()).unwrap();
        assert!(load_limits_in(dir.path(), 0.0).is_empty());
    }

    #[test]
    fn save_limits_keeps_the_higher_use_of_the_same_window() {
        let dir = TempDir::new().unwrap();
        let five = |pct, resets| limits(&[("five_hour", quota(pct, resets))]);
        save_limits_in(dir.path(), &five(40.0, 200.0), 100.0).unwrap();
        // Another session's status line, not updated for a while
        save_limits_in(dir.path(), &five(30.0, 200.0), 110.0).unwrap();
        assert_eq!(load_limits_in(dir.path(), 150.0), five(40.0, 200.0));
        // A new window starts low
        save_limits_in(dir.path(), &five(5.0, 500.0), 210.0).unwrap();
        assert_eq!(load_limits_in(dir.path(), 250.0), five(5.0, 500.0));
    }

    #[test]
    fn saved_statusline_path_is_in_the_state_dir() {
        assert_eq!(saved_statusline_path(), state_dir().join("statusline.json"));
    }

    #[test]
    fn saved_command_reads_the_saved_statusline() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("statusline.json");
        assert_eq!(saved_command_in(&path), None);
        let saved = json!({"type": "command", "command": "echo hi", "padding": 0});
        fs::write(&path, saved.to_string()).unwrap();
        assert_eq!(saved_command_in(&path).as_deref(), Some("echo hi"));
        fs::write(&path, "[").unwrap();
        assert_eq!(saved_command_in(&path), None);
        fs::write(&path, json!({"command": ""}).to_string()).unwrap();
        assert_eq!(saved_command_in(&path), None);
    }

    #[test]
    fn unknown_session_still_saves_the_limits() {
        let dir = TempDir::new().unwrap();
        assert!(record(dir.path(), &status_input(), 100.0).is_empty());
        assert!(read_in(dir.path(), "s1").is_none());
        assert!(!dir.path().join("s1.json.lock").exists());
        assert_eq!(
            load_limits_in(dir.path(), 0.0),
            limits(&[("five_hour", quota(40.0, 4e9))])
        );
    }

    #[test]
    fn records_usage_without_touching_status() {
        let dir = TempDir::new().unwrap();
        let before = Record {
            status: Status::Permission,
            status_since: 7.0,
            ..new_record("s1")
        };
        write_in(dir.path(), &before, &|_| {}).unwrap();
        record(dir.path(), &status_input(), 100.0);
        let after = read_in(dir.path(), "s1").unwrap();
        assert_eq!(
            after,
            Record {
                usage: Some(Usage {
                    cost: Some(1.5),
                    api_duration: Some(60.0),
                    model: Some("Opus".into()),
                    added: 3,
                    removed: 0,
                }),
                ..before
            }
        );
    }

    #[test]
    fn logs_the_cost_only_when_it_changes() {
        let dir = TempDir::new().unwrap();
        let rec = Record {
            status_since: 7.0,
            ..new_record("s1")
        };
        write_in(dir.path(), &rec, &|_| {}).unwrap();
        let first = record(dir.path(), &status_input(), 100.0);
        let second = record(dir.path(), &status_input(), 101.0);
        assert_eq!(first.len(), 1);
        assert_eq!((first[0].id.as_str(), first[0].cost), ("s1", Some(1.5)));
        assert!(second.is_empty());
    }

    #[test]
    fn cost_events_carry_lines_and_model() {
        let dir = TempDir::new().unwrap();
        write_in(dir.path(), &new_record("s1"), &|_| {}).unwrap();
        let mut data = status_input();
        data["cost"] =
            json!({"total_cost_usd": 1.0, "total_lines_added": 10, "total_lines_removed": 2});
        let events = record(dir.path(), &data, 100.0);
        assert_eq!(
            events,
            vec![JournalEvent {
                t: 100.0,
                id: "s1".into(),
                cost: Some(1.0),
                added: Some(10),
                removed: Some(2),
                model: Some("Opus".into()),
                ..JournalEvent::default()
            }]
        );
    }

    #[test]
    fn no_cost_yet_logs_nothing() {
        let dir = TempDir::new().unwrap();
        write_in(dir.path(), &new_record("s1"), &|_| {}).unwrap();
        let mut data = status_input();
        data["cost"] = Value::Null;
        assert!(record(dir.path(), &data, 100.0).is_empty());
    }

    #[test]
    fn a_session_id_that_cannot_name_a_record_still_saves_the_limits() {
        for id in [Value::Null, json!("../s1"), json!(7)] {
            let dir = TempDir::new().unwrap();
            let mut data = status_input();
            data["session_id"] = id;
            assert!(record(dir.path(), &data, 100.0).is_empty());
            assert_eq!(load_limits_in(dir.path(), 0.0).len(), 1);
        }
    }

    #[test]
    fn default_line_needs_model_and_cost() {
        assert_eq!(print(None, b"", &status_input()), b"Opus  $1.50\n");
        let early = json!({"model": {"display_name": "Opus"}});
        assert_eq!(print(None, b"", &early), b"\n");
        let no_model = json!({"cost": {"total_cost_usd": 1}});
        assert_eq!(print(None, b"", &no_model), b"\n");
        assert_eq!(print(None, b"", &Value::Null), b"\n");
    }

    #[test]
    fn chained_command_gets_the_same_stdin_and_its_stdout_unchanged() {
        let raw = r#"{"a": "café"}  "#.as_bytes();
        assert_eq!(print(Some("cat"), raw, &Value::Null), raw);
        assert_eq!(print(Some("printf 'a\\0b'"), b"", &Value::Null), b"a\0b");
    }

    #[test]
    fn chained_command_can_ignore_a_large_stdin() {
        let raw = vec![b'x'; 1 << 20];
        assert_eq!(print(Some("echo hi"), &raw, &Value::Null), b"hi\n");
    }

    #[test]
    fn chained_command_prints_even_when_it_fails() {
        assert_eq!(print(Some("echo hi; exit 3"), b"", &Value::Null), b"hi\n");
    }

    #[test]
    fn a_slow_chain_is_killed_and_prints_nothing() {
        let mut out = Vec::new();
        let start = Instant::now();
        print_statusline_with(
            Some("sleep 10; echo late"),
            b"",
            &Value::Null,
            &mut out,
            Duration::from_millis(300),
        )
        .unwrap();
        assert!(out.is_empty());
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
