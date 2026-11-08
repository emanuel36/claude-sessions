//! `cst`'s launcher against a private tmux server, never `tmux -L claude`, and never attached.

mod support;

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use cst::term::launcher::{
    conf_stamp, conf_text, ghostty_command, search_path, write_conf, LaunchError, Server, CONF,
    LANDING, SESSION,
};
use cst::term::tmux::SIDEBAR_COMMAND;
use support::{env, write_program, PrivateTmux, TestEnv};

/// The folder of the built `cst`, which the server's hooks and windows must run.
fn bin_dir() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_cst"))
        .parent()
        .expect("binary dir")
        .to_path_buf()
}

/// A launcher for the private server `tmux`, with the built `cst` first on the PATH and the
/// three env vars pointed at `env`.
fn server(tmux: &PrivateTmux, env: &TestEnv) -> Server {
    let path = search_path(&bin_dir(), std::env::var_os("PATH").as_deref());
    Server {
        program: "tmux".into(),
        socket: tmux.socket().to_owned(),
        conf: write_conf(&env.state_dir()).expect("write tmux.conf"),
        home: env.path().to_path_buf(),
        env: vec![
            ("PATH".into(), path),
            ("CST_RUNTIME_DIR".into(), env.runtime_dir().into()),
            ("CST_STATE_DIR".into(), env.state_dir().into()),
            ("CLAUDE_CONFIG_DIR".into(), env.claude_dir().into()),
        ],
    }
}

fn args_of(cmd: &std::process::Command) -> Vec<String> {
    std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn launcher_starts_the_landing_page_then_attaches() {
    let Some(tmux) = PrivateTmux::new() else {
        return;
    };
    let env = env();
    let server = server(&tmux, &env);
    server.start().expect("start the session");
    let out = tmux.run(&[
        "list-panes",
        "-a",
        "-F",
        "#{session_name} #{window_index} #{window_name} #{pane_start_command}",
    ]);
    let first = out.lines().next().unwrap_or_default();
    assert_eq!(first, format!("{SESSION} 1 sessions \"{LANDING}\""));
    let marked = tmux.run(&["show-options", "-wv", "-t", "claude:1", "@cs-landing"]);
    assert_eq!(marked.trim(), "1");
    let attach = args_of(&server.attach());
    let conf = server.conf.display().to_string();
    let want = [
        "tmux",
        "-L",
        tmux.socket(),
        "-f",
        &conf,
        "attach-session",
        "-t",
        "claude",
    ];
    assert_eq!(attach, want);
}

#[test]
fn a_second_start_keeps_the_session_and_marks_the_landing_page_again() {
    let Some(tmux) = PrivateTmux::new() else {
        return;
    };
    let env = env();
    let server = server(&tmux, &env);
    server.start().expect("first start");
    tmux.run(&["set-option", "-wu", "-t", "claude:1", "@cs-landing"]);
    server.start().expect("second start");
    let windows = tmux.run(&["list-windows", "-a", "-F", "#{window_index}"]);
    assert_eq!(windows.split_whitespace().collect::<Vec<_>>(), ["1"]);
    let marked = tmux.run(&["show-options", "-wv", "-t", "claude:1", "@cs-landing"]);
    assert_eq!(marked.trim(), "1");
}

#[test]
fn landing_runs_once_without_a_restart_loop() {
    assert_eq!(LANDING, "cst landing");
}

#[test]
fn ghostty_leaves_alt_left_right_and_unbinds_alt_up_down() {
    let cmd: Vec<String> = ghostty_command(Path::new("/x/cst"))
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(cmd[..2], ["ghostty", "--gtk-single-instance=false"]);
    for way in ["up", "down"] {
        assert!(cmd.contains(&format!("--keybind=alt+{way}=unbind")));
        assert!(cmd.contains(&format!("--keybind=alt+arrow_{way}=unbind")));
    }
    assert!(!cmd
        .iter()
        .any(|a| a.contains("alt+1") || a.contains("digit_")));
    assert!(!cmd
        .iter()
        .any(|a| a.contains("left") || a.contains("right")));
    assert_eq!(cmd[cmd.len() - 2..], ["-e", "/x/cst"]);
}

#[test]
fn missing_tmux_says_so() {
    let Some(tmux) = PrivateTmux::new() else {
        return;
    };
    let env = env();
    let server = Server {
        program: "/nonexistent/tmux".into(),
        ..server(&tmux, &env)
    };
    let err = server.start().expect_err("no tmux");
    assert!(matches!(err, LaunchError::NoTmux));
    assert_eq!(err.to_string(), "tmux is not installed");
}

#[test]
fn failed_new_session_says_so() {
    let Some(tmux) = PrivateTmux::new() else {
        return;
    };
    let env = env();
    // A tmux that fails every command: no session, and none can be made
    let fake = env.path().join("tmux");
    write_program(&fake, "#!/bin/sh\nexit 1\n");
    let server = Server {
        program: fake.into(),
        ..server(&tmux, &env)
    };
    let err = server.start().expect_err("no session");
    assert!(matches!(err, LaunchError::NoSession));
    assert_eq!(err.to_string(), "tmux could not create the claude session");
}

#[test]
fn conf_is_written_only_when_its_bytes_differ() {
    let env = env();
    let dir = env.path().join("fresh");
    let conf = write_conf(&dir).expect("first write");
    assert_eq!(conf, dir.join("tmux.conf"));
    assert_eq!(fs::read_to_string(&conf).expect("read"), conf_text());
    let inode = fs::metadata(&conf).expect("stat").ino();
    write_conf(&dir).expect("second write");
    assert_eq!(fs::metadata(&conf).expect("stat").ino(), inode);
    fs::write(&conf, "set -g status on\n").expect("edit");
    write_conf(&dir).expect("third write");
    assert_eq!(fs::read_to_string(&conf).expect("read"), conf_text());
}

#[test]
fn search_path_puts_the_binary_folder_first() {
    let dir = Path::new("/opt/cst/bin");
    let old = OsString::from("/usr/bin:/bin");
    assert_eq!(search_path(dir, Some(&old)), "/opt/cst/bin:/usr/bin:/bin");
    assert_eq!(search_path(dir, None), "/opt/cst/bin");
    assert_eq!(
        search_path(dir, Some(OsString::new().as_os_str())),
        "/opt/cst/bin"
    );
}

/// A session window of `tmux`: a Claude-like pane, and on its left a pane marked as the sidebar
/// that runs `sleep`. Returns the two pane ids.
fn session_window(tmux: &PrivateTmux) -> (String, String) {
    let claude = tmux.run(&[
        "new-window",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "-t",
        &format!("{SESSION}:"),
        "sleep 600",
    ]);
    let claude = claude.trim().to_owned();
    let sidebar = tmux.run(&[
        "split-window",
        "-hbd",
        "-l",
        "29",
        "-t",
        &claude,
        "-P",
        "-F",
        "#{pane_id}",
        "sleep 600",
    ]);
    let sidebar = sidebar.trim().to_owned();
    tmux.run(&["set-option", "-p", "-t", &sidebar, "@cs-sidebar", "1"]);
    (claude, sidebar)
}

fn pane_format(tmux: &PrivateTmux, pane: &str, format: &str) -> String {
    let out = tmux.run(&["display-message", "-p", "-t", pane, format]);
    out.trim().to_owned()
}

/// Starts `tmux` as an older version did, its first window running `first`; returns that
/// window's pane.
fn older_server(tmux: &PrivateTmux, env: &TestEnv, first: &str) -> String {
    // The conf of an older version: it runs `cs` and stamps nothing
    let old = env.path().join("old.conf");
    fs::write(&old, CONF.replace("cst ", "cs ")).expect("write the old conf");
    // `cs` is not on this PATH, so the old hooks run nothing
    let started = Command::new("tmux")
        .args(["-L", tmux.socket(), "-f"])
        .arg(&old)
        .args(["new-session", "-d", "-s", SESSION, "-n", "sessions", first])
        .env("PATH", "/usr/bin:/bin")
        .env_remove("TMUX")
        .status()
        .expect("start tmux");
    assert!(started.success());
    pane_format(tmux, &format!("{SESSION}:^"), "#{pane_id}")
}

#[test]
fn start_brings_an_older_server_up_to_date_and_keeps_its_sessions() {
    let Some(tmux) = PrivateTmux::new() else {
        return;
    };
    let env = env();
    // Stands for 1.x's `cs landing`, which is not installed here
    let landing = older_server(&tmux, &env, "sleep 600 || cs landing");
    let (claude, sidebar) = session_window(&tmux);
    let claude_pid = pane_format(&tmux, &claude, "#{pane_pid}");

    server(&tmux, &env).start().expect("start");

    assert_eq!(pane_format(&tmux, &claude, "#{pane_pid}"), claude_pid);
    let command = |pane: &str| pane_format(&tmux, pane, "#{pane_start_command}");
    assert_eq!(command(&sidebar), format!("\"{SIDEBAR_COMMAND}\""));
    assert_eq!(command(&landing), format!("\"{LANDING}\""));
    // It runs the built `cst`, which the server's PATH now finds first; the first run of a
    // fresh build can be slow
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let running = pane_format(&tmux, &sidebar, "#{pane_current_command}");
        if running == "cst" {
            break;
        }
        assert!(Instant::now() < deadline, "the sidebar runs {running}");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Hooks and keys run this `cst`
    let hooks = tmux.run(&["show-hooks", "-g"]);
    assert!(!hooks.contains("\"cs "), "{hooks}");
    let focus = tmux.run(&["show-options", "-gv", "@cs-focus"]);
    assert!(focus.starts_with("cst focus "), "{focus}");
    let keys = tmux.run(&["list-keys"]);
    assert!(
        keys.contains("cst step up") && !keys.contains("cs step"),
        "{keys}"
    );
    let stamp = tmux.run(&["show-options", "-sv", "@cs-conf"]);
    assert_eq!(stamp.trim(), conf_stamp());
}

#[test]
fn start_never_restarts_a_session_in_the_first_window() {
    let Some(tmux) = PrivateTmux::new() else {
        return;
    };
    let env = env();
    // The landing page is gone, so a session's window is first: the launcher marks it as the
    // landing page all the same
    let claude = older_server(&tmux, &env, "sleep 600");
    let claude_pid = pane_format(&tmux, &claude, "#{pane_pid}");

    server(&tmux, &env).start().expect("start");

    assert_eq!(pane_format(&tmux, &claude, "#{pane_pid}"), claude_pid);
}

#[test]
fn start_leaves_a_server_of_this_version_alone() {
    let Some(tmux) = PrivateTmux::new() else {
        return;
    };
    let env = env();
    let server = server(&tmux, &env);
    server.start().expect("first start");
    let landing = pane_format(&tmux, &format!("{SESSION}:^"), "#{pane_id}");
    let (_, sidebar) = session_window(&tmux);
    let pids = || [&landing, &sidebar].map(|pane| pane_format(&tmux, pane, "#{pane_pid}"));
    let before = pids();

    server.start().expect("second start");

    assert_eq!(pids(), before);
    let command = pane_format(&tmux, &sidebar, "#{pane_start_command}");
    assert_eq!(command, "\"sleep 600\"");
}
