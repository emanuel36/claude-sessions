//! Child processes that run under a time cap: started, read on a thread, waited for until a
//! deadline, and killed and reaped when they run late.

use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::fd::OwnedFd;
use rustix::process::{pidfd_open, Pid, PidfdFlags};

/// How often a running command is checked for having exited, where a pidfd is not available.
const POLL: Duration = Duration::from_millis(2);

/// Starts a job on a thread of its own; a seam for tests.
pub type Spawner<'a> = &'a dyn Fn(Box<dyn FnOnce() + Send>) -> io::Result<()>;

/// Starts `job` on a new thread; unlike `thread::spawn`, fails instead of panicking when the
/// system is out of threads.
///
/// # Errors
///
/// Fails if the thread cannot be created.
pub fn spawn_thread(job: Box<dyn FnOnce() + Send>) -> io::Result<()> {
    thread::Builder::new().spawn(job).map(drop)
}

/// Runs a command for at most `timeout` and returns its stdout minus one trailing newline.
///
/// Returns `""` when the command cannot start or does not finish in time; it is killed then.
pub fn run_capped(cmd: Command, timeout: Duration) -> String {
    run_capped_with(cmd, timeout, &spawn_thread)
}

fn run_capped_with(mut cmd: Command, timeout: Duration, spawn: Spawner<'_>) -> String {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return String::new();
    };
    let deadline = Instant::now() + timeout;
    let Some(stdout) = child.stdout.take() else {
        give_up(&mut child);
        return String::new();
    };
    let Ok(rx) = read_on_thread(stdout, spawn) else {
        give_up(&mut child);
        return String::new();
    };
    match collect(&mut child, &rx, timeout, deadline) {
        Some(buf) => {
            let text = String::from_utf8_lossy(&buf);
            text.strip_suffix('\n').unwrap_or(&text).to_owned()
        }
        None => {
            give_up(&mut child);
            String::new()
        }
    }
}

/// Reads `from` to its end on a thread started by `spawn`, which sends what it read.
///
/// A thread keeps a full pipe from stalling the child; it is left to end by itself when the
/// child is killed.
///
/// # Errors
///
/// Fails if the thread cannot start.
pub fn read_on_thread(
    mut from: impl Read + Send + 'static,
    spawn: Spawner<'_>,
) -> io::Result<mpsc::Receiver<Vec<u8>>> {
    let (tx, rx) = mpsc::channel();
    spawn(Box::new(move || {
        let mut buf = Vec::new();
        let _ = from.read_to_end(&mut buf);
        let _ = tx.send(buf);
    }))?;
    Ok(rx)
}

/// Waits up to `timeout` for the child's output on `rx`, then until `deadline` for the child to
/// exit; returns the output, or `None` if either ran late and the caller is to give up on it.
pub fn collect(
    child: &mut Child,
    rx: &mpsc::Receiver<Vec<u8>>,
    timeout: Duration,
    deadline: Instant,
) -> Option<Vec<u8>> {
    let buf = rx.recv_timeout(timeout).ok()?;
    wait_until(child, deadline).then_some(buf)
}

/// Kills a child and waits for it, so it leaves no zombie behind.
pub fn give_up(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Waits for the child to exit until `deadline`; false if it is still running.
pub fn wait_until(child: &mut Child, deadline: Instant) -> bool {
    wait_until_with(child, deadline, &mut || thread::sleep(POLL))
}

fn wait_until_with(child: &mut Child, deadline: Instant, sleep: &mut dyn FnMut()) -> bool {
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

fn child_pidfd(child: &Child) -> Option<OwnedFd> {
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

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::{Duration, Instant};

    use crate::process::{give_up, run_capped, run_capped_with, wait_until, wait_until_with};

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
}
