//! Black-box tests of `cst statusline`.

mod support;

use std::fs;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use support::{cst, env, TestEnv};

fn status_input() -> Value {
    json!({
        "session_id": "s1",
        "model": {"id": "claude-opus-4-1", "display_name": "Opus"},
        "cost": {"total_cost_usd": 1.5, "total_api_duration_ms": 60000, "total_lines_added": 3},
        "rate_limits": {"five_hour": {"used_percentage": 40, "resets_at": 4_000_000_000_u64}},
    })
}

fn statusline(env: &TestEnv, stdin: &[u8]) -> std::process::Output {
    let out = cst(env, &["statusline"], stdin);
    assert_eq!(out.status.code(), Some(0));
    out
}

fn save_statusline(env: &TestEnv, command: &str) {
    let saved = json!({"type": "command", "command": command, "padding": 0});
    fs::write(env.state_dir().join("statusline.json"), saved.to_string()).expect("write setting");
}

fn start_session(env: &TestEnv, id: &str) {
    let input = json!({"session_id": id, "hook_event_name": "SessionStart"}).to_string();
    assert_eq!(cst(env, &["hook"], input.as_bytes()).status.code(), Some(0));
}

fn record(env: &TestEnv, id: &str) -> Value {
    let bytes = fs::read(env.runtime_dir().join(format!("{id}.json"))).expect("record file");
    serde_json::from_slice(&bytes).expect("record json")
}

fn limits(env: &TestEnv) -> Value {
    let bytes = fs::read(env.runtime_dir().join("limits.json")).expect("limits file");
    serde_json::from_slice(&bytes).expect("limits json")
}

fn error_log(env: &TestEnv) -> String {
    fs::read_to_string(env.state_dir().join("errors.log")).unwrap_or_default()
}

/// The journal lines that carry a cost.
fn cost_events(env: &TestEnv) -> Vec<Value> {
    let Ok(days) = fs::read_dir(env.state_dir().join("journal")) else {
        return Vec::new();
    };
    days.flat_map(|day| {
        let text = fs::read_to_string(day.expect("entry").path()).expect("read day");
        text.lines()
            .map(|l| serde_json::from_str::<Value>(l).expect("journal line"))
            .filter(|e| e.get("cost").is_some())
            .collect::<Vec<_>>()
    })
    .collect()
}

#[test]
fn unknown_session_still_chains() {
    let env = env();
    save_statusline(&env, "echo hi");
    let out = statusline(&env, status_input().to_string().as_bytes());
    assert_eq!(out.stdout, b"hi\n");
    assert!(!env.runtime_dir().join("s1.json").exists());
}

#[test]
fn chained_command_gets_the_same_stdin() {
    let env = env();
    save_statusline(&env, "cat");
    let stdin = status_input().to_string();
    assert_eq!(statusline(&env, stdin.as_bytes()).stdout, stdin.as_bytes());
}

#[test]
fn non_json_stdin_prints_and_exits_0() {
    let env = env();
    assert_eq!(statusline(&env, b"not json").stdout, b"\n");
    assert!(!error_log(&env).is_empty());
    save_statusline(&env, "echo hi");
    assert_eq!(statusline(&env, b"not json").stdout, b"hi\n");
    assert_eq!(statusline(&env, b"").stdout, b"hi\n");
    assert_eq!(statusline(&env, b"[1]").stdout, b"hi\n");
}

#[test]
fn records_usage_without_touching_status() {
    let env = env();
    start_session(&env, "s1");
    let before = record(&env, "s1");
    let out = statusline(&env, status_input().to_string().as_bytes());
    assert_eq!(out.stdout, b"Opus  $1.50\n");
    let after = record(&env, "s1");
    assert_eq!(
        after["usage"],
        json!({"cost": 1.5, "api_duration": 60.0, "model": "Opus", "added": 3, "removed": 0})
    );
    assert_eq!(after["status"], before["status"]);
    assert_eq!(after["status_since"], before["status_since"]);
    assert_eq!(limits(&env)["five_hour"]["pct"], 40.0);
    assert_eq!(limits(&env)["five_hour"]["resets_at"], 4e9);
}

#[test]
fn default_line_is_empty_until_cost_is_known() {
    let env = env();
    let mut early = status_input();
    early["cost"] = Value::Null;
    assert_eq!(statusline(&env, early.to_string().as_bytes()).stdout, b"\n");
}

#[test]
fn logs_the_cost_only_when_it_changes() {
    let env = env();
    start_session(&env, "s1");
    let stdin = status_input().to_string();
    statusline(&env, stdin.as_bytes());
    statusline(&env, stdin.as_bytes());
    let events = cost_events(&env);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["id"], json!("s1"));
    assert_eq!(events[0]["cost"], json!(1.5));
}

#[test]
fn cost_events_carry_non_negative_integer_lines() {
    let env = env();
    start_session(&env, "s1");
    let mut data = status_input();
    data["cost"] =
        json!({"total_cost_usd": 1.0, "total_lines_added": 10, "total_lines_removed": 2.7});
    statusline(&env, data.to_string().as_bytes());
    let events = cost_events(&env);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["added"], json!(10));
    assert_eq!(events[0]["removed"], json!(2));
    assert_eq!(events[0]["model"], json!("Opus"));
}

#[test]
fn without_a_valid_session_the_limits_are_still_saved() {
    let env = env();
    for id in [Value::Null, json!("../s1")] {
        let mut data = status_input();
        data["session_id"] = id;
        statusline(&env, data.to_string().as_bytes());
    }
    assert_eq!(limits(&env)["five_hour"]["pct"], 40.0);
    assert_eq!(error_log(&env), "");
}

/// The bound leaves 3 s over the 5 s timeout: a loaded host can be slow to start the chain.
#[test]
fn chain_is_killed_after_five_seconds() {
    let env = env();
    save_statusline(&env, "sleep 10; echo late");
    let start = Instant::now();
    let out = statusline(&env, status_input().to_string().as_bytes());
    assert!(
        start.elapsed() < Duration::from_secs(8),
        "{:?}",
        start.elapsed()
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn an_unusable_records_directory_still_prints() {
    let env = env();
    fs::remove_dir(env.runtime_dir()).expect("remove dir");
    fs::write(env.runtime_dir(), b"").expect("write file");
    let out = statusline(&env, status_input().to_string().as_bytes());
    assert_eq!(out.stdout, b"Opus  $1.50\n");
    assert!(!error_log(&env).is_empty());
}
