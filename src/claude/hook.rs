//! `cst hook` and what follows from Claude Code's events: the live sessions, as they really are.
//!
//! Esc fires no hook, so a session the user interrupted is settled here, when it is read.
//!
//! Each public function is a thin wrapper over one that takes [`Seams`] (the records directory,
//! the journal, and how to reach a tmux server), so unit tests use a temporary directory and a
//! fake tmux and never touch the process environment, the real journal or a real tmux.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::claude::transcript::{title_of, was_interrupted};
use crate::core::journal;
use crate::core::paths;
use crate::core::state::{
    apply_event, mark_interrupted, mark_seen, sort_key, Event, Record, Status, INTERRUPTIBLE,
};
use crate::core::store::{self, Sink};
use crate::term::tmux::{self, Cli, Runner};

/// Events after which the window takes the session's folder name.
///
/// Claude Code writes a session's title during its first turn: `Stop` picks it up.
pub const NAMING_EVENTS: [&str; 3] = ["SessionStart", "UserPromptSubmit", "Stop"];
/// How a session starts when it takes the place of another in the same Claude process.
pub const REPLACING: [&str; 2] = ["clear", "resume"];
/// A working session with no hook for this long is checked for an Esc interrupt.
pub const QUIET_SECONDS: f64 = 3.0;

/// Gives a runner for the tmux server at a socket path.
type Binder = dyn Fn(&str) -> Box<dyn Runner>;

/// A borrowed [`Binder`].
type Bind<'a> = &'a Binder;

/// What the hook needs from outside the process.
struct Seams<'a> {
    /// The records directory.
    dir: &'a Path,
    /// Where status changes are reported.
    journal: Sink<'a>,
    /// How to talk to the tmux server of a record's pane.
    bind: Bind<'a>,
}

/// Runs `f` with the real records directory, journal and tmux.
///
/// Fails if the records directory is unusable.
fn with_seams<R>(f: impl FnOnce(&Seams<'_>) -> R) -> Result<R> {
    let dir = paths::runtime_dir().context("cannot open the records directory")?;
    let seams = Seams {
        dir: &dir,
        journal: &journal::append,
        bind: &|socket| Box::new(Cli::at(socket)),
    };
    Ok(f(&seams))
}

/// Parses and checks the hook input.
///
/// # Errors
///
/// Fails if the input is not JSON, or lacks a session id that can name a record or the name
/// of the hook event.
pub fn parse_event(input: &[u8]) -> Result<Event> {
    let event: Event = serde_json::from_slice(input).context("not a hook input")?;
    if !store::is_session_id(&event.session_id) {
        bail!("bad session id {:?}", event.session_id);
    }
    if event.hook_event_name.is_empty() {
        bail!("no hook_event_name");
    }
    Ok(event)
}

/// Parses one hook input and applies it.
///
/// # Errors
///
/// Fails for a malformed input, or if the record cannot be locked or written.
pub fn run(input: &[u8], env: &HashMap<String, String>, now: f64, t: &dyn Runner) -> Result<()> {
    handle_event(&parse_event(input)?, env, now, t)
}

/// The PID of the Claude process that ran the hook.
fn claude_pid(env: &HashMap<String, String>) -> i32 {
    env.get("CLAUDE_PID")
        .filter(|p| p.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| i32::try_from(std::os::unix::process::parent_id()).unwrap_or(0))
}

/// Applies one hook event to its session record and tells tmux.
///
/// `t` talks to the server this process runs in; it finds the pane of a background session.
/// The server of the record's own pane is reached by its socket.
///
/// # Errors
///
/// Fails if the records directory is unusable, or the record cannot be locked or written.
pub fn handle_event(
    event: &Event,
    env: &HashMap<String, String>,
    now: f64,
    t: &dyn Runner,
) -> Result<()> {
    with_seams(|s| handle_event_in(s, event, env, now, t))?
}

fn handle_event_in(
    s: &Seams<'_>,
    event: &Event,
    env: &HashMap<String, String>,
    now: f64,
    t: &dyn Runner,
) -> Result<()> {
    let id = event.session_id.as_str();
    let name = event.hook_event_name.as_str();
    let lock = store::lock_in(s.dir, id).context("cannot lock the record")?;
    if name == "SessionEnd" {
        // /clear and /resume end a session as they start another in the same process, and the
        // two hooks may run in either order: removing here could take the record the new
        // session just wrote (a resumed session keeps its id). The new session's start removes
        // the one it replaced.
        if !event
            .reason
            .as_deref()
            .is_some_and(|r| REPLACING.contains(&r))
        {
            // The lock stays: a hook may be waiting on it, and the dead-process sweep removes it
            store::remove_in(s.dir, id, true, s.journal);
        }
        return Ok(());
    }
    let mut record = apply_event(store::read_in(s.dir, id).as_ref(), event, now);
    let pid = claude_pid(env);
    record.pid = Some(pid);
    record.pid_start = store::process_start(pid);
    record.mux = tmux::detect(env, id, t);
    // A run without a terminal (a start hook's `claude -p`) is listed apart from the sessions
    record.headless = tmux::is_headless(env);
    // The pane's own server, as `tmux -S <socket>`
    let pane_server = record.mux.as_ref().map(|m| (s.bind)(&m.socket));
    if let Some(server) = &pane_server {
        record = tmux::settle_if_viewed(record, now, &**server);
    }
    let naming = NAMING_EVENTS.contains(&name);
    if naming {
        // A title found once stays, even after it scrolls out of the transcript's tail
        let found = record
            .transcript
            .as_deref()
            .map(title_of)
            .unwrap_or_default();
        if !found.is_empty() {
            record.title = found;
        }
    }
    store::write_in(s.dir, &record, s.journal).context("cannot write the record")?;
    // Told under the lock, so overlapping hooks reach tmux in the order they hit the disk
    if let (true, Some(server)) = (naming, &pane_server) {
        tmux::name_window(&record, &record.title, &**server);
    }
    drop(lock);
    if name == "SessionStart"
        && event
            .source
            .as_deref()
            .is_some_and(|r| REPLACING.contains(&r))
    {
        store::remove_replaced_in(s.dir, &record, s.journal);
    }
    Ok(())
}

/// Returns the records of all live sessions, ordered for display.
///
/// Records of dead processes are deleted, and sessions the user interrupted with Esc (in the
/// middle of a turn, or at a prompt) are settled as idle on disk.
pub fn live_sessions(now: f64) -> Vec<Record> {
    with_seams(|s| live_sessions_in(s, now)).unwrap_or_else(|e| {
        paths::log_error(&format_args!("hook: cannot list sessions: {e:#}"));
        Vec::new()
    })
}

fn live_sessions_in(s: &Seams<'_>, now: f64) -> Vec<Record> {
    let mut records: Vec<Record> = store::load_all_in(s.dir, now, s.journal)
        .into_iter()
        .map(|record| {
            let quiet = now - record.updated_at > QUIET_SECONDS;
            if INTERRUPTIBLE.contains(&record.status)
                && record.background == 0
                && quiet
                && record.transcript.as_deref().is_some_and(was_interrupted)
            {
                settle_interrupted_in(s, &record.id, now).unwrap_or(record)
            } else {
                record
            }
        })
        .collect();
    // Settling changes a status, and with it the order
    records.sort_by(|a, b| {
        let (a, b) = (sort_key(a), sort_key(b));
        a.0.cmp(&b.0).then(a.1.total_cmp(&b.1))
    });
    records
}

/// Writes an Esc interrupt to disk, unless a hook changed the session meanwhile.
///
/// Returns the record as it is afterwards, or `None` if there is none.
pub fn settle_interrupted(id: &str, now: f64) -> Option<Record> {
    with_seams(|s| settle_interrupted_in(s, id, now)).unwrap_or_else(|e| {
        paths::log_error(&format_args!("hook: cannot settle {id}: {e:#}"));
        None
    })
}

fn settle_interrupted_in(s: &Seams<'_>, id: &str, now: f64) -> Option<Record> {
    let _lock = store::lock_in(s.dir, id).ok()?;
    let current = store::read_in(s.dir, id)?;
    if !INTERRUPTIBLE.contains(&current.status) {
        return Some(current);
    }
    let settled = mark_interrupted(&current, now);
    if let Err(e) = store::write_in(s.dir, &settled, s.journal) {
        paths::log_error(&format_args!("hook: cannot settle {id}: {e}"));
    }
    Some(settled)
}

/// Marks the finished session in a tmux pane as seen on disk.
///
/// Pane ids repeat across tmux servers, so only the session of the server at `socket` counts.
pub fn seen(pane: &str, socket: &str, now: f64) {
    if let Err(e) = with_seams(|s| seen_in(s, pane, socket, now)) {
        paths::log_error(&format_args!("hook: cannot mark {pane} seen: {e:#}"));
    }
}

fn seen_in(s: &Seams<'_>, pane: &str, socket: &str, now: f64) {
    for r in live_sessions_in(s, now) {
        let here = r
            .mux
            .as_ref()
            .is_some_and(|m| m.pane == pane && m.socket == socket);
        if !here {
            continue;
        }
        let Ok(_lock) = store::lock_in(s.dir, &r.id) else {
            continue;
        };
        let Some(current) = store::read_in(s.dir, &r.id).filter(|c| c.status == Status::Done)
        else {
            continue;
        };
        if let Err(e) = store::write_in(s.dir, &mark_seen(&current, now), s.journal) {
            paths::log_error(&format_args!("hook: cannot mark {} seen: {e}", r.id));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::fs::{self, OpenOptions};
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::Duration;

    use rustix::fs::{flock, FlockOperation};
    use rustix::io::Errno;
    use tempfile::TempDir;

    use crate::claude::hook::{
        claude_pid, handle_event_in, live_sessions_in, parse_event, seen_in, settle_interrupted_in,
        Bind, Binder, Seams,
    };
    use crate::claude::install::HOOK_TIMEOUT;
    use crate::core::journal::JournalEvent;
    use crate::core::state::{new_record, Event, Mux, Pending, Record, Status, Waiting};
    use crate::core::store::{process_start, read_in, write_in};
    use crate::term::tmux::{Runner, TIMEOUT};

    fn no_journal(_: &JournalEvent) {}

    type Reply = Rc<dyn Fn(&[&str]) -> String>;
    type Calls = Rc<RefCell<Vec<String>>>;

    /// A tmux that records `<socket>: <args>` for every call and replays `reply`.
    struct Fake {
        socket: String,
        calls: Calls,
        reply: Reply,
    }

    impl Runner for Fake {
        fn run(&self, args: &[&str]) -> String {
            self.calls
                .borrow_mut()
                .push(format!("{}: {}", self.socket, args.join(" ")));
            (self.reply)(args)
        }
    }

    /// A records directory and a fake tmux.
    struct Rig {
        dir: TempDir,
        calls: Calls,
        bind: Box<Binder>,
    }

    impl Rig {
        fn new() -> Self {
            Self::replying(|_| Rc::new(|_: &[&str]| String::new()))
        }

        fn replying(reply: impl FnOnce(&Path) -> Reply) -> Self {
            let dir = TempDir::new().expect("create temp dir");
            let reply = reply(dir.path());
            let calls = Calls::default();
            let shared = Rc::clone(&calls);
            let bind = Box::new(move |socket: &str| -> Box<dyn Runner> {
                Box::new(Fake {
                    socket: socket.to_owned(),
                    calls: Rc::clone(&shared),
                    reply: Rc::clone(&reply),
                })
            });
            Rig { dir, calls, bind }
        }

        fn seams(&self) -> Seams<'_> {
            let bind: Bind<'_> = &*self.bind;
            Seams {
                dir: self.dir.path(),
                journal: &no_journal,
                bind,
            }
        }

        /// The runner for the server this process runs in.
        fn here(&self) -> Box<dyn Runner> {
            (self.bind)("here")
        }

        fn handle(&self, event: &Event, env: &HashMap<String, String>, now: f64) {
            handle_event_in(&self.seams(), event, env, now, &*self.here()).expect("handle event");
        }

        fn read(&self, id: &str) -> Option<Record> {
            read_in(self.dir.path(), id)
        }

        fn write(&self, r: &Record) {
            write_in(self.dir.path(), r, &no_journal).expect("write record");
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    fn me() -> i32 {
        i32::try_from(std::process::id()).expect("pid fits")
    }

    fn env(extra: &[(&str, &str)]) -> HashMap<String, String> {
        let mut env = HashMap::from([("CLAUDE_PID".to_owned(), me().to_string())]);
        env.extend(extra.iter().map(|&(k, v)| (k.to_owned(), v.to_owned())));
        env
    }

    fn event(id: &str, name: &str) -> Event {
        Event {
            session_id: id.to_owned(),
            hook_event_name: name.to_owned(),
            ..Event::default()
        }
    }

    fn live_record(id: &str) -> Record {
        Record {
            pid: Some(me()),
            pid_start: process_start(me()),
            ..new_record(id)
        }
    }

    fn write_transcript(path: &Path, lines: &[&str]) -> PathBuf {
        fs::write(
            path,
            lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
        )
        .expect("write transcript");
        path.to_owned()
    }

    const USER_ESC: &str =
        r#"{"type":"user","message":{"content":"[Request interrupted by user]"}}"#;

    fn lock_is_held(dir: &Path, id: &str) -> bool {
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(dir.join(format!("{id}.json.lock")))
            .expect("open lock file");
        matches!(
            flock(&file, FlockOperation::NonBlockingLockExclusive),
            Err(Errno::WOULDBLOCK)
        )
    }

    #[test]
    fn claude_pid_reads_digits_and_falls_back_to_the_parent() {
        let env = |pid: &str| HashMap::from([("CLAUDE_PID".to_owned(), pid.to_owned())]);
        assert_eq!(claude_pid(&env("4242")), 4242);
        let parent = i32::try_from(std::os::unix::process::parent_id()).expect("pid fits");
        for bad in ["", "abc", "-5", "12x", "99999999999999999999"] {
            assert_eq!(claude_pid(&env(bad)), parent, "{bad:?}");
        }
        assert_eq!(claude_pid(&HashMap::new()), parent);
    }

    #[test]
    fn hook_outside_tmux_writes_record() {
        let rig = Rig::new();
        let mut e = event("s1", "UserPromptSubmit");
        e.cwd = Some("/nonexistent".to_owned());
        rig.handle(&e, &env(&[]), 5.0);
        let r = rig.read("s1").expect("record");
        assert_eq!(
            (r.status, r.pid, r.mux),
            (Status::Working, Some(me()), None)
        );
        assert_eq!(r.pid_start, process_start(me()));
        assert!(rig.calls().is_empty());
    }

    #[test]
    fn a_headless_run_is_marked_and_takes_no_tab() {
        let rig = Rig::new();
        let tmux = [("TMUX", "/tmp/t/claude,1,0"), ("TMUX_PANE", "%3")];
        let headless = env(&[tmux[0], tmux[1], ("CLAUDE_CODE_ENTRYPOINT", "sdk-cli")]);
        let mut e = event("s1", "UserPromptSubmit");
        e.cwd = Some("/tmp/daily-recap".to_owned());
        rig.handle(&e, &headless, 5.0);
        let r = rig.read("s1").expect("record");
        assert_eq!((r.headless, r.mux, r.started_at), (true, None, 5.0));
        // Later events keep when it started
        e.hook_event_name = "PreToolUse".to_owned();
        rig.handle(&e, &headless, 9.0);
        assert_eq!(rig.read("s1").expect("record").started_at, 5.0);
        e.session_id = "s2".to_owned();
        e.hook_event_name = "UserPromptSubmit".to_owned();
        rig.handle(
            &e,
            &env(&[tmux[0], tmux[1], ("CLAUDE_CODE_ENTRYPOINT", "cli")]),
            5.0,
        );
        assert!(!rig.read("s2").expect("record").headless);
        // Only the run with a terminal asked tmux about its window
        let calls = rig.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(calls[0].starts_with("/tmp/t/claude: display-message -p -t %3 "));
    }

    #[test]
    fn hook_with_a_dead_tmux_socket_keeps_the_pane() {
        let rig = Rig::new();
        let mut e = event("s1", "UserPromptSubmit");
        e.cwd = Some("/w/notes".to_owned());
        rig.handle(
            &e,
            &env(&[("TMUX", "/nonexistent,1,0"), ("TMUX_PANE", "%1")]),
            5.0,
        );
        let r = rig.read("s1").expect("record");
        let mux = Mux {
            socket: "/nonexistent".to_owned(),
            pane: "%1".to_owned(),
        };
        assert_eq!((r.status, r.mux), (Status::Working, Some(mux)));
    }

    #[test]
    fn a_clear_or_resume_drops_the_session_it_replaced() {
        let rig = Rig::new();
        rig.handle(&event("before", "SessionStart"), &env(&[]), 1.0);
        rig.write(&Record {
            pid_start: Some(u64::MAX),
            ..live_record("other-process")
        });
        let mut start = event("after", "SessionStart");
        start.source = Some("resume".to_owned());
        rig.handle(&start, &env(&[]), 2.0);
        assert!(rig.read("before").is_none());
        assert!(rig.read("after").is_some());
        assert!(rig.read("other-process").is_some());
    }

    #[test]
    fn a_plain_start_drops_nothing() {
        let rig = Rig::new();
        rig.handle(&event("before", "SessionStart"), &env(&[]), 1.0);
        rig.handle(&event("after", "SessionStart"), &env(&[]), 2.0);
        assert!(rig.read("before").is_some() && rig.read("after").is_some());
    }

    #[test]
    fn hook_session_end_removes_the_record() {
        let rig = Rig::new();
        rig.handle(&event("s1", "SessionStart"), &env(&[]), 1.0);
        rig.handle(&event("s1", "SessionEnd"), &env(&[]), 2.0);
        assert!(rig.read("s1").is_none());
    }

    #[test]
    fn session_end_of_a_clear_or_resume_keeps_the_record() {
        // The new session may start first, and a resumed session has the same id
        let rig = Rig::new();
        rig.handle(&event("s1", "SessionStart"), &env(&[]), 1.0);
        for reason in ["clear", "resume"] {
            let mut end = event("s1", "SessionEnd");
            end.reason = Some(reason.to_owned());
            rig.handle(&end, &env(&[]), 2.0);
            assert!(rig.read("s1").is_some(), "{reason}");
        }
        let mut exit = event("s1", "SessionEnd");
        exit.reason = Some("prompt_input_exit".to_owned());
        rig.handle(&exit, &env(&[]), 3.0);
        assert!(rig.read("s1").is_none());
    }

    #[test]
    fn seen_ignores_the_same_pane_on_another_tmux_server() {
        let rig = Rig::new();
        for (id, socket) in [("mine", "/tmp/a"), ("other", "/tmp/b")] {
            rig.write(&Record {
                status: Status::Done,
                mux: Some(Mux {
                    socket: socket.to_owned(),
                    pane: "%3".to_owned(),
                }),
                ..live_record(id)
            });
        }
        seen_in(&rig.seams(), "%3", "/tmp/a", 0.0);
        assert_eq!(rig.read("mine").expect("record").status, Status::Idle);
        assert_eq!(rig.read("other").expect("record").status, Status::Done);
    }

    #[test]
    fn seen_leaves_a_session_that_is_not_done() {
        let rig = Rig::new();
        rig.write(&Record {
            status: Status::Working,
            updated_at: 100.0,
            mux: Some(Mux {
                socket: "/tmp/a".to_owned(),
                pane: "%3".to_owned(),
            }),
            ..live_record("s1")
        });
        seen_in(&rig.seams(), "%3", "/tmp/a", 101.0);
        assert_eq!(rig.read("s1").expect("record").status, Status::Working);
    }

    #[test]
    fn hook_names_the_window_while_it_holds_the_lock() {
        let held = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&held);
        let rig = Rig::replying(|dir| {
            let dir = dir.to_owned();
            Rc::new(move |_| {
                log.borrow_mut().push(lock_is_held(&dir, "s1"));
                "1\t\tclaude".to_owned()
            })
        });
        let tmux = env(&[("TMUX", "/tmp/t/claude,1,0"), ("TMUX_PANE", "%3")]);
        let mut e = event("s1", "UserPromptSubmit");
        e.cwd = Some("/w/notes".to_owned());
        rig.handle(&e, &tmux, 1.0);
        let during = held.borrow().len();
        // The window name is read, the rename and the option follow
        assert_eq!(during, 3);
        assert!(held.borrow().iter().all(|&h| h));
        // A session that ended names nothing
        rig.handle(&event("s1", "SessionEnd"), &tmux, 2.0);
        assert_eq!(held.borrow().len(), during);
    }

    #[test]
    fn tmux_calls_go_to_the_server_of_the_records_pane() {
        let rig = Rig::replying(|_| {
            Rc::new(|args: &[&str]| match args {
                ["display-message", "-p", "-t", _, fmt] if fmt.contains("session_attached") => {
                    "1".to_owned()
                }
                ["display-message", ..] => "1\t\tclaude".to_owned(),
                _ => String::new(),
            })
        });
        let tmux = env(&[("TMUX", "/tmp/t/claude,1,0"), ("TMUX_PANE", "%3")]);
        let mut stop = event("s1", "Stop");
        stop.cwd = Some("/w/notes".to_owned());
        rig.handle(&stop, &tmux, 1.0);
        let calls = rig.calls();
        assert!(!calls.is_empty());
        assert!(
            calls.iter().all(|c| c.starts_with("/tmp/t/claude: ")),
            "{calls:?}"
        );
        // The pane was on screen, so the finished session is seen at once
        assert_eq!(rig.read("s1").expect("record").status, Status::Idle);
    }

    #[test]
    fn the_tmux_calls_of_a_hook_fit_in_its_timeout() {
        // A background session's pane lookup (1), the viewed check (1) and the naming (3)
        assert!(5 * TIMEOUT < Duration::from_secs(HOOK_TIMEOUT));
    }

    #[test]
    fn interrupted_working_session_is_shown_idle() {
        let rig = Rig::new();
        let transcript = write_transcript(
            &rig.dir.path().join("t.jsonl"),
            &[
                r#"{"type":"user","message":{"content":"do it"}}"#,
                r#"{"type":"assistant","message":{"content":"ok"}}"#,
                USER_ESC,
            ],
        );
        rig.write(&Record {
            status: Status::Working,
            transcript: Some(transcript),
            ..live_record("s1")
        });
        let all = live_sessions_in(&rig.seams(), 100.0);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, Status::Idle);
        assert_eq!(rig.read("s1").expect("record").status, Status::Idle);
    }

    #[test]
    fn a_prompt_rejected_with_esc_is_settled() {
        let rig = Rig::new();
        let transcript = write_transcript(
            &rig.dir.path().join("t.jsonl"),
            &[
                r#"{"type":"user","message":{"content":"[Request interrupted by user for tool use]"}}"#,
            ],
        );
        rig.write(&Record {
            status: Status::Permission,
            pending: vec![Pending {
                agent: "main".to_owned(),
                kind: Waiting::Permission,
            }],
            transcript: Some(transcript),
            ..live_record("s1")
        });
        let all = live_sessions_in(&rig.seams(), 100.0);
        assert_eq!((all[0].status, all[0].pending.len()), (Status::Idle, 0));
    }

    #[test]
    fn recent_working_session_is_not_checked_for_interrupts() {
        let rig = Rig::new();
        let transcript = write_transcript(&rig.dir.path().join("t.jsonl"), &[USER_ESC]);
        rig.write(&Record {
            status: Status::Working,
            transcript: Some(transcript),
            updated_at: 99.0,
            ..live_record("s1")
        });
        assert_eq!(
            live_sessions_in(&rig.seams(), 100.0)[0].status,
            Status::Working
        );
    }

    #[test]
    fn a_background_session_is_not_checked_for_interrupts() {
        let rig = Rig::new();
        let transcript = write_transcript(&rig.dir.path().join("t.jsonl"), &[USER_ESC]);
        rig.write(&Record {
            status: Status::Working,
            transcript: Some(transcript),
            background: 2,
            ..live_record("s1")
        });
        assert_eq!(
            live_sessions_in(&rig.seams(), 100.0)[0].status,
            Status::Working
        );
    }

    #[test]
    fn live_sessions_are_ordered_after_settling() {
        let rig = Rig::new();
        let transcript = write_transcript(&rig.dir.path().join("t.jsonl"), &[USER_ESC]);
        // Working sorts before idle until the interrupt settles it
        rig.write(&Record {
            status: Status::Working,
            transcript: Some(transcript),
            ..live_record("a")
        });
        rig.write(&Record {
            status: Status::Done,
            status_since: 5.0,
            ..live_record("b")
        });
        let ids: Vec<String> = live_sessions_in(&rig.seams(), 100.0)
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, ["b", "a"]);
    }

    #[test]
    fn settle_interrupted_leaves_a_session_a_hook_already_changed() {
        let rig = Rig::new();
        rig.write(&Record {
            status: Status::Done,
            ..live_record("s1")
        });
        let r = settle_interrupted_in(&rig.seams(), "s1", 100.0).expect("record");
        assert_eq!(r.status, Status::Done);
        assert!(settle_interrupted_in(&rig.seams(), "missing", 100.0).is_none());
    }

    #[test]
    fn hook_names_the_window_after_the_sessions_title() {
        let rig = Rig::replying(|_| Rc::new(|_| "1\t\tclaude".to_owned()));
        let dir = TempDir::new().expect("temp dir");
        let transcript = dir.path().join("s1.jsonl");
        fs::write(
            &transcript,
            "{\"type\":\"ai-title\",\"aiTitle\":\"Fix the login bug\"}\n",
        )
        .expect("write transcript");
        let tmux = env(&[("TMUX", "/tmp/t/claude,1,0"), ("TMUX_PANE", "%3")]);
        let mut e = event("s1", "Stop");
        e.cwd = Some("/w/notes".to_owned());
        e.transcript_path = Some(transcript.to_string_lossy().into_owned());
        rig.handle(&e, &tmux, 1.0);
        assert!(rig
            .calls()
            .iter()
            .any(|c| c.contains("rename-window -t %3 Fix the login bug")));
        // A title found once stays, even after it scrolls out of the transcript's tail
        fs::write(&transcript, "{\"type\":\"user\"}\n").expect("write transcript");
        e.hook_event_name = "UserPromptSubmit".to_owned();
        rig.handle(&e, &tmux, 2.0);
        assert_eq!(rig.read("s1").expect("record").title, "Fix the login bug");
    }

    #[test]
    fn only_naming_events_look_for_a_title() {
        let rig = Rig::new();
        let dir = TempDir::new().expect("temp dir");
        let transcript = dir.path().join("s1.jsonl");
        fs::write(
            &transcript,
            "{\"type\":\"ai-title\",\"aiTitle\":\"A title\"}\n",
        )
        .expect("write transcript");
        let mut e = event("s1", "PreToolUse");
        e.transcript_path = Some(transcript.to_string_lossy().into_owned());
        rig.handle(&e, &env(&[]), 1.0);
        assert_eq!(rig.read("s1").expect("record").title, "");
    }

    #[test]
    fn parse_event_accepts_a_hook_input() {
        let e = parse_event(br#"{"session_id":"s-1_a","hook_event_name":"Stop","extra":1}"#)
            .expect("valid");
        assert_eq!(
            (e.session_id.as_str(), e.hook_event_name.as_str()),
            ("s-1_a", "Stop")
        );
        let e = parse_event(br#"{"session_id":"s","hook_event_name":"Stop","background_tasks":5}"#)
            .expect("lenient tasks");
        assert!(e.background_tasks.is_empty());
    }

    #[test]
    fn parse_event_rejects_malformed_input() {
        let bad: [&[u8]; 11] = [
            b"",
            b"nope",
            b"{}",
            b"[]",
            b"null",
            br#"{"session_id":null,"hook_event_name":"Stop"}"#,
            br#"{"session_id":"s","hook_event_name":null}"#,
            br#"{"session_id":"s"}"#,
            br#"{"hook_event_name":"Stop"}"#,
            br#"{"session_id":"../x","hook_event_name":"Stop"}"#,
            br#"{"session_id":"s","hook_event_name":""}"#,
        ];
        for input in bad {
            assert!(
                parse_event(input).is_err(),
                "{}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn a_bogus_event_name_still_records_the_session() {
        let rig = Rig::new();
        rig.handle(&event("s", "Bogus"), &env(&[]), 1.0);
        assert!(rig.read("s").is_some());
    }
}
