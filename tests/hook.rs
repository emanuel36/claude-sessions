//! Black-box tests of `cst hook`.

mod support;

use std::fs;
use std::path::Path;

use support::{command, cst, env, start, TestEnv};

fn error_lines(env: &TestEnv) -> usize {
    fs::read_to_string(env.state_dir().join("errors.log")).map_or(0, |log| log.lines().count())
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn record(env: &TestEnv, id: &str) -> serde_json::Value {
    let bytes = fs::read(env.runtime_dir().join(format!("{id}.json"))).expect("record file");
    serde_json::from_slice(&bytes).expect("record json")
}

#[test]
fn hook_survives_bad_input() {
    let env = env();
    let rejected: [&[u8]; 4] = [
        b"",
        b"nope",
        b"{}",
        br#"{"session_id":"../x","hook_event_name":"Stop"}"#,
    ];
    for input in rejected {
        let before = error_lines(&env);
        let out = cst(&env, &["hook"], input);
        let shown = String::from_utf8_lossy(input);
        assert_eq!(out.status.code(), Some(0), "{shown}");
        assert!(error_lines(&env) > before, "no error logged for {shown:?}");
        assert!(out.stdout.is_empty(), "{shown}");
    }
    // Nothing was written for any of them
    assert!(names(&env.runtime_dir()).is_empty());
    let accepted: [&[u8]; 2] = [
        br#"{"session_id":"s","hook_event_name":"Bogus"}"#,
        br#"{"session_id":"t","hook_event_name":"Stop","background_tasks":5}"#,
    ];
    for input in accepted {
        let before = error_lines(&env);
        let out = cst(&env, &["hook"], input);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(
            error_lines(&env),
            before,
            "{}",
            String::from_utf8_lossy(input)
        );
    }
    // No file outside the records directory but the error log and the journal
    assert_eq!(names(env.path()), ["claude", "runtime", "state"]);
    let records = names(&env.runtime_dir());
    assert!(
        records.iter().all(|n| n.starts_with(['s', 't'])),
        "{records:?}"
    );
    assert_eq!(names(&env.state_dir()), ["errors.log", "journal"]);
}

#[test]
fn hook_ignores_extra_arguments_and_exits_0() {
    let env = env();
    let input = br#"{"session_id":"s","hook_event_name":"SessionStart"}"#;
    let out = cst(&env, &["hook", "--whatever", "x"], input);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(record(&env, "s")["status"], "idle");
}

#[test]
fn hook_exits_0_when_the_records_directory_is_unusable() {
    let env = env();
    // A file where the records directory should be
    fs::remove_dir(env.runtime_dir()).expect("remove dir");
    fs::write(env.runtime_dir(), b"").expect("write file");
    let input = br#"{"session_id":"s","hook_event_name":"SessionStart"}"#;
    let out = cst(&env, &["hook"], input);
    assert_eq!(out.status.code(), Some(0));
    assert!(error_lines(&env) > 0);
}

#[test]
fn parallel_hooks_keep_every_pending_agent() {
    let env = env();
    let children: Vec<_> = (0..8)
        .map(|i| {
            let input = format!(
                r#"{{"session_id":"par","hook_event_name":"PermissionRequest","agent_id":"a{i}"}}"#
            );
            start(command(&env, &["hook"]), input.as_bytes())
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().expect("wait for cst");
        assert_eq!(out.status.code(), Some(0));
    }
    let r = record(&env, "par");
    assert_eq!(r["status"], "permission");
    let mut agents: Vec<&str> = r["pending"]
        .as_array()
        .expect("pending")
        .iter()
        .map(|p| p["agent"].as_str().expect("agent"))
        .collect();
    agents.sort_unstable();
    assert_eq!(agents, ["a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7"]);
    assert_eq!(error_lines(&env), 0);
}

#[test]
fn hook_records_the_claude_process_it_was_given() {
    let env = env();
    let mut cmd = command(&env, &["hook"]);
    cmd.env("CLAUDE_PID", std::process::id().to_string());
    let input = br#"{"session_id":"s","hook_event_name":"UserPromptSubmit","cwd":"/w/notes"}"#;
    let out = start(cmd, input).wait_with_output().expect("wait for cst");
    assert_eq!(out.status.code(), Some(0));
    let r = record(&env, "s");
    assert_eq!(r["pid"], std::process::id());
    assert_eq!(r["status"], "working");
    assert!(r["mux"].is_null());
}
