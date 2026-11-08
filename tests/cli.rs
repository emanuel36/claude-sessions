//! Black-box tests of the `cst` command line.

mod support;

use support::{PrivateTmux, cst, env};

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
