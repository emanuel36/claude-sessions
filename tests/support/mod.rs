//! Helpers shared by the integration test crates.
#![allow(dead_code)]

pub mod golden;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use tempfile::TempDir;

/// A temp dir whose subdirectories back the three env vars `cst` reads.
pub struct TestEnv {
    dir: TempDir,
}

impl TestEnv {
    /// Root of the temp dir.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Value of `CST_RUNTIME_DIR`.
    pub fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("runtime")
    }

    /// Value of `CST_STATE_DIR`.
    pub fn state_dir(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    /// Value of `CLAUDE_CONFIG_DIR`.
    pub fn claude_dir(&self) -> PathBuf {
        self.dir.path().join("claude")
    }
}

/// Creates a fresh [`TestEnv`] with its three directories in place.
pub fn env() -> TestEnv {
    let dir = TempDir::new().expect("create temp dir");
    let env = TestEnv { dir };
    for sub in [env.runtime_dir(), env.state_dir(), env.claude_dir()] {
        std::fs::create_dir_all(sub).expect("create env dir");
    }
    env
}

/// Variables that say where Claude Code or tmux runs `cst`; the tests may run inside either, and
/// `cst` must never reach the real `tmux -L claude` from a test.
const AMBIENT: [&str; 6] = [
    "TMUX",
    "TMUX_PANE",
    "CLAUDE_PID",
    "CLAUDE_JOB_DIR",
    "CLAUDE_CODE_SESSION_KIND",
    "CLAUDE_CODE_ENTRYPOINT",
];

/// A `cst` command with `args`, the three env vars pointed at `env`, and no tmux or Claude Code
/// variables; stdio is piped.
pub fn command(env: &TestEnv, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cst"));
    cmd.args(args)
        .env("CST_RUNTIME_DIR", env.runtime_dir())
        .env("CST_STATE_DIR", env.state_dir())
        .env("CLAUDE_CONFIG_DIR", env.claude_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in AMBIENT {
        cmd.env_remove(name);
    }
    cmd
}

/// Starts `cmd` and feeds it `stdin`.
pub fn start(mut cmd: Command, stdin: &[u8]) -> Child {
    let mut child = cmd.spawn().expect("spawn cst");
    // The child may exit without reading stdin, so a broken pipe is not an error.
    let _ = child.stdin.take().expect("stdin is piped").write_all(stdin);
    child
}

/// Runs the `cst` binary with `args` and `stdin`, with the three env vars pointed at `env`.
pub fn cst(env: &TestEnv, args: &[&str], stdin: &[u8]) -> Output {
    let child = start(command(env, args), stdin);
    child.wait_with_output().expect("wait for cst")
}

/// A tmux server on its own socket (`cst-test-<uuid>`), killed with its socket file on drop.
pub struct PrivateTmux {
    socket: String,
}

impl PrivateTmux {
    /// Returns `None` when tmux is not installed.
    pub fn new() -> Option<Self> {
        Command::new("tmux").arg("-V").output().ok()?;
        Some(Self {
            socket: format!("cst-test-{}", uuid::Uuid::new_v4()),
        })
    }

    /// The socket name, as passed to `tmux -L`.
    pub fn socket(&self) -> &str {
        &self.socket
    }

    /// Path of the socket file: `<TMUX_TMPDIR or /tmp>/tmux-<uid>/<socket>`.
    pub fn socket_path(&self) -> PathBuf {
        let uid = rustix::process::getuid().as_raw();
        let tmpdir = std::env::var("TMUX_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(tmpdir)
            .join(format!("tmux-{uid}"))
            .join(&self.socket)
    }

    /// Runs `tmux -L <socket> <args>` and returns its stdout.
    pub fn run(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .args(["-L", &self.socket])
            .args(args)
            .output()
            .expect("run tmux");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for PrivateTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .output();
        let _ = std::fs::remove_file(self.socket_path());
    }
}
