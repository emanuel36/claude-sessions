//! `cst landing` running in a pane of a private tmux server.

mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use support::{write_history, write_program, TmuxApp};

/// How long the page gets to start and draw its first frame.
const START: Duration = Duration::from_secs(5);

/// The landing page in window `home`, over a history of two folders: `alpha-proj` used a
/// minute ago, then `beta-proj`. A `claude` that sleeps is first on the server's PATH.
struct Landing {
    app: TmuxApp,
    alpha: PathBuf,
    beta: PathBuf,
}

impl Landing {
    fn start() -> Option<Self> {
        let (tmux, env) = TmuxApp::server()?;
        let (alpha, beta) = (env.path().join("alpha-proj"), env.path().join("beta-proj"));
        let bin = env.path().join("bin");
        for dir in [&alpha, &beta, &bin] {
            fs::create_dir(dir).expect("create dir");
        }
        let claude = bin.join("claude");
        write_program(&claude, "#!/bin/sh\nexec sleep 600\n");
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        tmux.run(&["set-environment", "-g", "PATH", &path]);
        write_history(
            &env,
            &[
                ("s1", &alpha, 60.0, "fix the api"),
                ("s2", &beta, 120.0, "write the docs"),
            ],
        );
        let app = TmuxApp::run(tmux, env, "home", "landing");
        Some(Landing { app, alpha, beta })
    }

    /// The working folder of each pane running the fake `claude`.
    fn claude_folders(&self) -> Vec<String> {
        let listed = self.app.tmux.run(&[
            "list-panes",
            "-a",
            "-F",
            "#{pane_start_command}\t#{pane_current_path}",
        ]);
        listed
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .filter(|(command, _)| command.trim_matches('"') == "claude")
            .map(|(_, path)| path.to_owned())
            .collect()
    }

    /// Whether the private server still runs.
    fn server_alive(&self) -> bool {
        Command::new("tmux")
            .args(["-L", self.app.tmux.socket(), "has-session"])
            .output()
            .is_ok_and(|out| out.status.success())
    }
}

#[test]
fn landing_enter_opens_a_window_in_the_folder() {
    let Some(page) = Landing::start() else { return };
    page.app.assert_shows("alpha-proj", START);
    page.app.assert_shows("beta-proj", START);
    page.app.send("Down");
    page.app.send("Enter");
    let beta = page.beta.display().to_string();
    if !page.app.within(Duration::from_secs(3), || {
        page.claude_folders() == [beta.clone()]
    }) {
        page.app.fail(&format!(
            "no claude window in {beta} (alpha is {}): {:?}",
            page.alpha.display(),
            page.claude_folders()
        ));
    }
}

#[test]
fn landing_q_kills_the_private_server() {
    let Some(page) = Landing::start() else { return };
    page.app.assert_shows("alpha-proj", START);
    // `q` ends the server named by the page's $TMUX: make sure it is the private one
    let pid = page.app.pid();
    let environ = fs::read(format!("/proc/{pid}/environ")).expect("environ");
    let socket = page.app.tmux.socket_path().display().to_string();
    let tmux_var = environ
        .split(|&b| b == 0)
        .find_map(|var| var.strip_prefix(b"TMUX="))
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .unwrap_or_default();
    assert!(
        tmux_var.starts_with(&format!("{socket},")),
        "the page's TMUX is {tmux_var:?}, not the private server {socket}"
    );
    // Typed while searching, `q` is only a letter
    page.app.send("/");
    page.app.send("q");
    page.app.assert_shows("❯ q", Duration::from_secs(2));
    assert!(
        page.server_alive(),
        "q typed in the search ended the server"
    );
    page.app.send("Escape");
    page.app.assert_shows("/ to search", Duration::from_secs(2));
    page.app.send("q");
    if !page
        .app
        .within(Duration::from_secs(3), || !page.server_alive())
    {
        page.app.fail("the server still runs after q");
    }
}
