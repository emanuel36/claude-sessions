//! Black-box tests of `cst install` and `cst uninstall`.

mod support;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Output;

use cst::claude::install::{is_ours, EVENTS};
use serde_json::{json, Value};
use support::{command, env, start, TestEnv};

/// The binary under test, as `cst install` records it.
fn exe() -> PathBuf {
    fs::canonicalize(env!("CARGO_BIN_EXE_cst")).expect("canonical path of the binary")
}

fn run(env: &TestEnv, args: &[&str], legacy_state: Option<&Path>) -> Output {
    let mut cmd = command(env, args);
    // The test may run where the Python version's variables are set
    cmd.env_remove("XDG_STATE_HOME");
    match legacy_state {
        Some(dir) => cmd.env("CLAUDE_SESSIONS_STATE", dir),
        None => cmd.env("CLAUDE_SESSIONS_STATE", env.path().join("no-python-state")),
    };
    start(cmd, b"").wait_with_output().expect("wait for cst")
}

fn settings_path(env: &TestEnv) -> PathBuf {
    env.claude_dir().join("settings.json")
}

fn saved_path(env: &TestEnv) -> PathBuf {
    env.state_dir().join("statusline.json")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read file")).expect("json")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn foreign() -> Value {
    json!({"hooks": [{"type": "command", "command": "notify-me"}]})
}

fn commands_of(settings: &Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .expect("groups")
        .iter()
        .flat_map(|g| g["hooks"].as_array().expect("hooks").iter())
        .map(|h| h["command"].as_str().expect("command").to_owned())
        .collect()
}

#[test]
fn install_registers_the_running_binary() {
    let env = env();
    let out = run(&env, &["install"], None);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "hooks: added to 11 events\nstatus line: now cs statusline\n"
    );
    let settings = read_json(&settings_path(&env));
    let exe = exe();
    let exe = exe.to_str().expect("utf-8 path");
    for event in EVENTS {
        let commands = commands_of(&settings, event);
        assert_eq!(commands.len(), 1, "{event}");
        assert!(
            is_ours(&commands[0]) && commands[0].contains(exe),
            "{commands:?}"
        );
        assert!(commands[0].ends_with(" hook"));
    }
    let line = settings["statusLine"]["command"].as_str().expect("command");
    assert!(is_ours(line) && line.contains(exe) && line.ends_with(" statusline"));
    let again = run(&env, &["install"], None);
    assert_eq!(stdout(&again), "already installed\n");
}

#[test]
fn migrates_python_hooks_to_cst() {
    let env = env();
    let hooks: serde_json::Map<String, Value> = EVENTS
        .iter()
        .map(|e| {
            let entry = json!({
                "type": "command",
                "command": "/home/u/.local/bin/cs hook",
                "timeout": 10,
            });
            ((*e).to_owned(), json!([{"hooks": [entry]}]))
        })
        .collect();
    let python = json!({
        "hooks": hooks,
        "statusLine": {"type": "command", "command": "/home/u/.local/bin/cs statusline"},
    });
    fs::write(settings_path(&env), python.to_string()).expect("write settings");
    let out = run(&env, &["install"], None);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let settings = read_json(&settings_path(&env));
    let exe = exe();
    let exe = exe.to_str().expect("utf-8 path");
    for event in EVENTS {
        // The same group, pointed at cst: no second one beside it
        let commands = commands_of(&settings, event);
        assert_eq!(commands.len(), 1, "{event}");
        assert!(
            commands[0].contains(exe) && commands[0].ends_with(" hook"),
            "{commands:?}"
        );
    }
    let line = settings["statusLine"]["command"].as_str().expect("command");
    assert!(
        line.contains(exe) && line.ends_with(" statusline"),
        "{line}"
    );
}

#[test]
fn carries_over_the_python_saved_status_line() {
    let env = env();
    let python_state = env.path().join("python-state");
    fs::create_dir_all(&python_state).expect("mkdir");
    let mine = json!({"type": "command", "command": "~/.claude/statusline.sh"});
    fs::write(python_state.join("statusline.json"), mine.to_string()).expect("write saved");
    let python = json!({
        "statusLine": {"type": "command", "command": "/home/u/.local/bin/cs statusline"},
    });
    fs::write(settings_path(&env), python.to_string()).expect("write settings");

    let out = run(&env, &["install"], Some(&python_state));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(read_json(&saved_path(&env)), mine);

    // The chain survives: uninstalling hands the user's line back
    let out = run(&env, &["uninstall"], Some(&python_state));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(read_json(&settings_path(&env)), json!({"statusLine": mine}));
    assert!(!saved_path(&env).exists());
}

#[test]
fn keeps_unrelated_hooks_and_key_order() {
    let env = env();
    let original = json!({
        "zeta": 1,
        "model": "opus",
        "hooks": {"Stop": [foreign()], "Custom": [foreign()], "Another": []},
        "alpha": {"b": 1, "a": [3, 1, 2]},
        "statusLine": {"type": "command", "command": "mine.sh", "padding": 2},
        "omega": null,
    });
    let text = serde_json::to_string_pretty(&original).expect("json") + "\n";
    fs::write(settings_path(&env), &text).expect("write settings");

    assert_eq!(run(&env, &["install"], None).status.code(), Some(0));
    let installed: Value = read_json(&settings_path(&env));
    let top: Vec<&str> = installed
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        top,
        ["zeta", "model", "hooks", "alpha", "statusLine", "omega"]
    );
    let events: Vec<&str> = installed["hooks"]
        .as_object()
        .expect("hooks")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(events[..3], ["Stop", "Custom", "Another"]);
    assert_eq!(installed["hooks"]["Stop"][0], foreign());
    assert_eq!(installed["hooks"]["Custom"], json!([foreign()]));
    assert_eq!(installed["alpha"], original["alpha"]);
    assert_eq!(installed["statusLine"]["padding"], 2);

    // Taking ours out again gives back the file, byte for byte
    assert_eq!(run(&env, &["uninstall"], None).status.code(), Some(0));
    assert_eq!(fs::read_to_string(settings_path(&env)).expect("read"), text);
}

#[test]
fn follows_a_symlinked_settings_file() {
    let env = env();
    let real = env.path().join("dotfiles/settings.json");
    fs::create_dir_all(real.parent().expect("parent")).expect("mkdir");
    fs::write(&real, r#"{"model": "opus"}"#).expect("write settings");
    symlink(&real, settings_path(&env)).expect("symlink");

    assert_eq!(run(&env, &["install"], None).status.code(), Some(0));
    assert!(fs::symlink_metadata(settings_path(&env))
        .expect("meta")
        .is_symlink());
    assert!(read_json(&real).get("hooks").is_some());
    assert!(real
        .with_file_name("settings.json.before-claude-sessions")
        .is_file());

    assert_eq!(run(&env, &["uninstall"], None).status.code(), Some(0));
    assert!(fs::symlink_metadata(settings_path(&env))
        .expect("meta")
        .is_symlink());
    assert_eq!(read_json(&real), json!({"model": "opus"}));
}

#[test]
fn malformed_settings_change_nothing() {
    let env = env();
    let bad = [
        "{oops",
        "[]",
        r#"{"hooks": []}"#,
        r#"{"hooks": {"Stop": [{}]}}"#,
        r#"{"statusLine": "x.sh"}"#,
    ];
    for text in bad {
        fs::write(settings_path(&env), text).expect("write settings");
        let before = fs::read(settings_path(&env)).expect("read");
        for args in [["install"], ["uninstall"]] {
            let out = run(&env, &args, None);
            assert_eq!(out.status.code(), Some(1), "{text} {args:?}");
            let err = stderr(&out);
            assert!(
                err.starts_with("error: ") && err.contains("settings.json"),
                "{err}"
            );
            assert!(err.ends_with("; nothing was changed\n"), "{err}");
            assert_eq!(
                fs::read(settings_path(&env)).expect("read"),
                before,
                "{text}"
            );
        }
        assert!(!saved_path(&env).exists());
        assert!(!env
            .claude_dir()
            .join("settings.json.before-claude-sessions")
            .exists());
    }
}
