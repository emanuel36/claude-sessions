//! Black-box tests of the `cst` command line.

mod support;

use cst::core::paths::write_atomic;
use cst::core::state::{Mux, Record, Status};
use cst::core::store::process_start;
use support::{command, cst, env, start, PrivateTmux};

#[test]
fn focus_prints_nothing_and_exits_0_even_when_it_cannot_work() {
    let env = env();
    for args in [
        &["focus"][..],
        &["focus", "--pane", "%1"],
        &[
            "focus",
            "--pane",
            "%1",
            "--socket",
            "/nonexistent/tmux-sock",
        ],
    ] {
        let out = cst(&env, args, b"");
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{args:?}");
    }
    let log = std::fs::read_to_string(env.state_dir().join("errors.log")).unwrap_or_default();
    assert_eq!(log.matches("focus: bad arguments").count(), 2, "{log}");
}

#[test]
fn step_and_close_act_on_the_server_they_run_in() {
    let Some(tmux) = PrivateTmux::new() else {
        return;
    };
    let env = env();
    tmux.run(&["-f", "/dev/null", "new-session", "-d", "-s", "t"]);
    let new_window = || {
        let out = tmux.run(&[
            "new-window",
            "-P",
            "-F",
            "#{window_id} #{pane_id}",
            "-t",
            "t:",
        ]);
        let (window, pane) = out.trim().split_once(' ').expect("window and pane");
        (window.to_owned(), pane.to_owned())
    };
    let (session, pane) = new_window();
    let (other, _) = new_window();
    // A session needs the user in the first window; the second, current, has none
    let pid = i32::try_from(std::process::id()).expect("pid");
    let record = Record {
        id: "s1".into(),
        status: Status::Permission,
        pid: Some(pid),
        pid_start: process_start(pid),
        mux: Some(Mux {
            socket: tmux.socket_path().display().to_string(),
            pane,
        }),
        ..Record::default()
    };
    let json = serde_json::to_vec(&record).expect("record json");
    write_atomic(&env.runtime_dir().join("s1.json"), &json, None).expect("write record");
    let in_server = |args: &[&str]| {
        let mut cmd = command(&env, args);
        cmd.env("TMUX", format!("{},1,0", tmux.socket_path().display()));
        start(cmd, b"").wait_with_output().expect("wait for cst")
    };
    let current = || tmux.run(&["display-message", "-p", "-t", "t", "#{window_id}"]);
    assert_eq!(in_server(&["step", "down", &other]).status.code(), Some(0));
    assert_eq!(current().trim(), session);
    assert_eq!(in_server(&["close", &other]).status.code(), Some(0));
    let left = tmux.run(&["list-windows", "-F", "#{window_id}"]);
    assert!(!left.lines().any(|w| w == other), "{left}");
    assert_eq!(
        cst(&env, &["step", "sideways", "@1"], b"").status.code(),
        Some(2)
    );
}

#[test]
fn version_prints_2_0_0() {
    let out = cst(&env(), &["--version"], b"");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "cst 2.0.0\n");
}

#[test]
fn help_lists_only_public_commands() {
    let out = cst(&env(), &["--help"], b"");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    for shown in ["update", "uninstall", "--ghostty"] {
        assert!(text.contains(shown), "help should mention {shown}: {text}");
    }
    for hidden in ["hook", "statusline", "sidebar"] {
        assert!(
            !text.contains(hidden),
            "help should not mention {hidden}: {text}"
        );
    }
}

#[test]
fn unknown_command_exits_2() {
    let out = cst(&env(), &["frobnicate"], b"");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Usage"));
}

#[test]
fn non_utf8_argument_exits_2_without_panicking() {
    use std::os::unix::ffi::OsStrExt;

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cst"))
        .arg(std::ffi::OsStr::from_bytes(b"bad-\xff-arg"))
        .output()
        .expect("spawn cst");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown command"));
}

#[test]
fn private_tmux_runs_and_cleans_up() {
    let Some(tmux) = PrivateTmux::new() else {
        return;
    };
    tmux.run(&["new-session", "-d", "-s", "t"]);
    assert_eq!(
        tmux.run(&["list-sessions", "-F", "#{session_name}"]).trim(),
        "t"
    );
    let socket = tmux.socket_path();
    assert!(socket.exists());
    drop(tmux);
    assert!(!socket.exists());
}
