//! Where a session runs in tmux, and how to tell tmux about its changes.
//!
//! Every function that talks to tmux takes a [`Runner`], so tests can replay outputs. A runner
//! talks to one server: [`Cli`] with a socket path runs `tmux -S <path>`, without one it runs
//! plain `tmux`, which uses the server this process runs in (through `$TMUX`).

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::fd::OwnedFd;
use rustix::process::{pidfd_open, Pid, PidfdFlags};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::state::{self, Mux, Record, Status};

/// A hook makes up to 7 tmux calls; together they must stay under its 10 s timeout.
pub const TIMEOUT: Duration = Duration::from_secs(1);
/// `claude stop` gets longer: it talks to Claude's daemon.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// Long enough for a session's title; the sidebar fits it to its own width.
pub const WINDOW_NAME_LEN: usize = 60;
/// The tmux socket (`-L`) cst's own server listens on.
pub const SOCKET: &str = "claude";
/// Columns of the sidebar pane; tmux's border makes it 30.
pub const SIDEBAR_WIDTH: u32 = 29;
/// What runs in a new sidebar pane.
pub const SIDEBAR_COMMAND: &str = "cst sidebar";

/// What Claude Code calls a session that runs in its daemon (moved to the background).
const BACKGROUND: &str = "bg";
/// How Claude Code names a run without a terminal (`claude -p`, the Agent SDK).
const HEADLESS: [&str; 3] = ["sdk-cli", "sdk-ts", "sdk-py"];
/// `claude attach` takes a session's id or its first 8 characters.
const MIN_ID_PREFIX: usize = 8;
/// Every pane of the server and its window, in one listing; the window's name comes last, as
/// it may hold a tab.
const PANES_FORMAT: &str = "#{pane_id}\t#{window_id}\t#{pane_index}\t#{window_index}\t\
                            #{@cs-sidebar}\t#{@cs-landing}\t#{window_name}";
const PANE_IN_VIEW_FORMAT: &str = "#{&&:#{&&:#{pane_active},#{window_active}},#{session_attached}}";
const NAMING_FORMAT: &str = "#{automatic-rename}\t#{@cs-auto-name}\t#{window_name}";
/// How often a running command is checked for having exited.
const POLL: Duration = Duration::from_millis(2);

/// Something that runs tmux commands on one server.
pub trait Runner {
    /// Runs one tmux command and returns its output minus one trailing newline.
    ///
    /// Returns `""` on any failure (no tmux, no server, a timeout); never panics.
    fn run(&self, args: &[&str]) -> String;
}

/// Runs the `tmux` binary, killing it after [`TIMEOUT`].
#[derive(Clone, Debug, Default)]
pub struct Cli {
    /// The server's socket path; `None` means the server this process runs in.
    pub socket: Option<String>,
}

impl Cli {
    /// A runner for the server at `socket`, as found in a record's [`Mux`].
    pub fn at(socket: &str) -> Self {
        Cli {
            socket: Some(socket.to_owned()),
        }
    }
}

impl Runner for Cli {
    fn run(&self, args: &[&str]) -> String {
        let mut cmd = Command::new("tmux");
        if let Some(socket) = &self.socket {
            cmd.args(["-S", socket]);
        }
        cmd.args(args);
        run_capped(cmd, TIMEOUT)
    }
}

/// Runs a command for at most `timeout` and returns its stdout minus one trailing newline.
///
/// Returns `""` when the command cannot start or does not finish in time; it is killed then.
fn run_capped(cmd: Command, timeout: Duration) -> String {
    run_capped_with(cmd, timeout, &spawn_thread)
}

/// Starts the job that reads a child's output on a thread of its own.
type Spawner<'a> = &'a dyn Fn(Box<dyn FnOnce() + Send>) -> std::io::Result<()>;

fn spawn_thread(job: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
    thread::Builder::new().spawn(job).map(drop)
}

fn run_capped_with(mut cmd: Command, timeout: Duration, spawn: Spawner<'_>) -> String {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return String::new();
    };
    let deadline = Instant::now() + timeout;
    let Some(mut stdout) = child.stdout.take() else {
        give_up(&mut child);
        return String::new();
    };
    // A reader thread keeps a full pipe from stalling the child; it is left to end by itself
    // when the child is killed.
    let (tx, rx) = mpsc::channel();
    let reader = Box::new(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    // `thread::spawn` panics when the system is out of threads; a hook must not
    if spawn(reader).is_err() {
        give_up(&mut child);
        return String::new();
    }
    let out = rx.recv_timeout(timeout);
    let exited = out.is_ok() && wait_until(&mut child, deadline);
    match out {
        Ok(buf) if exited => {
            let text = String::from_utf8_lossy(&buf);
            text.strip_suffix('\n').unwrap_or(&text).to_owned()
        }
        _ => {
            give_up(&mut child);
            String::new()
        }
    }
}

/// Kills a child and waits for it, so it leaves no zombie behind.
fn give_up(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Waits for the child to exit until `deadline`; false if it is still running.
fn wait_until(child: &mut std::process::Child, deadline: Instant) -> bool {
    wait_until_with(child, deadline, &mut || thread::sleep(POLL))
}

fn wait_until_with(
    child: &mut std::process::Child,
    deadline: Instant,
    sleep: &mut dyn FnMut(),
) -> bool {
    // A pidfd becomes readable when the child exits, so the wait needs no polling. The child is
    // not reaped yet, so its pid cannot belong to anyone else.
    let pidfd = child_pidfd(child);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Err(_) => return false,
            Ok(None) => {}
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        match &pidfd {
            Some(fd) if block_on(fd, left) => {}
            _ => sleep(),
        }
    }
}

fn child_pidfd(child: &std::process::Child) -> Option<OwnedFd> {
    let pid = Pid::from_raw(i32::try_from(child.id()).ok()?)?;
    pidfd_open(pid, PidfdFlags::empty()).ok()
}

/// Blocks until the process behind `fd` exits or `left` has passed; false if poll failed.
fn block_on(fd: &OwnedFd, left: Duration) -> bool {
    let timeout = Timespec {
        tv_sec: i64::try_from(left.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(left.subsec_nanos()),
    };
    let mut fds = [PollFd::new(fd, PollFlags::IN)];
    poll(&mut fds, Some(&timeout)).is_ok()
}

/// Reads other processes' command lines and children; a seam for tests.
trait Procs {
    /// A process's command line; empty when it is gone.
    fn cmdline(&self, pid: &str) -> Vec<String>;
    /// The ids of a process's direct children.
    fn children(&self, pid: &str) -> Vec<String>;
}

/// Reads `/proc`.
struct Proc;

impl Procs for Proc {
    fn cmdline(&self, pid: &str) -> Vec<String> {
        let Ok(bytes) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            return Vec::new();
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut argv: Vec<String> = text.split('\0').map(str::to_owned).collect();
        // The command line ends with a NUL, so the split leaves an empty piece
        if argv.last().is_some_and(String::is_empty) {
            argv.pop();
        }
        argv
    }

    fn children(&self, pid: &str) -> Vec<String> {
        std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default()
    }
}

/// A pane of a tmux server, and the window it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    /// The pane's id, such as `%3`.
    pub pane_id: String,
    /// The window's id, such as `@2`.
    pub window_id: String,
    /// The pane's index in its window.
    pub index: u32,
    /// The window's index.
    pub window_index: u32,
    /// Whether the pane is a sidebar (`@cs-sidebar` is set).
    pub sidebar: bool,
    /// Whether the pane is the landing page (`@cs-landing` is set).
    pub landing: bool,
    /// The window's name.
    pub window_name: String,
}

/// A tmux window running Claude; teammates share it.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// The window's id.
    pub window_id: String,
    /// Its most urgent session, which sets the status shown.
    pub lead: Record,
    /// The session in the window's first Claude pane, whose model, branch and cost the window
    /// shows.
    pub leader: Option<Record>,
}

impl Window {
    /// The session whose details the window shows: the leader, else the lead.
    pub fn head(&self) -> &Record {
        self.leader.as_ref().unwrap_or(&self.lead)
    }
}

/// Returns the tmux pane a session runs in, or `None` outside tmux.
///
/// A background session runs in Claude Code's daemon, outside tmux, so its pane is the one
/// that shows it with `claude attach`; that is looked up on cst's own server (`-L claude`)
/// through `tmux`, which must therefore not be bound to another socket.
///
/// Args:
///     env: The process environment.
///     session_id: The session's id, to find the pane of a background session.
///     tmux: Runs the lookup of a background session's pane.
pub fn detect(env: &HashMap<String, String>, session_id: &str, tmux: &dyn Runner) -> Option<Mux> {
    detect_with(env, session_id, tmux, &Proc)
}

fn detect_with(
    env: &HashMap<String, String>,
    session_id: &str,
    tmux: &dyn Runner,
    procs: &dyn Procs,
) -> Option<Mux> {
    let set = |name: &str| env.get(name).filter(|v| !v.is_empty());
    // A session moved to the background may still carry the tmux pane it left. Hooks get no
    // session kind, only the job folder the daemon gives a background session.
    if set("CLAUDE_JOB_DIR").is_some()
        || set("CLAUDE_CODE_SESSION_KIND").is_some_and(|k| k == BACKGROUND)
    {
        return if session_id.is_empty() {
            None
        } else {
            attached_pane(session_id, tmux, procs)
        };
    }
    // A `claude -p` started from a session (a start hook, a script) inherits that session's
    // TMUX, but runs in no pane.
    if is_headless(env) {
        return None;
    }
    let socket = set("TMUX")?.split(',').next()?;
    let pane = set("TMUX_PANE")?;
    Some(Mux {
        socket: socket.to_owned(),
        pane: pane.to_owned(),
    })
}

/// Tells whether the hook comes from a run without a terminal, such as `claude -p`.
pub fn is_headless(env: &HashMap<String, String>) -> bool {
    env.get("CLAUDE_CODE_ENTRYPOINT")
        .is_some_and(|e| HEADLESS.contains(&e.as_str()))
}

/// The cst pane that runs `claude attach` on a session.
fn attached_pane(session_id: &str, tmux: &dyn Runner, procs: &dyn Procs) -> Option<Mux> {
    let fmt = "#{socket_path}\t#{pane_id}\t#{pane_pid}";
    let out = tmux.run(&["-L", SOCKET, "list-panes", "-a", "-F", fmt]);
    out.lines().find_map(|line| {
        let fields: Vec<&str> = line.split('\t').collect();
        let [socket, pane, pid] = fields[..] else {
            return None;
        };
        // The pane runs claude itself, or a shell that started it
        let found = std::iter::once(pid.to_owned())
            .chain(procs.children(pid))
            .any(|p| attaches(&procs.cmdline(&p), session_id));
        found.then(|| Mux {
            socket: socket.to_owned(),
            pane: pane.to_owned(),
        })
    })
}

/// The arguments after `attach` in a command line, if it has one.
///
/// Not always `argv[1]`: claude may run through node, or with flags before the subcommand.
fn attach_args(argv: &[String]) -> Option<&[String]> {
    let at = argv.iter().position(|a| a == "attach")?;
    Some(&argv[at + 1..])
}

/// Tells whether a command line is `claude attach` on the session, by its id or a prefix.
pub fn attaches(argv: &[String], session_id: &str) -> bool {
    attach_args(argv).is_some_and(|after| {
        after
            .iter()
            .any(|a| a.chars().count() >= MIN_ID_PREFIX && session_id.starts_with(a.as_str()))
    })
}

/// What `claude stop` takes for a command line that is `claude attach`.
fn stop_target(argv: &[String]) -> Option<&str> {
    attach_args(argv)?
        .iter()
        .filter(|a| !a.starts_with('-'))
        .find(|a| a.chars().count() >= MIN_ID_PREFIX)
        .map(String::as_str)
}

/// `claude stop <id>`; a failure or a timeout is ignored.
fn stop_session(id: &str) {
    let mut cmd = Command::new("claude");
    cmd.args(["stop", id]);
    run_capped(cmd, STOP_TIMEOUT);
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Returns the panes of the tmux server `t` talks to.
///
/// Lines tmux did not format as asked are skipped.
pub fn panes(t: &dyn Runner) -> Vec<Pane> {
    t.run(&["list-panes", "-a", "-F", PANES_FORMAT])
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.splitn(7, '\t').collect();
            let [pane_id, window_id, index, window_index, sidebar, landing, name] = f[..] else {
                return None;
            };
            if !is_number(index) || !is_number(window_index) {
                return None;
            }
            Some(Pane {
                pane_id: pane_id.to_owned(),
                window_id: window_id.to_owned(),
                index: index.parse().ok()?,
                window_index: window_index.parse().ok()?,
                sidebar: !sidebar.is_empty(),
                landing: !landing.is_empty(),
                window_name: name.to_owned(),
            })
        })
        .collect()
}

/// Groups live session records into the windows of the tmux server at `socket`, most urgent
/// first.
///
/// Args:
///     records: The live records, most urgent first.
///     panes: The server's panes, from [`panes`].
///     socket: The socket of the server whose windows to list; records on other servers, and
///         all of them when it is empty, are left out.
pub fn windows_of(records: &[Record], panes: &[Pane], socket: &str) -> Vec<Window> {
    let by_pane: HashMap<&str, &Pane> = panes.iter().map(|p| (p.pane_id.as_str(), p)).collect();
    let mut windows: Vec<Window> = Vec::new();
    let mut slot: HashMap<&str, usize> = HashMap::new();
    // The session in a window's first pane: (pane index, record)
    let mut leaders: HashMap<&str, (u32, &Record)> = HashMap::new();
    // Records arrive most urgent first, so the first one in a window leads it
    for r in records {
        let Some(m) = r
            .mux
            .as_ref()
            .filter(|m| !socket.is_empty() && m.socket == socket)
        else {
            continue;
        };
        let Some(found) = by_pane.get(m.pane.as_str()) else {
            continue;
        };
        let id = found.window_id.as_str();
        slot.entry(id).or_insert_with(|| {
            windows.push(Window {
                window_id: id.to_owned(),
                lead: r.clone(),
                leader: None,
            });
            windows.len() - 1
        });
        match leaders.get(id) {
            Some((index, _)) if *index <= found.index => {}
            _ => {
                leaders.insert(id, (found.index, r));
            }
        }
    }
    for (id, (_, r)) in leaders {
        windows[slot[id]].leader = Some(r.clone());
    }
    windows.sort_by(|a, b| {
        let (a, b) = (state::sort_key(&a.lead), state::sort_key(&b.lead));
        a.0.cmp(&b.0).then(a.1.total_cmp(&b.1))
    });
    windows
}

/// Returns `(window_id, index, name)` of the server's windows, by index.
pub fn window_entries(panes: &[Pane]) -> Vec<(String, u32, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut rows: Vec<_> = panes
        .iter()
        .filter(|p| seen.insert(p.window_id.as_str()))
        .map(|p| (p.window_id.clone(), p.window_index, p.window_name.clone()))
        .collect();
    rows.sort_by_key(|row| row.1);
    rows
}

/// Names a window after its session's title, or its project folder until it has one.
///
/// The name is cut to [`WINDOW_NAME_LEN`] terminal cells, ending in `…`.
pub fn window_name(r: &Record, title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = if title.is_empty() {
        state::project(r)
    } else {
        &title
    };
    if name.width() <= WINDOW_NAME_LEN {
        return name.to_owned();
    }
    let mut cut = String::new();
    let mut used = 0;
    for c in name.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > WINDOW_NAME_LEN - 1 {
            break;
        }
        cut.push(c);
        used += w;
    }
    cut.push('…');
    cut
}

/// Gives a tmux window its session's name, unless the user named the window.
///
/// tmux names a window after its command ("claude") while automatic-rename is on; renaming
/// turns that off, so `@cs-auto-name` remembers the last name set here. A window still on
/// either of those is ours to rename; any other name came from the user.
///
/// `t` must talk to the server of the record's pane. Does nothing when tmux fails.
pub fn name_window(r: &Record, title: &str, t: &dyn Runner) {
    let Some(m) = &r.mux else { return };
    if r.cwd.is_empty() {
        return;
    }
    let out = t.run(&["display-message", "-p", "-t", &m.pane, NAMING_FORMAT]);
    let fields: Vec<&str> = out.split('\t').collect();
    // tmux failed, or the pane is gone
    let [auto, previous, current] = fields[..] else {
        return;
    };
    let wanted = window_name(r, title);
    if (auto == "1" || current == previous) && current != wanted {
        t.run(&["rename-window", "-t", &m.pane, &wanted]);
        t.run(&["set-option", "-w", "-t", &m.pane, "@cs-auto-name", &wanted]);
    }
}

/// Tells whether the pane is the active one in a window someone is looking at.
///
/// `t` must talk to the server of `m`.
pub fn pane_in_view(m: &Mux, t: &dyn Runner) -> bool {
    t.run(&["display-message", "-p", "-t", &m.pane, PANE_IN_VIEW_FORMAT]) == "1"
}

/// Marks a finished session seen right away when its pane is already on screen.
///
/// `t` must talk to the server of the record's pane.
pub fn settle_if_viewed(r: Record, now: f64, t: &dyn Runner) -> Record {
    match &r.mux {
        Some(m) if r.status == Status::Done && pane_in_view(m, t) => state::mark_seen(&r, now),
        _ => r,
    }
}

/// Closes a window, first stopping the background sessions its panes show.
///
/// A pane that runs `claude attach <id>` only views a session of Claude's daemon, which would
/// outlive the window, so each one is stopped. A failed stop does not keep the window open.
pub fn close_window(window_id: &str, t: &dyn Runner) {
    close_window_with(window_id, t, &Proc, &stop_session);
}

fn close_window_with(window_id: &str, t: &dyn Runner, procs: &dyn Procs, stop: &dyn Fn(&str)) {
    let pids = t.run(&["list-panes", "-t", window_id, "-F", "#{pane_pid}"]);
    for pid in pids.split_whitespace() {
        for p in std::iter::once(pid.to_owned()).chain(procs.children(pid)) {
            if let Some(id) = stop_target(&procs.cmdline(&p)) {
                stop(id);
            }
        }
    }
    // Left to itself, tmux goes back to the window used before, often the landing page
    let active = t.run(&["display-message", "-p", "-t", window_id, "#{window_active}"]);
    if active == "1" {
        if let Some(next) = next_session_window(window_id, &panes(t)) {
            t.run(&["select-window", "-t", &next]);
        }
    }
    t.run(&["kill-window", "-t", window_id]);
}

/// The session window to show when one closes: the next one by index, else the one before.
///
/// Session windows are those with a sidebar; `None` when the closing one is the last of them.
pub fn next_session_window(window_id: &str, panes: &[Pane]) -> Option<String> {
    let with_sidebar: std::collections::HashSet<&str> = panes
        .iter()
        .filter(|p| p.sidebar)
        .map(|p| p.window_id.as_str())
        .collect();
    let order: Vec<String> = window_entries(panes)
        .into_iter()
        .map(|(id, _, _)| id)
        .filter(|id| with_sidebar.contains(id.as_str()) || id == window_id)
        .collect();
    let at = order.iter().position(|id| id == window_id)?;
    order[at + 1..]
        .first()
        .or_else(|| order[..at].last())
        .cloned()
}

/// Puts the sidebar in a pane on the far left of a window, unless it already has one.
///
/// The pane is marked `@cs-sidebar` and the window's other pane stays the active one.
pub fn add_sidebar(window_id: &str, t: &dyn Runner) {
    add_sidebar_running(window_id, SIDEBAR_COMMAND, t);
}

/// [`add_sidebar`] with the command that runs in the new pane.
pub fn add_sidebar_running(window_id: &str, command: &str, t: &dyn Runner) {
    let flags = t.run(&["list-panes", "-t", window_id, "-F", "#{@cs-sidebar}"]);
    if flags.split_whitespace().any(|f| f == "1") {
        return;
    }
    let width = SIDEBAR_WIDTH.to_string();
    let pane = t.run(&[
        "split-window",
        "-hbd",
        "-f",
        "-l",
        &width,
        "-t",
        window_id,
        "-P",
        "-F",
        "#{pane_id}",
        command,
    ]);
    if !pane.is_empty() {
        t.run(&["set-option", "-p", "-t", &pane, "@cs-sidebar", "1"]);
    }
}

/// Resizes every sidebar pane back to its width, after a split or a terminal resize.
pub fn fit_sidebars(t: &dyn Runner) {
    let width = SIDEBAR_WIDTH.to_string();
    for pane in panes(t).iter().filter(|p| p.sidebar) {
        t.run(&["resize-pane", "-t", &pane.pane_id, "-x", &width]);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use crate::state::{new_record, Mux, Record, Status};
    use crate::term::tmux::{
        add_sidebar_running, attaches, close_window_with, detect, detect_with, fit_sidebars,
        give_up, is_headless, name_window, next_session_window, pane_in_view, panes, run_capped,
        run_capped_with, settle_if_viewed, wait_until, wait_until_with, window_entries,
        window_name, windows_of, Cli, Pane, Procs, Runner, Window, SIDEBAR_WIDTH, WINDOW_NAME_LEN,
    };

    type Reply = Box<dyn Fn(&[&str]) -> String>;

    /// Replays outputs and records every call.
    struct Fake {
        calls: RefCell<Vec<Vec<String>>>,
        reply: Reply,
    }

    impl Fake {
        fn new(reply: impl Fn(&[&str]) -> String + 'static) -> Self {
            Fake {
                calls: RefCell::new(Vec::new()),
                reply: Box::new(reply),
            }
        }

        fn silent() -> Self {
            Fake::new(|_| String::new())
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.borrow().clone()
        }
    }

    impl Runner for Fake {
        fn run(&self, args: &[&str]) -> String {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|a| (*a).to_owned()).collect());
            (self.reply)(args)
        }
    }

    /// A process table: command lines and children by pid.
    #[derive(Default)]
    struct Table {
        cmdlines: HashMap<&'static str, Vec<&'static str>>,
        children: HashMap<&'static str, Vec<&'static str>>,
    }

    impl Procs for Table {
        fn cmdline(&self, pid: &str) -> Vec<String> {
            let argv = self.cmdlines.get(pid).cloned().unwrap_or_default();
            argv.into_iter().map(str::to_owned).collect()
        }

        fn children(&self, pid: &str) -> Vec<String> {
            let kids = self.children.get(pid).cloned().unwrap_or_default();
            kids.into_iter().map(str::to_owned).collect()
        }
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn tmux_env() -> HashMap<String, String> {
        env(&[
            ("TMUX", "/tmp/ttmux-1000/claude,123,0"),
            ("TMUX_PANE", "%3"),
        ])
    }

    fn mux(socket: &str, pane: &str) -> Mux {
        Mux {
            socket: socket.into(),
            pane: pane.into(),
        }
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_owned()).collect()
    }

    const SID: &str = "685e0c8a-d242-44fd-ab34-55ac744fd5ab";

    fn pane(
        id: &str,
        window: &str,
        index: u32,
        window_index: u32,
        sidebar: bool,
        name: &str,
    ) -> Pane {
        Pane {
            pane_id: id.into(),
            window_id: window.into(),
            index,
            window_index,
            sidebar,
            landing: false,
            window_name: name.into(),
        }
    }

    fn at(id: &str, status: Status, pane: &str, socket: &str) -> Record {
        Record {
            status,
            mux: Some(mux(socket, pane)),
            ..new_record(id)
        }
    }

    fn at_cwd(cwd: &str) -> Record {
        Record {
            cwd: cwd.into(),
            ..new_record("s")
        }
    }

    fn listing(rows: &[&[&str]]) -> String {
        rows.iter()
            .map(|r| r.join("\t"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn detect_outside_tmux_is_none_and_inside_is_the_pane() {
        let t = Fake::silent();
        assert_eq!(detect(&HashMap::new(), "s1", &t), None);
        assert_eq!(
            detect(&tmux_env(), "s1", &t),
            Some(mux("/tmp/ttmux-1000/claude", "%3"))
        );
        assert!(t.calls().is_empty());
    }

    #[test]
    fn detect_needs_both_tmux_and_the_pane() {
        let t = Fake::silent();
        assert_eq!(detect(&env(&[("TMUX", "/s,1,0")]), "s1", &t), None);
        assert_eq!(detect(&env(&[("TMUX_PANE", "%3")]), "s1", &t), None);
    }

    #[test]
    fn detect_ignores_the_tab_a_headless_run_inherited() {
        let t = Fake::silent();
        let mut headless = tmux_env();
        headless.insert("CLAUDE_CODE_ENTRYPOINT".into(), "sdk-cli".into());
        assert_eq!(detect(&headless, "s1", &t), None);
        headless.insert("CLAUDE_CODE_ENTRYPOINT".into(), "cli".into());
        assert_eq!(
            detect(&headless, "s1", &t).map(|m| m.pane),
            Some("%3".into())
        );
        assert!(is_headless(&env(&[("CLAUDE_CODE_ENTRYPOINT", "sdk-py")])));
        assert!(!is_headless(&HashMap::new()));
    }

    fn attach_table() -> Table {
        let mut t = Table::default();
        t.cmdlines.insert("100", vec!["zsh"]);
        t.cmdlines
            .insert("101", vec!["/bin/claude", "attach", "685e0c8a"]);
        t.cmdlines.insert("200", vec!["claude"]);
        t.children.insert("100", vec!["101"]);
        t
    }

    fn attach_listing() -> Fake {
        Fake::new(|_| "/s/claude\t%1\t100\n/s/claude\t%2\t200".into())
    }

    #[test]
    fn detect_finds_the_tab_attached_to_a_background_session() {
        let t = attach_listing();
        let procs = attach_table();
        let bg = env(&[("CLAUDE_CODE_SESSION_KIND", "bg")]);
        let want = Some(mux("/s/claude", "%1"));
        assert_eq!(detect_with(&bg, SID, &t, &procs), want);
        // The listing asks the claude server, not the current one
        assert_eq!(t.calls()[0][..3], ["-L", "claude", "list-panes"]);
        // A hook sees the job folder, not the kind
        let job = env(&[("CLAUDE_JOB_DIR", "/home/u/.claude/jobs/685e0c8a")]);
        assert_eq!(detect_with(&job, SID, &t, &procs), want);
    }

    #[test]
    fn detect_a_background_session_attached_nowhere_has_no_pane() {
        let t = attach_listing();
        let bg = env(&[("CLAUDE_CODE_SESSION_KIND", "bg")]);
        assert_eq!(detect_with(&bg, "ffffffff-0000", &t, &attach_table()), None);
        assert_eq!(detect_with(&bg, "", &t, &attach_table()), None);
        assert_eq!(detect_with(&HashMap::new(), SID, &t, &attach_table()), None);
    }

    #[test]
    fn detect_a_background_session_ignores_the_pane_it_left_behind() {
        let t = attach_listing();
        let mut bg = tmux_env();
        bg.insert("CLAUDE_CODE_SESSION_KIND".into(), "bg".into());
        assert_eq!(
            detect_with(&bg, SID, &t, &attach_table()),
            Some(mux("/s/claude", "%1"))
        );
    }

    #[test]
    fn detect_a_background_session_wins_over_a_headless_entrypoint() {
        let t = attach_listing();
        let job = env(&[
            ("CLAUDE_JOB_DIR", "/j"),
            ("CLAUDE_CODE_ENTRYPOINT", "sdk-cli"),
        ]);
        assert_eq!(
            detect_with(&job, SID, &t, &attach_table()).map(|m| m.pane),
            Some("%1".into())
        );
    }

    #[test]
    fn detect_skips_listing_lines_tmux_did_not_format() {
        let t = Fake::new(|_| "warning\n/s/claude\t%1\t100\n/s/claude\t%2\t200".into());
        let bg = env(&[("CLAUDE_CODE_SESSION_KIND", "bg")]);
        assert_eq!(
            detect_with(&bg, SID, &t, &attach_table()),
            Some(mux("/s/claude", "%1"))
        );
    }

    #[test]
    fn attaches_matches_the_session_id_or_its_prefix() {
        assert!(attaches(&argv(&["/bin/claude", "attach", "685e0c8a"]), SID));
        assert!(attaches(&argv(&["claude", "attach", SID]), SID));
        assert!(attaches(
            &argv(&["node", "/lib/claude/cli.js", "attach", "685e0c8a"]),
            SID
        ));
        assert!(!attaches(&argv(&["claude", "attach", "685e"]), SID));
        assert!(!attaches(&argv(&["claude", "--resume", "685e0c8a"]), SID));
        assert!(!attaches(&[], SID));
    }

    #[test]
    fn window_name_is_the_title_or_else_the_project_folder() {
        assert_eq!(window_name(&at_cwd("/w/vehicle.k265"), ""), "vehicle.k265");
        assert_eq!(window_name(&at_cwd("/w/notes/"), ""), "notes");
        assert_eq!(
            window_name(&at_cwd("/w/api"), "Fix the\nlogin  bug"),
            "Fix the login bug"
        );
    }

    #[test]
    fn window_name_is_cut_to_sixty_cells_with_an_ellipsis() {
        let long = window_name(&at_cwd(&format!("/w/{}", "x".repeat(80))), "");
        assert_eq!(long.chars().count(), WINDOW_NAME_LEN);
        assert!(long.ends_with('…'));
        let exact = "y".repeat(WINDOW_NAME_LEN);
        assert_eq!(window_name(&at_cwd("/w/a"), &exact), exact);
        // Wide characters take two cells each
        let wide = window_name(&at_cwd("/w/a"), &"界".repeat(40));
        assert_eq!(wide, format!("{}…", "界".repeat(29)));
    }

    #[test]
    fn run_capped_returns_empty_when_the_command_is_missing_or_slow() {
        let missing = Command::new("/nonexistent/tmux");
        assert_eq!(run_capped(missing, Duration::from_secs(1)), "");
        let mut slow = Command::new("sleep");
        slow.arg("5");
        let start = Instant::now();
        assert_eq!(run_capped(slow, Duration::from_millis(100)), "");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn run_capped_gives_up_at_once_when_the_reader_thread_cannot_start() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let refuse = |_job: Box<dyn FnOnce() + Send>| Err(std::io::Error::other("no threads"));
        let start = Instant::now();
        assert_eq!(run_capped_with(cmd, Duration::from_secs(5), &refuse), "");
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_child_is_killed_and_reaped_when_given_up_on() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let mut child = cmd.spawn().expect("spawn sleep");
        give_up(&mut child);
        // `try_wait` only reports a status for a child that was waited for
        assert!(matches!(child.try_wait(), Ok(Some(status)) if !status.success()));
    }

    #[test]
    fn wait_until_does_not_poll_for_a_child_that_exits_soon() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 0.05"]);
        let mut child = cmd.spawn().expect("spawn sh");
        let mut sleeps = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        assert!(wait_until_with(&mut child, deadline, &mut || sleeps += 1));
        assert_eq!(sleeps, 0);
    }

    #[test]
    fn wait_until_stops_at_the_deadline_for_a_child_that_lingers() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let mut child = cmd.spawn().expect("spawn sleep");
        let start = Instant::now();
        let deadline = start + Duration::from_millis(100);
        assert!(!wait_until(&mut child, deadline));
        assert!(start.elapsed() < Duration::from_secs(2));
        give_up(&mut child);
    }

    #[test]
    fn run_capped_does_not_hang_on_a_child_that_closes_stdout_and_lingers() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exec 1>&-; sleep 30"]);
        let start = Instant::now();
        assert_eq!(run_capped(cmd, Duration::from_millis(200)), "");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn run_capped_strips_one_trailing_newline() {
        let mut cmd = Command::new("printf");
        cmd.arg("a\n\n");
        assert_eq!(run_capped(cmd, Duration::from_secs(1)), "a\n");
    }

    #[test]
    fn run_capped_reads_output_larger_than_a_pipe() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "head -c 300000 /dev/zero | tr '\\0' x"]);
        assert_eq!(run_capped(cmd, Duration::from_secs(5)).len(), 300_000);
    }

    #[test]
    fn cli_with_a_gone_server_returns_empty() {
        let cli = Cli {
            socket: Some("/nonexistent/tmux-sock".into()),
        };
        assert_eq!(cli.run(&["list-windows"]), "");
    }

    #[test]
    fn windows_of_keeps_this_server_and_the_most_urgent_per_window() {
        let listed = [
            pane("%1", "@1", 1, 1, false, "a"),
            pane("%2", "@1", 2, 1, false, "a"),
            pane("%3", "@2", 1, 2, false, "b"),
            pane("%4", "@3", 1, 3, false, "c"),
        ];
        let records = [
            at("a", Status::Permission, "%2", "/s/claude"),
            at("b", Status::Working, "%1", "/s/claude"),
            at("c", Status::Idle, "%3", "/s/claude"),
            at("d", Status::Permission, "%4", "/s/other"),
        ];
        let windows = windows_of(&records, &listed, "/s/claude");
        let got: Vec<_> = windows
            .iter()
            .map(|w| (w.window_id.as_str(), w.lead.id.as_str()))
            .collect();
        assert_eq!(got, [("@1", "a"), ("@2", "c")]);
    }

    #[test]
    fn windows_of_picks_the_leader_by_its_pane_not_by_urgency() {
        // The sidebar is pane 1; the first Claude pane is 2, a teammate's is 3
        let listed = [
            pane("%9", "@1", 1, 1, true, "a"),
            pane("%2", "@1", 2, 1, false, "a"),
            pane("%3", "@1", 3, 1, false, "a"),
            pane("%7", "@2", 4, 2, false, "b"),
            pane("%6", "@2", 3, 2, false, "b"),
        ];
        let s = "/s/claude";
        let records = [
            at("mate", Status::Permission, "%3", s),
            at("lead", Status::Idle, "%2", s),
            at("late", Status::Working, "%7", s),
            at("early", Status::Working, "%6", s),
        ];
        let windows = windows_of(&records, &listed, s);
        let by: HashMap<&str, &Window> =
            windows.iter().map(|w| (w.window_id.as_str(), w)).collect();
        assert_eq!(by["@1"].lead.id, "mate");
        assert_eq!(
            by["@1"].leader.as_ref().map(|r| r.id.as_str()),
            Some("lead")
        );
        assert_eq!(by["@1"].lead.status, Status::Permission);
        assert_eq!(by["@2"].head().id, "early");
    }

    #[test]
    fn head_is_the_lead_when_there_is_no_leader() {
        let w = Window {
            window_id: "@1".into(),
            lead: new_record("a"),
            leader: None,
        };
        assert_eq!(w.head().id, "a");
    }

    #[test]
    fn windows_of_outside_tmux_is_empty() {
        let listed = [pane("%1", "@1", 1, 1, false, "a")];
        let records = [at("a", Status::Idle, "%1", "")];
        assert!(windows_of(&records, &listed, "").is_empty());
    }

    #[test]
    fn windows_of_sorts_with_a_total_order_even_with_nan() {
        let listed = [
            pane("%1", "@1", 1, 1, false, "a"),
            pane("%2", "@2", 1, 2, false, "b"),
        ];
        let mut a = at("a", Status::Idle, "%1", "/s");
        a.status_since = f64::NAN;
        let b = at("b", Status::Idle, "%2", "/s");
        assert_eq!(windows_of(&[a, b], &listed, "/s").len(), 2);
    }

    #[test]
    fn window_entries_lists_each_window_once_by_index() {
        let listed = [
            pane("%2", "@2", 1, 2, false, "b"),
            pane("%3", "@2", 2, 2, false, "b"),
            pane("%1", "@1", 1, 1, false, "a"),
        ];
        assert_eq!(
            window_entries(&listed),
            [("@1".into(), 1, "a".into()), ("@2".into(), 2, "b".into())]
        );
    }

    #[test]
    fn panes_skip_lines_tmux_did_not_format_and_keep_tabs_in_names() {
        let out = format!(
            "warning\n{}",
            listing(&[
                &["%2", "@1", "2", "1", "1", "", "a\tb"],
                &["%3", "@2", "x", "2", "", "", "c"],
                &["%4", "@3", "1", "3", "", "1", "sessions"],
            ])
        );
        let t = Fake::new(move |_| out.clone());
        let got = panes(&t);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], pane("%2", "@1", 2, 1, true, "a\tb"));
        assert!(got[1].landing && !got[1].sidebar);
        assert_eq!(t.calls()[0][..3], ["list-panes", "-a", "-F"]);
        assert!(t.calls()[0][3].ends_with("#{@cs-landing}\t#{window_name}"));
    }

    /// Windows @1 (the landing page, no sidebar), @2 and @3 (sessions).
    fn windows_reply(active: &'static str) -> Fake {
        Fake::new(move |args| match args {
            ["list-panes", "-a", ..] => listing(&[
                &["%1", "@1", "1", "1", "", "1", "sessions"],
                &["%2", "@2", "1", "2", "1", "", "api"],
                &["%3", "@2", "2", "2", "", "", "api"],
                &["%4", "@3", "1", "3", "1", "", "web"],
                &["%5", "@3", "2", "3", "", "", "web"],
            ]),
            ["display-message", ..] => active.into(),
            _ => String::new(),
        })
    }

    fn close_without_stops(window: &str, t: &Fake) {
        close_window_with(window, t, &Table::default(), &|_| {});
    }

    #[test]
    fn closing_the_window_on_screen_moves_to_the_next_session_not_the_landing() {
        let t = windows_reply("1");
        close_without_stops("@2", &t);
        let calls = t.calls();
        assert_eq!(calls[calls.len() - 2], ["select-window", "-t", "@3"]);
        assert_eq!(calls[calls.len() - 1], ["kill-window", "-t", "@2"]);
        assert_eq!(next_session_window("@3", &panes(&t)), Some("@2".into()));
    }

    #[test]
    fn closing_the_last_session_or_a_hidden_one_selects_nothing() {
        let t = windows_reply("0");
        close_without_stops("@2", &t);
        assert!(t.calls().iter().all(|c| c[0] != "select-window"));
        let two = [
            pane("%1", "@1", 1, 1, false, "sessions"),
            pane("%2", "@2", 1, 2, true, "api"),
        ];
        assert_eq!(next_session_window("@2", &two), None);
        assert_eq!(next_session_window("@9", &two), None);
    }

    #[test]
    fn close_window_stops_attached_background_sessions() {
        let t = Fake::new(|args| {
            if args[0] == "list-panes" {
                "100\n200".into()
            } else {
                String::new()
            }
        });
        let mut procs = Table::default();
        procs
            .cmdlines
            .insert("101", vec!["/bin/claude", "attach", "685e0c8a"]);
        procs.cmdlines.insert("200", vec!["claude"]);
        procs.children.insert("100", vec!["101"]);
        let stops = RefCell::new(Vec::new());
        close_window_with("@2", &t, &procs, &|id| {
            stops.borrow_mut().push(id.to_owned())
        });
        let calls = t.calls();
        assert_eq!(calls[0], ["list-panes", "-t", "@2", "-F", "#{pane_pid}"]);
        assert_eq!(calls[calls.len() - 1], ["kill-window", "-t", "@2"]);
        assert_eq!(*stops.borrow(), ["685e0c8a"]);
    }

    #[test]
    fn close_window_skips_flags_and_short_ids_when_stopping() {
        let t = Fake::new(|args| {
            if args[0] == "list-panes" {
                "100".into()
            } else {
                String::new()
            }
        });
        let mut procs = Table::default();
        procs.cmdlines.insert(
            "100",
            vec!["claude", "attach", "--verbose", "abc", "685e0c8a"],
        );
        let stops = RefCell::new(Vec::new());
        close_window_with("@2", &t, &procs, &|id| {
            stops.borrow_mut().push(id.to_owned())
        });
        assert_eq!(*stops.borrow(), ["685e0c8a"]);
        let mut procs = Table::default();
        procs
            .cmdlines
            .insert("100", vec!["claude", "attach", "abc"]);
        let stops = RefCell::new(Vec::new());
        close_window_with("@2", &t, &procs, &|id| {
            stops.borrow_mut().push(id.to_owned())
        });
        assert!(stops.borrow().is_empty());
    }

    fn record_in(pane: &str, status: Status) -> Record {
        at("s", status, pane, "/s")
    }

    #[test]
    fn pane_in_view_asks_tmux_for_the_active_pane_of_an_attached_window() {
        let t = Fake::new(|_| "1".into());
        assert!(pane_in_view(&mux("/s", "%4"), &t));
        let call = &t.calls()[0];
        assert_eq!(call[..4], ["display-message", "-p", "-t", "%4"]);
        assert_eq!(
            call[4],
            "#{&&:#{&&:#{pane_active},#{window_active}},#{session_attached}}"
        );
        assert!(!pane_in_view(&mux("/s", "%4"), &Fake::new(|_| "0".into())));
        assert!(!pane_in_view(&mux("/s", "%4"), &Fake::silent()));
    }

    #[test]
    fn settle_if_viewed_marks_a_finished_session_in_view_seen() {
        let t = Fake::new(|_| "1".into());
        let mut done = record_in("%1", Status::Done);
        done.status_since = 1.0;
        let settled = settle_if_viewed(done, 5.0, &t);
        assert_eq!((settled.status, settled.status_since), (Status::Idle, 5.0));
    }

    #[test]
    fn settle_if_viewed_keeps_a_session_behind_or_unfinished_or_off_tmux() {
        let behind = record_in("%1", Status::Done);
        assert_eq!(
            settle_if_viewed(behind.clone(), 5.0, &Fake::new(|_| "0".into())),
            behind
        );
        let t = Fake::new(|_| "1".into());
        let working = record_in("%1", Status::Working);
        assert_eq!(settle_if_viewed(working.clone(), 5.0, &t), working);
        let bare = Record {
            status: Status::Done,
            ..new_record("s")
        };
        assert_eq!(settle_if_viewed(bare.clone(), 5.0, &t), bare);
        // Neither of the last two asked tmux
        assert!(t.calls().is_empty());
        // A dead server leaves the record as it is
        let dead = Fake::silent();
        let done = record_in("%1", Status::Done);
        assert_eq!(settle_if_viewed(done.clone(), 5.0, &dead), done);
    }

    fn naming_reply(auto: &'static str, previous: &'static str, current: &'static str) -> Fake {
        Fake::new(move |args| {
            if args[0] == "display-message" {
                format!("{auto}\t{previous}\t{current}")
            } else {
                String::new()
            }
        })
    }

    fn with_cwd(cwd: &str) -> Record {
        Record {
            cwd: cwd.into(),
            mux: Some(mux("/s", "%4")),
            ..new_record("s")
        }
    }

    #[test]
    fn name_window_renames_an_automatic_name_and_remembers_it() {
        let t = naming_reply("1", "", "claude");
        name_window(&with_cwd("/w/api"), "", &t);
        let calls = t.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0][..4], ["display-message", "-p", "-t", "%4"]);
        assert_eq!(
            calls[0][4],
            "#{automatic-rename}\t#{@cs-auto-name}\t#{window_name}"
        );
        assert_eq!(calls[1], ["rename-window", "-t", "%4", "api"]);
        assert_eq!(
            calls[2],
            ["set-option", "-w", "-t", "%4", "@cs-auto-name", "api"]
        );
    }

    #[test]
    fn name_window_renames_the_name_it_set_itself() {
        let t = naming_reply("0", "api", "api");
        name_window(&with_cwd("/w/notes"), "", &t);
        assert_eq!(t.calls()[1], ["rename-window", "-t", "%4", "notes"]);
    }

    #[test]
    fn name_window_leaves_a_name_the_user_gave_alone() {
        let t = naming_reply("0", "api", "mine");
        name_window(&with_cwd("/w/api"), "", &t);
        assert_eq!(t.calls().len(), 1);
    }

    #[test]
    fn name_window_does_nothing_when_the_name_is_already_right() {
        let t = naming_reply("1", "", "api");
        name_window(&with_cwd("/w/api"), "", &t);
        assert_eq!(t.calls().len(), 1);
    }

    #[test]
    fn name_window_without_a_pane_a_cwd_or_a_server_does_nothing() {
        let t = naming_reply("1", "", "claude");
        name_window(&at_cwd("/w/api"), "", &t);
        name_window(
            &Record {
                cwd: String::new(),
                ..with_cwd("")
            },
            "",
            &t,
        );
        assert!(t.calls().is_empty());
        let dead = Fake::silent();
        name_window(&with_cwd("/w/api"), "", &dead);
        assert_eq!(dead.calls().len(), 1);
    }

    #[test]
    fn add_sidebar_splits_left_and_marks_the_new_pane() {
        let t = Fake::new(|args| match args[0] {
            "split-window" => "%7".into(),
            _ => String::new(),
        });
        add_sidebar_running("@2", "sleep 600", &t);
        let calls = t.calls();
        assert_eq!(calls[0], ["list-panes", "-t", "@2", "-F", "#{@cs-sidebar}"]);
        let width = SIDEBAR_WIDTH.to_string();
        let split = [
            "split-window",
            "-hbd",
            "-f",
            "-l",
            &width,
            "-t",
            "@2",
            "-P",
            "-F",
            "#{pane_id}",
            "sleep 600",
        ];
        assert_eq!(calls[1], split);
        assert_eq!(
            calls[2],
            ["set-option", "-p", "-t", "%7", "@cs-sidebar", "1"]
        );
    }

    #[test]
    fn add_sidebar_does_nothing_when_the_window_has_one() {
        let t = Fake::new(|_| "\n1".into());
        add_sidebar_running("@2", "sleep 600", &t);
        assert_eq!(t.calls().len(), 1);
    }

    #[test]
    fn add_sidebar_does_not_mark_when_the_split_failed() {
        let t = Fake::silent();
        add_sidebar_running("@2", "sleep 600", &t);
        assert_eq!(t.calls().len(), 2);
    }

    #[test]
    fn fit_sidebars_resizes_only_sidebar_panes() {
        let t = windows_reply("0");
        fit_sidebars(&t);
        let resizes: Vec<_> = t
            .calls()
            .into_iter()
            .filter(|c| c[0] == "resize-pane")
            .collect();
        assert_eq!(
            resizes,
            [
                ["resize-pane", "-t", "%2", "-x", "29"],
                ["resize-pane", "-t", "%4", "-x", "29"],
            ]
        );
    }
}
