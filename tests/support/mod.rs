//! Helpers shared by the integration test crates.
#![allow(dead_code)]

pub mod golden;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    command_of(Path::new(env!("CARGO_BIN_EXE_cst")), env, args)
}

/// Like [`command`], for the `cst` executable at `program`.
pub fn command_of(program: &Path, env: &TestEnv, args: &[&str]) -> Command {
    let mut cmd = Command::new(program);
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
    ///
    /// # Panics
    ///
    /// When tmux fails, with its exit status and stderr: a failed call must not pass for an
    /// empty answer. [`PrivateTmux::try_run`] is for calls that may fail.
    pub fn run(&self, args: &[&str]) -> String {
        let out = self.try_run(args);
        assert!(
            out.status.success(),
            "tmux {args:?} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Runs `tmux -L <socket> <args>` and returns all it did, failed or not.
    pub fn try_run(&self, args: &[&str]) -> Output {
        Command::new("tmux")
            .args(["-L", &self.socket])
            .args(args)
            .output()
            .expect("run tmux")
    }
}

/// The shell command a pane runs for `cst <args>`: the three env vars pointed at `env`, the test
/// binary first on the PATH, then the folders of `path`. `$TMUX` and `$TMUX_PANE` are the
/// pane's own.
///
/// The windows `cst` opens take this PATH: tmux gives a pane the environment of the client that
/// asked for it, so a `set-environment -g PATH` on the server would not reach them.
pub fn pane_command(env: &TestEnv, args: &str, path: &[&Path]) -> String {
    let bin = Path::new(env!("CARGO_BIN_EXE_cst"))
        .parent()
        .expect("binary dir");
    let dirs: String = std::iter::once(bin)
        .chain(path.iter().copied())
        .map(|dir| format!("'{}':", dir.display()))
        .collect();
    format!(
        "exec env CST_RUNTIME_DIR='{}' CST_STATE_DIR='{}' CLAUDE_CONFIG_DIR='{}' \
         PATH={dirs}\"$PATH\" cst {args}",
        env.runtime_dir().display(),
        env.state_dir().display(),
        env.claude_dir().display(),
    )
}

/// `cst` running in a window of a private tmux server, session `t`.
///
/// Panes stay after their command exits (`remain-on-exit`), so a `cst` that quits early leaves
/// its last words and exit status for the failure message.
pub struct TmuxApp {
    pub tmux: PrivateTmux,
    pub env: TestEnv,
    /// The pane `cst` runs in.
    pub pane: String,
}

impl TmuxApp {
    /// A private server with session `t`, its first window `alpha` running `sleep`; `None`
    /// when tmux is not installed.
    pub fn server() -> Option<(PrivateTmux, TestEnv)> {
        let tmux = PrivateTmux::new()?;
        let env = env();
        tmux.run(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "t",
            "-x",
            "100",
            "-y",
            "50",
            "-n",
            "alpha",
            "sleep 600",
            ";",
            "set-option",
            "-g",
            "remain-on-exit",
            "on",
        ]);
        Some((tmux, env))
    }

    /// Runs `cst <args>` in a new current window named `window`.
    pub fn run(tmux: PrivateTmux, env: TestEnv, window: &str, args: &str) -> Self {
        Self::run_with_path(tmux, env, window, args, &[])
    }

    /// Like [`TmuxApp::run`], with the folders of `path` next on `cst`'s PATH (see
    /// [`pane_command`]).
    pub fn run_with_path(
        tmux: PrivateTmux,
        env: TestEnv,
        window: &str,
        args: &str,
        path: &[&Path],
    ) -> Self {
        let command = pane_command(&env, args, path);
        let pane = tmux.run(&[
            "new-window",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            "t:",
            "-n",
            window,
            &command,
        ]);
        TmuxApp {
            tmux,
            env,
            pane: pane.trim().to_owned(),
        }
    }

    /// What the pane shows.
    pub fn screen(&self) -> String {
        self.tmux.run(&["capture-pane", "-p", "-t", &self.pane])
    }

    /// Waits until the screen shows `text`, for at most `limit`.
    pub fn shows_within(&self, text: &str, limit: Duration) -> bool {
        self.within(limit, || self.screen().contains(text))
    }

    /// Waits until `done` holds, for at most `limit`; returns whether it did.
    pub fn within(&self, limit: Duration, done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if done() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Fails with the panes, the error log and the screen unless `text` shows within `limit`.
    pub fn assert_shows(&self, text: &str, limit: Duration) {
        if !self.shows_within(text, limit) {
            self.fail(&format!("{text:?} not shown within {limit:?}"));
        }
    }

    /// Fails with `what`, the panes, the error log and the screen.
    pub fn fail(&self, what: &str) -> ! {
        // Read with try_run: a failing tmux must not hide `what`
        let text = |args: &[&str]| {
            let out = self.tmux.try_run(args);
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        };
        let panes = text(&[
            "list-panes",
            "-a",
            "-F",
            "#{pane_id} #{pane_pid} dead=#{pane_dead} status=#{pane_dead_status}",
        ]);
        let log = fs::read_to_string(self.env.state_dir().join("errors.log")).unwrap_or_default();
        let screen = text(&["capture-pane", "-p", "-t", &self.pane]);
        panic!(
            "{what} in {}\npanes:\n{panes}\nerrors:\n{log}\nscreen:\n{screen}",
            self.pane
        );
    }

    /// Sends tmux key names (`Enter`, `F11`) to the pane.
    pub fn send(&self, key: &str) {
        self.tmux.run(&["send-keys", "-t", &self.pane, key]);
    }

    /// Types `text` into the pane as it is.
    pub fn type_text(&self, text: &str) {
        self.tmux.run(&["send-keys", "-l", "-t", &self.pane, text]);
    }

    /// A tmux format of the pane, such as `#{pane_dead}`.
    pub fn format(&self, format: &str) -> String {
        let out = self
            .tmux
            .run(&["display-message", "-p", "-t", &self.pane, format]);
        out.trim().to_owned()
    }

    /// The process the pane runs, checked to be `cst` itself.
    pub fn pid(&self) -> String {
        let pid = self.format("#{pane_pid}");
        let comm = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        assert_eq!(comm.trim(), "cst", "the pane does not run cst directly");
        pid
    }
}

/// Writes Claude Code's prompt history in `env`: one prompt per `(session, folder, seconds ago,
/// prompt)`.
pub fn write_history(env: &TestEnv, prompts: &[(&str, &Path, f64, &str)]) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    let mut lines = String::new();
    for (id, folder, ago, prompt) in prompts {
        let line = serde_json::json!({
            "display": prompt,
            "timestamp": ((now - ago) * 1000.0) as u64,
            "project": folder.display().to_string(),
            "sessionId": id,
        });
        lines.push_str(&line.to_string());
        lines.push('\n');
    }
    fs::write(env.claude_dir().join("history.jsonl"), lines).expect("write history");
}

impl Drop for PrivateTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .output();
        let _ = std::fs::remove_file(self.socket_path());
    }
}

/// The release asset for this machine: `cst-<arch>-unknown-linux-musl`, with the arch named as
/// `install.sh` names it from `uname -m`.
pub fn asset_name() -> String {
    let out = Command::new("uname").arg("-m").output().expect("run uname");
    let arch = match String::from_utf8_lossy(&out.stdout).trim() {
        "aarch64" | "arm64" => "aarch64".to_owned(),
        other => other.to_owned(),
    };
    format!("cst-{arch}-unknown-linux-musl")
}

/// The test binary with `marker` appended: it still runs, but differs from it byte for byte.
pub fn release_binary(marker: &str) -> Vec<u8> {
    let mut bytes = fs::read(env!("CARGO_BIN_EXE_cst")).expect("read the test binary");
    bytes.extend_from_slice(marker.as_bytes());
    bytes
}

/// The SHA-256 of `bytes`, in lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Publishes a release in `dir` laid out as `CST_RELEASE_BASE=file://<dir>` reads it:
/// `latest/tag`, and `download/<tag>/` holding the asset and its `.sha256` in `sha256sum`
/// format. `checksum` replaces the asset's true SHA-256.
pub fn publish(dir: &Path, tag: &str, asset: &[u8], checksum: Option<&str>) {
    let name = asset_name();
    let download = dir.join("download").join(tag);
    fs::create_dir_all(&download).expect("create the release dir");
    fs::create_dir_all(dir.join("latest")).expect("create the latest dir");
    fs::write(dir.join("latest/tag"), format!("{tag}\n")).expect("write the tag");
    fs::write(download.join(&name), asset).expect("write the asset");
    let sum = checksum.map_or_else(|| sha256_hex(asset), str::to_owned);
    fs::write(
        download.join(format!("{name}.sha256")),
        format!("{sum}  {name}\n"),
    )
    .expect("write the checksum");
}

/// Writes an executable `script` at `path`.
///
/// A file this process writes can fail to run with ETXTBSY: another test thread may fork while
/// it is open for writing, and the child holds it until it execs. So `install`, a process of its
/// own, writes the program; this process only writes the source next to it.
pub fn write_program(path: &Path, script: &str) {
    let source = path.with_extension("source");
    fs::write(&source, script).expect("write the program's source");
    let status = Command::new("install")
        .args(["-m", "755"])
        .arg(&source)
        .arg(path)
        .status()
        .expect("run install");
    assert!(status.success(), "install {}", path.display());
    fs::remove_file(&source).expect("remove the program's source");
}

/// Runs `cmd`, whose program was just written, retrying while a fork in another test thread
/// still holds it open for writing (ETXTBSY).
pub fn output_retrying(cmd: &mut Command) -> Output {
    const TEXT_FILE_BUSY: i32 = 26;
    for _ in 0..50 {
        match cmd.output() {
            Err(e) if e.raw_os_error() == Some(TEXT_FILE_BUSY) => {
                thread::sleep(Duration::from_millis(20));
            }
            result => return result.expect("run the command"),
        }
    }
    panic!("the program stayed busy");
}
