//! The tmux queries and actions against a real, private tmux server.

mod support;

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cst::state::{new_record, Mux, Record, Status};
use cst::term::tmux::{
    add_sidebar_running, close_window, fit_sidebars, name_window, pane_in_view, panes,
    settle_if_viewed, Cli, Pane, Runner, SIDEBAR_WIDTH,
};
use support::PrivateTmux;

/// A private server with one detached session `t` of 200x50 and no configuration, and a runner
/// for it. Returns `None` when tmux is not installed.
fn server() -> Option<(PrivateTmux, Cli)> {
    let tmux = PrivateTmux::new()?;
    tmux.run(&[
        "-f",
        "/dev/null",
        "new-session",
        "-d",
        "-s",
        "t",
        "-x",
        "200",
        "-y",
        "50",
    ]);
    let cli = Cli::at(tmux.socket_path().to_str()?);
    Some((tmux, cli))
}

fn lines(tmux: &PrivateTmux, args: &[&str]) -> Vec<String> {
    tmux.run(args).lines().map(str::to_owned).collect()
}

fn first_window(tmux: &PrivateTmux) -> String {
    tmux.run(&["display-message", "-p", "-t", "t", "#{window_id}"])
        .trim()
        .to_owned()
}

/// Where a session in the active pane of the window `target` runs.
fn mux_of(tmux: &PrivateTmux, cli: &Cli, target: &str) -> Mux {
    let pane = tmux.run(&["display-message", "-p", "-t", target, "#{pane_id}"]);
    Mux {
        socket: cli.socket.clone().unwrap_or_default(),
        pane: pane.trim().to_owned(),
    }
}

fn window_name_of(tmux: &PrivateTmux, target: &str) -> String {
    tmux.run(&["display-message", "-p", "-t", target, "#{window_name}"])
        .trim()
        .to_owned()
}

/// Attaches a client to the session through a pseudo-terminal made by `script`.
fn attach(tmux: &PrivateTmux) -> Child {
    let cmd = format!("tmux -L {} attach -t t", tmux.socket());
    let child = Command::new("script")
        .args(["-qec", &cmd, "/dev/null"])
        .env("TERM", "xterm-256color")
        .env_remove("TMUX")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn script");
    let deadline = Instant::now() + Duration::from_secs(5);
    while tmux
        .run(&["display-message", "-p", "-t", "t", "#{session_attached}"])
        .trim()
        != "1"
    {
        assert!(Instant::now() < deadline, "the client never attached");
        std::thread::sleep(Duration::from_millis(50));
    }
    child
}

#[test]
fn add_sidebar_on_a_private_server() {
    let Some((tmux, cli)) = server() else { return };
    let window = first_window(&tmux);
    add_sidebar_running(&window, "sleep 600", &cli);
    add_sidebar_running(&window, "sleep 600", &cli);
    let fmt = "#{pane_left}\t#{pane_width}\t#{@cs-sidebar}\t#{pane_active}";
    let rows = lines(&tmux, &["list-panes", "-t", &window, "-F", fmt]);
    assert_eq!(
        rows.len(),
        2,
        "the second call must not add a pane: {rows:?}"
    );
    assert_eq!(rows[0], format!("0\t{SIDEBAR_WIDTH}\t1\t0"));
    assert!(
        rows[1].ends_with("\t\t1"),
        "the other pane stays active and unmarked: {rows:?}"
    );
}

#[test]
fn runner_returns_empty_when_the_server_is_gone() {
    let gone = Cli::at("/nonexistent/tmux-sock");
    assert_eq!(gone.run(&["list-windows"]), "");
    let Some((tmux, cli)) = server() else { return };
    assert_eq!(
        cli.run(&["display-message", "-p", "-t", "t", "#{session_name}"]),
        "t"
    );
    tmux.run(&["kill-server"]);
    assert_eq!(cli.run(&["list-windows"]), "");
    assert!(panes(&cli).is_empty());
}

#[test]
fn panes_read_the_sidebar_and_landing_flags() {
    let Some((tmux, cli)) = server() else { return };
    let window = first_window(&tmux);
    tmux.run(&["set-option", "-p", "-t", &window, "@cs-landing", "1"]);
    add_sidebar_running(&window, "sleep 600", &cli);
    let listed = panes(&cli);
    assert_eq!(listed.len(), 2);
    let sidebar: &Pane = listed.iter().find(|p| p.sidebar).expect("a sidebar pane");
    let landing: &Pane = listed.iter().find(|p| p.landing).expect("a landing pane");
    assert!(!sidebar.landing && !landing.sidebar);
    assert_eq!(sidebar.window_id, window);
    assert_eq!(sidebar.index, landing.index - 1);
}

#[test]
fn fit_sidebars_restores_the_width_after_a_split() {
    let Some((tmux, cli)) = server() else { return };
    let window = first_window(&tmux);
    add_sidebar_running(&window, "sleep 600", &cli);
    tmux.run(&["split-window", "-h", "-t", &window]);
    tmux.run(&["resize-pane", "-t", &format!("{window}.0"), "-x", "50"]);
    let width = || {
        let fmt = "#{?@cs-sidebar,#{pane_width},}";
        let rows = lines(&tmux, &["list-panes", "-t", &window, "-F", fmt]);
        rows.into_iter()
            .find(|w| !w.is_empty())
            .expect("a sidebar width")
    };
    assert_eq!(width(), "50");
    fit_sidebars(&cli);
    assert_eq!(width(), SIDEBAR_WIDTH.to_string());
}

#[test]
fn name_window_renames_its_own_names_and_leaves_the_users() {
    let Some((tmux, cli)) = server() else { return };
    let window = first_window(&tmux);
    let record = |cwd: &str| Record {
        cwd: cwd.into(),
        mux: Some(mux_of(&tmux, &cli, &window)),
        ..new_record("s")
    };
    name_window(&record("/w/api"), "", &cli);
    assert_eq!(window_name_of(&tmux, &window), "api");
    // Renaming turned automatic-rename off; the name it set is still its own to change
    name_window(&record("/w/notes"), "", &cli);
    assert_eq!(window_name_of(&tmux, &window), "notes");
    tmux.run(&["rename-window", "-t", &window, "mine"]);
    name_window(&record("/w/api"), "", &cli);
    assert_eq!(window_name_of(&tmux, &window), "mine");
}

#[test]
fn name_window_with_a_dead_server_does_not_panic() {
    let gone = Cli::at("/nonexistent/tmux-sock");
    let record = Record {
        cwd: "/w/api".into(),
        mux: Some(Mux {
            socket: "/nonexistent/tmux-sock".into(),
            pane: "%1".into(),
        }),
        ..new_record("s")
    };
    name_window(&record, "", &gone);
}

#[test]
fn a_pane_is_in_view_only_when_it_is_active_in_an_attached_session() {
    let Some((tmux, cli)) = server() else { return };
    let window = first_window(&tmux);
    let shown = mux_of(&tmux, &cli, &window);
    assert!(!pane_in_view(&shown, &cli), "no client is attached yet");
    let mut client = attach(&tmux);
    assert!(pane_in_view(&shown, &cli));
    tmux.run(&["split-window", "-t", &window]);
    let hidden = tmux.run(&[
        "display-message",
        "-p",
        "-t",
        &format!("{window}.0"),
        "#{pane_id}",
    ]);
    assert_eq!(
        hidden.trim(),
        shown.pane,
        "the first pane is the one that was split"
    );
    assert!(!pane_in_view(&shown, &cli), "the new pane took focus");
    let _ = client.kill();
    let _ = client.wait();
}

#[test]
fn settle_if_viewed_marks_a_finished_session_in_view_seen() {
    let Some((tmux, cli)) = server() else { return };
    let window = first_window(&tmux);
    let done = Record {
        status: Status::Done,
        status_since: 1.0,
        mux: Some(mux_of(&tmux, &cli, &window)),
        ..new_record("s")
    };
    assert_eq!(
        settle_if_viewed(done.clone(), 5.0, &cli),
        done,
        "nobody is looking"
    );
    let mut client = attach(&tmux);
    let settled = settle_if_viewed(done, 5.0, &cli);
    assert_eq!((settled.status, settled.status_since), (Status::Idle, 5.0));
    let _ = client.kill();
    let _ = client.wait();
}

#[test]
fn close_window_selects_the_next_session_and_kills_the_window() {
    let Some((tmux, cli)) = server() else { return };
    // Window 1 is the landing page (no sidebar); 2 and 3 are sessions
    let landing = first_window(&tmux);
    let mut sessions = Vec::new();
    for name in ["api", "web"] {
        let id = tmux.run(&["new-window", "-d", "-P", "-F", "#{window_id}", "-n", name]);
        let id = id.trim().to_owned();
        add_sidebar_running(&id, "sleep 600", &cli);
        sessions.push(id);
    }
    tmux.run(&["select-window", "-t", &sessions[0]]);
    close_window(&sessions[0], &cli);
    let alive = lines(&tmux, &["list-windows", "-F", "#{window_id}"]);
    assert_eq!(alive, [landing, sessions[1].clone()]);
    let active = tmux.run(&["display-message", "-p", "#{window_id}"]);
    assert_eq!(
        active.trim(),
        sessions[1],
        "the next session, not the landing page"
    );
}
