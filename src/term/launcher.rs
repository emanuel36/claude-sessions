//! `cst`: open the tmux server of Claude Code sessions, creating it if needed.
//!
//! `cst` attaches to the `claude` session in this terminal; `cst --ghostty` opens it in a new
//! Ghostty window that leaves Alt+Up/Down to tmux.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::core::paths;
use crate::term::tmux::SOCKET;

/// The tmux session that holds every Claude Code session's window.
pub const SESSION: &str = "claude";
/// What the first window runs. It stays the first window: closing the last session returns
/// to it.
pub const LANDING: &str = "cst landing";
/// The tmux configuration the server starts with.
pub const CONF: &str = include_str!("../../assets/tmux.conf");
/// The configuration's file name in the state directory.
const CONF_NAME: &str = "tmux.conf";

/// Why the `claude` session could not be opened.
#[derive(Debug)]
pub enum LaunchError {
    /// tmux could not be run.
    NoTmux,
    /// tmux ran but did not create the session.
    NoSession,
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LaunchError::NoTmux => "tmux is not installed",
            LaunchError::NoSession => "tmux could not create the claude session",
        })
    }
}

impl std::error::Error for LaunchError {}

/// A tmux server, and how to start the `claude` session on it.
#[derive(Clone, Debug)]
pub struct Server {
    /// The tmux program.
    pub program: OsString,
    /// The server's socket name, as given to `tmux -L`.
    pub socket: String,
    /// The configuration file the server starts with.
    pub conf: PathBuf,
    /// The folder the landing page starts in.
    pub home: PathBuf,
    /// Variables set on every tmux command; a server started by one keeps them for its windows
    /// and hooks.
    pub env: Vec<(OsString, OsString)>,
}

impl Server {
    /// A tmux command on this server, with its configuration and variables.
    fn tmux(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.arg("-L")
            .arg(&self.socket)
            .arg("-f")
            .arg(&self.conf)
            .envs(self.env.iter().map(|(k, v)| (k, v)));
        cmd
    }

    /// Creates the `claude` session, unless the server already has it, with the landing page as
    /// its first window.
    ///
    /// # Errors
    ///
    /// [`LaunchError::NoTmux`] when tmux cannot run, [`LaunchError::NoSession`] when it does not
    /// create the session.
    pub fn start(&self) -> Result<(), LaunchError> {
        let mut has = self.tmux();
        // It says why when there is no session, which is expected
        has.args(["has-session", "-t", SESSION])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if !status(has)?.success() {
            let mut new = self.tmux();
            // Window 1 is the landing page
            new.args(["new-session", "-d", "-s", SESSION, "-n", "sessions", "-c"])
                .arg(&self.home)
                .arg(LANDING);
            if !status(new)?.success() {
                return Err(LaunchError::NoSession);
            }
        }
        // Marked so Alt+Up/Down step over it. Set on every start, so a server from before the
        // mark gets it too
        let mut mark = self.tmux();
        mark.args([
            "set-option",
            "-w",
            "-t",
            &format!("{SESSION}:^"),
            "@cs-landing",
            "1",
        ]);
        status(mark)?;
        Ok(())
    }

    /// The command that attaches this terminal to the `claude` session.
    pub fn attach(&self) -> Command {
        let mut cmd = self.tmux();
        cmd.args(["attach-session", "-t", SESSION]);
        cmd
    }
}

/// Runs a tmux command to its end.
fn status(mut cmd: Command) -> Result<std::process::ExitStatus, LaunchError> {
    cmd.stdin(Stdio::null())
        .status()
        .map_err(|_| LaunchError::NoTmux)
}

/// Returns the Ghostty command line that runs `cst` with Alt+Up/Down unbound.
///
/// Ghostty moves between splits with Alt+Up/Down; a separate instance lets these flags apply.
/// Alt+Left/Right stay with Ghostty: tmux does not use them.
pub fn ghostty_command(cst: &Path) -> Vec<OsString> {
    let mut cmd: Vec<OsString> = vec!["ghostty".into(), "--gtk-single-instance=false".into()];
    for way in ["up", "down"] {
        cmd.push(format!("--keybind=alt+{way}=unbind").into());
        cmd.push(format!("--keybind=alt+arrow_{way}=unbind").into());
    }
    cmd.push("-e".into());
    cmd.push(cst.into());
    cmd
}

/// Returns `path` with `dir` in front, so tmux finds the `cst` beside this one first.
pub fn search_path(dir: &Path, path: Option<&OsStr>) -> OsString {
    let mut joined = dir.as_os_str().to_owned();
    if let Some(rest) = path.filter(|p| !p.is_empty()) {
        joined.push(":");
        joined.push(rest);
    }
    joined
}

/// Writes [`CONF`] to `dir/tmux.conf`, creating `dir`, unless the file already holds it.
///
/// # Errors
///
/// Returns the error of creating the folder or writing the file.
pub fn write_conf(dir: &Path) -> io::Result<PathBuf> {
    let path = dir.join(CONF_NAME);
    if std::fs::read(&path).is_ok_and(|bytes| bytes == CONF.as_bytes()) {
        return Ok(path);
    }
    std::fs::create_dir_all(dir)?;
    paths::write_atomic(&path, CONF.as_bytes(), None)?;
    Ok(path)
}

/// `cst` and `cst --ghostty`: opens the `claude` session, and returns the exit code when it
/// could not.
///
/// On success it does not return: the process becomes `tmux attach` (or Ghostty).
pub fn main(ghostty: bool) -> i32 {
    let exe = std::env::current_exe();
    let Some(here) = exe.as_deref().ok().and_then(Path::parent) else {
        return fail(&"cannot find the cst executable");
    };
    if ghostty {
        let cmd = ghostty_command(&here.join("cst"));
        let e = Command::new(&cmd[0]).args(&cmd[1..]).exec();
        return fail(&format_args!("cannot run ghostty: {e}"));
    }
    let conf = match write_conf(&paths::state_dir()) {
        Ok(conf) => conf,
        Err(e) => return fail(&format_args!("cannot write tmux.conf: {e}")),
    };
    let server = Server {
        program: "tmux".into(),
        socket: SOCKET.to_owned(),
        conf,
        home: std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
        // tmux runs the hooks' `cst`, which is this one
        env: vec![(
            "PATH".into(),
            search_path(here, std::env::var_os("PATH").as_deref()),
        )],
    };
    if let Err(e) = server.start() {
        return fail(&e);
    }
    let e = server.attach().exec();
    fail(&format_args!("cannot attach to tmux: {e}"))
}

fn fail(what: &dyn fmt::Display) -> i32 {
    eprintln!("cst: {what}");
    1
}
