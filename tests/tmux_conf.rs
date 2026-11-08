//! `assets/tmux.conf` on a private tmux server whose hooks run the built `cst`.

mod support;

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cst::paths::write_atomic;
use cst::state::{Mux, Record, Status};
use cst::store::process_start;
use cst::term::launcher::search_path;
use cst::term::tmux::SIDEBAR_WIDTH;
use support::{env, PrivateTmux, TestEnv};

const CONF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/tmux.conf");
/// What `cat -v` echoes for the keys tmux sends: F10 layout, F11 hidden, F12 shown.
const LAYOUT_SEQ: &str = "^[[21~";
const HIDDEN_SEQ: &str = "^[[23~";
const SHOWN_SEQ: &str = "^[[24~";
/// Generous: a dozen servers start at once, each running `cst` on every hook.
const WAIT: Duration = Duration::from_secs(10);
/// How often a wait checks again; each check runs tmux or reads `/proc`, for a dozen tests at
/// once.
const POLL: Duration = Duration::from_millis(50);

fn bin_dir() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_cst"))
        .parent()
        .expect("binary dir")
        .to_path_buf()
}

/// A private server started from the conf: session `t` of 120x20 with two windows, the second
/// one current and given its sidebar by the conf's hook.
struct Server {
    tmux: PrivateTmux,
    env: TestEnv,
}

impl Server {
    /// Starts the server with `first` at the front of its PATH, so tmux runs the `cst` in it.
    fn with_first_on_path(first: &Path) -> Option<Self> {
        let tmux = PrivateTmux::new()?;
        let env = env();
        let path = search_path(first, std::env::var_os("PATH").as_deref());
        let out = Command::new("tmux")
            .args(["-L", tmux.socket(), "-f", CONF])
            // Panes run `sleep` rather than the user's shell, which may take long to start
            .args([
                "new-session",
                "-d",
                "-s",
                "t",
                "-x",
                "120",
                "-y",
                "20",
                "sleep 600",
            ])
            .env("PATH", path)
            .env("CST_RUNTIME_DIR", env.runtime_dir())
            .env("CST_STATE_DIR", env.state_dir())
            .env("CLAUDE_CONFIG_DIR", env.claude_dir())
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .output()
            .expect("start tmux");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        tmux.run(&["new-window", "-t", "t:", "sleep 600"]);
        Some(Server { tmux, env })
    }

    /// Waits until no `cst layout` or `cst focus` runs for this server, for a while in a row:
    /// tmux runs them in the background or after the command that set them off returned.
    fn settle(&self) {
        let socket = format!("TMUX={},", self.tmux.socket_path().display());
        let deadline = Instant::now() + WAIT;
        let mut quiet_since = Instant::now();
        while Instant::now() < deadline {
            if hook_runs(&socket) > 0 {
                quiet_since = Instant::now();
            } else if quiet_since.elapsed() >= Duration::from_millis(300) {
                return;
            }
            thread::sleep(POLL);
        }
        panic!("the server's hooks are still running after {WAIT:?}");
    }

    /// Starts the server with the built `cst` first on its PATH.
    fn start() -> Option<Self> {
        Self::with_first_on_path(&bin_dir())
    }

    fn run(&self, args: &[&str]) -> String {
        self.tmux.run(args)
    }

    fn hooks(&self) -> String {
        format!(
            "{}{}",
            self.run(&["show-hooks", "-g"]),
            self.run(&["show-hooks", "-gw"])
        )
    }

    fn format(&self, target: &str, format: &str) -> String {
        self.run(&["display-message", "-p", "-t", target, format])
            .trim()
            .to_owned()
    }

    /// Opens a window running `command` and returns its id; the conf gives it a sidebar.
    fn new_window(&self, command: &str) -> String {
        let id = self.run(&[
            "new-window",
            "-P",
            "-F",
            "#{window_id}",
            "-t",
            "t:",
            command,
        ]);
        id.trim().to_owned()
    }

    /// The sidebar pane of a window, once the conf's hook has put it there.
    fn sidebar_of(&self, window: &str) -> String {
        let find = || {
            let out = self.run(&[
                "list-panes",
                "-t",
                window,
                "-F",
                "#{pane_id} #{@cs-sidebar}",
            ]);
            out.lines()
                .find_map(|l| l.strip_suffix(" 1").map(str::to_owned))
        };
        if !within(WAIT, || find().is_some()) {
            panic!("no sidebar in {window}:\n{}", self.panes());
        }
        find().unwrap_or_default()
    }

    /// Replaces what a pane runs with `cat -v`, which echoes the keys it gets from then on.
    fn echo_keys(&self, pane: &str) {
        // A key a hook sends late must not show up as if the test had set it off
        self.settle();
        self.run(&["respawn-pane", "-k", "-t", pane, "cat -v"]);
        let started = within(WAIT, || {
            self.format(pane, "#{pane_current_command}") == "cat"
        });
        assert!(started, "cat -v never started in {pane}");
        self.run(&["clear-history", "-t", pane]);
        self.run(&["send-keys", "-R", "-t", pane]);
    }

    fn screen(&self, pane: &str) -> String {
        self.run(&["capture-pane", "-p", "-t", pane])
    }

    /// Every pane, for failure messages.
    fn panes(&self) -> String {
        let format = "#{window_id} #{pane_id} sidebar=#{@cs-sidebar} #{pane_current_command} \
                      width=#{pane_width}";
        self.run(&["list-panes", "-a", "-F", format])
    }

    fn error_log(&self) -> String {
        fs::read_to_string(self.env.state_dir().join("errors.log")).unwrap_or_default()
    }
}

/// How many `cst layout` and `cst focus` processes run with `tmux_var` (`TMUX=<socket>,`) in
/// their environment.
fn hook_runs(tmux_var: &str) -> usize {
    let Ok(procs) = fs::read_dir("/proc") else {
        return 0;
    };
    procs
        .filter_map(Result::ok)
        .filter(|p| {
            let dir = p.path();
            let argv = fs::read(dir.join("cmdline")).unwrap_or_default();
            let mut args = argv.split(|&b| b == 0);
            let is_cst = args.next().is_some_and(|a| a.ends_with(b"cst"));
            let hook = args.next().is_some_and(|a| a == b"layout" || a == b"focus");
            is_cst && hook && {
                let environ = fs::read(dir.join("environ")).unwrap_or_default();
                environ
                    .split(|&b| b == 0)
                    .any(|v| v.starts_with(tmux_var.as_bytes()))
            }
        })
        .count()
}

fn within(limit: Duration, done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
}

#[test]
fn conf_loads_without_errors_on_tmux() {
    let Some(server) = Server::start() else {
        return;
    };
    let out = Command::new("tmux")
        .args(["-L", server.tmux.socket(), "source-file", CONF])
        .output()
        .expect("source the conf");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    let hooks = server.hooks();
    let focus = "run-shell \"cst focus --pane #{pane_id} --socket #{q:socket_path}\"";
    for hook in [
        "session-window-changed",
        "window-pane-changed",
        "client-attached",
        "client-session-changed",
    ] {
        assert!(
            hooks.contains(&format!("{hook}[0] {focus}")),
            "{hook}: {hooks}"
        );
    }
    // Only after-* hooks hold up the command that set them off, so only they run in the
    // background; the others stay in order with `cst focus`
    for hook in ["after-split-window", "after-kill-pane"] {
        let line = format!("{hook}[0] run-shell -b \"cst layout\"");
        assert!(hooks.contains(&line), "{hook}: {hooks}");
    }
    for hook in [
        "window-resized",
        "pane-exited",
        "window-renamed",
        "window-unlinked",
    ] {
        let line = format!("{hook}[0] run-shell \"cst layout\"");
        assert!(hooks.contains(&line), "{hook}: {hooks}");
    }
    let new_window = "after-new-window[0] run-shell -b \"cst layout #{window_id}\"";
    assert!(hooks.contains(new_window), "{hooks}");
    // The Python commands are all gone
    assert!(!hooks.contains("\"cs ") && !hooks.contains("@cs-seen") && !hooks.contains("@cs-wake"));
}

#[test]
fn windows_are_numbered_from_one() {
    let Some(server) = Server::start() else {
        return;
    };
    let out = server.run(&["list-windows", "-F", "#{window_index}"]);
    assert_eq!(out.split_whitespace().collect::<Vec<_>>(), ["1", "2"]);
}

#[test]
fn scheme_a_keys() {
    let Some(server) = Server::start() else {
        return;
    };
    let listed = server.run(&["list-keys", "-T", "root"]);
    let keys = listed.split_whitespace().collect::<Vec<_>>().join(" ");
    // Both follow the sidebar's order, which steps over the landing page
    assert!(
        keys.contains("M-Up run-shell -b \"cst step up #{window_id}\""),
        "{keys}"
    );
    assert!(keys.contains("M-Down run-shell -b \"cst step down #{window_id}\""));
    assert!(keys.contains("M-n display-popup") && keys.contains("cst new --pick"));
    assert!(keys.contains("M-c confirm-before") && keys.contains("close #{window_name}? (y/n)"));
    assert!(keys.contains("cst close #{window_id}"));
    for gone in [
        "M-Left", "M-Right", "M-0", "M-1 ", "M-9 ", "M-,", "M-.", "M-g", "cst next",
    ] {
        assert!(!keys.contains(gone), "{gone} is bound: {keys}");
    }
}

#[test]
fn status_bar_is_off() {
    let Some(server) = Server::start() else {
        return;
    };
    assert_eq!(server.run(&["show", "-gv", "status"]).trim(), "off");
}

#[test]
fn conf_hooks_and_mouse() {
    let Some(server) = Server::start() else {
        return;
    };
    let hooks = server.run(&["show-hooks", "-g"]);
    assert!(hooks.contains("after-new-window") && hooks.contains("cst layout"));
    assert!(hooks.contains("after-split-window"));
    // A window hook, so it is listed among the window options
    let resized = server.run(&["show-hooks", "-gw", "window-resized"]);
    assert!(resized.contains("cst layout"));
    let click = server.run(&["list-keys", "-T", "root", "MouseDown1Pane"]);
    assert!(click.contains("@cs-sidebar"));
}

#[test]
fn new_window_hook_passes_its_window_id_to_cst_layout() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let log = dir.path().join("log");
    let fake = dir.path().join("cst");
    fs::write(
        &fake,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display()),
    )
    .expect("write fake cst");
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod");
    let Some(server) = Server::with_first_on_path(dir.path()) else {
        return;
    };
    let window = server.new_window("sleep 600");
    let logged = || fs::read_to_string(&log).unwrap_or_default();
    let want = format!("layout {window}");
    assert!(
        within(WAIT, || logged().lines().any(|l| l == want)),
        "{want} not in {}",
        logged()
    );
}

/// A client attached in control mode, whose size the test sets as a terminal's would change.
struct Client(Child);

impl Client {
    fn attach(server: &Server) -> Self {
        let child = Command::new("tmux")
            .args(["-L", server.tmux.socket(), "-C", "attach", "-t", "t"])
            .env_remove("TMUX")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("attach a control client");
        let attached = within(WAIT, || server.format("t", "#{session_attached}") == "1");
        assert!(attached, "the client never attached");
        Client(child)
    }

    fn resize(&mut self, cols: u32, rows: u32) {
        let stdin = self.0.stdin.as_mut().expect("stdin");
        writeln!(stdin, "refresh-client -C {cols}x{rows}").expect("resize");
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn sidebars_keep_their_width_in_windows_resized_while_hidden() {
    // tmux resizes a window only once it becomes the current one, so a window that was hidden
    // during a terminal resize must be fitted when it is resized, not only on client-resized
    let Some(server) = Server::start() else {
        return;
    };
    let mut client = Client::attach(&server);
    client.resize(200, 50);
    for _ in 0..2 {
        server.new_window("sleep 600");
    }
    let widths = || -> Vec<u32> {
        let out = server.run(&["list-panes", "-a", "-F", "#{@cs-sidebar}:#{pane_width}"]);
        out.lines()
            .filter_map(|l| l.strip_prefix("1:"))
            .map(|w| w.parse().expect("width"))
            .collect()
    };
    let all_fit = || {
        let w = widths();
        w.len() == 3 && w.iter().all(|&w| w == SIDEBAR_WIDTH)
    };
    assert!(within(WAIT, all_fit), "{}", server.panes());
    for (cols, rows) in [(120, 40), (220, 50)] {
        client.resize(cols, rows);
        thread::sleep(Duration::from_millis(300));
        for window in 1..=4 {
            server.run(&["select-window", "-t", &format!("t:{window}")]);
            thread::sleep(Duration::from_millis(200));
        }
        assert!(
            within(WAIT, all_fit),
            "at {cols}x{rows}:\n{}",
            server.panes()
        );
    }
}

/// Opens a session window, `before` and `after` windows on either side of it, and ends the
/// session's pane with `end` once its sidebar is in; checks the window closes and the one after
/// it is shown.
fn closes_when_claude_ends(end: impl Fn(&Server, &str)) {
    let Some(server) = Server::start() else {
        return;
    };
    // `head` stands for Claude: it exits on the first line it reads
    let window = server.new_window("head -n 1");
    let after = server.new_window("sleep 600");
    server.sidebar_of(&window);
    server.sidebar_of(&after);
    server.run(&["select-window", "-t", &window]);
    let claude = server.format(&format!("{window}.2"), "#{pane_id}");
    end(&server, &claude);
    let gone = || {
        let out = server.run(&["list-windows", "-F", "#{window_id}"]);
        !out.lines().any(|w| w == window)
    };
    assert!(
        within(WAIT, gone),
        "{window} still open:\n{}\nerrors:\n{}",
        server.panes(),
        server.error_log()
    );
    // The window on screen went: the session next to it is shown, not the landing page
    assert_eq!(server.format("t", "#{window_id}"), after);
}

#[test]
fn closing_claude_closes_a_sidebar_only_window() {
    closes_when_claude_ends(|server, claude| {
        server.run(&["send-keys", "-t", claude, "Enter"]);
    });
}

#[test]
fn killing_claudes_pane_closes_its_window() {
    closes_when_claude_ends(|server, claude| {
        server.run(&["kill-pane", "-t", claude]);
    });
}

#[test]
fn focus_sends_hidden_to_the_previous_window() {
    let Some(server) = Server::start() else {
        return;
    };
    let a = server.new_window("sleep 600");
    let b = server.new_window("sleep 600");
    let (side_a, side_b) = (server.sidebar_of(&a), server.sidebar_of(&b));
    server.echo_keys(&side_a);
    server.echo_keys(&side_b);
    server.run(&["select-window", "-t", &a]);
    let told = || {
        server.screen(&side_b).contains(HIDDEN_SEQ) && server.screen(&side_a).contains(SHOWN_SEQ)
    };
    assert!(
        within(WAIT, told),
        "a: {:?}\nb: {:?}\nerrors:\n{}",
        server.screen(&side_a),
        server.screen(&side_b),
        server.error_log()
    );
    server.settle();
    assert!(!server.screen(&side_a).contains(HIDDEN_SEQ));
    assert!(!server.screen(&side_b).contains(SHOWN_SEQ));
    assert_eq!(
        server
            .run(&["show-options", "-sv", "@cs-last-window"])
            .trim(),
        a
    );
}

#[test]
fn a_split_sends_layout_to_the_visible_sidebar_only() {
    let Some(server) = Server::start() else {
        return;
    };
    let a = server.new_window("sleep 600");
    let b = server.new_window("sleep 600");
    let (side_a, side_b) = (server.sidebar_of(&a), server.sidebar_of(&b));
    server.echo_keys(&side_a);
    server.echo_keys(&side_b);
    server.run(&["split-window", "-d", "-t", &b, "sleep 600"]);
    assert!(
        within(WAIT, || server.screen(&side_b).contains(LAYOUT_SEQ)),
        "b: {:?}\nerrors:\n{}",
        server.screen(&side_b),
        server.error_log()
    );
    server.settle();
    assert!(!server.screen(&side_a).contains(LAYOUT_SEQ));
}

#[test]
fn looking_at_a_finished_session_settles_it() {
    let Some(server) = Server::start() else {
        return;
    };
    let a = server.new_window("sleep 600");
    server.new_window("sleep 600");
    // The sidebar is pane 1, so Claude's is 2
    server.sidebar_of(&a);
    let claude = server.format(&format!("{a}.2"), "#{pane_id}");
    // Only the switch below may settle the session
    server.settle();
    let pid = i32::try_from(std::process::id()).expect("pid");
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
        - 5.0;
    let record = Record {
        id: "s1".into(),
        status: Status::Done,
        cwd: "/w/api".into(),
        pid: Some(pid),
        pid_start: process_start(pid),
        status_since: at,
        updated_at: at,
        started_at: at,
        mux: Some(Mux {
            socket: server.tmux.socket_path().display().to_string(),
            pane: claude,
        }),
        ..Record::default()
    };
    let path = server.env.runtime_dir().join("s1.json");
    let json = serde_json::to_vec(&record).expect("record json");
    write_atomic(&path, &json, None).expect("write record");
    server.run(&["select-window", "-t", &a]);
    let idle = || {
        let bytes = fs::read(&path).unwrap_or_default();
        serde_json::from_slice::<Record>(&bytes).is_ok_and(|r| r.status == Status::Idle)
    };
    assert!(within(WAIT, idle), "errors:\n{}", server.error_log());
}
