//! Session state machine driven by Claude Code hook events.
//!
//! Pure: no I/O. A [`Record`] is plain data that round-trips through JSON unchanged.

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// What a session is doing, in display order: what needs the user first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Blocked on an approval.
    Permission,
    /// Blocked on an answer to a question or a plan.
    Input,
    /// The turn is over.
    Done,
    /// A turn is running.
    Working,
    /// The turn ended with only shell commands left running in the background.
    Shell,
    /// Nothing is happening.
    #[default]
    Idle,
    /// A status this version does not know. Only ever read from disk, never produced by
    /// [`apply_event`]; it sorts after every known status.
    #[serde(other)]
    Unknown,
}

/// Blocked on the user: an approval, or an answer.
pub const NEEDS_YOU: [Status; 2] = [Status::Permission, Status::Input];
/// Statuses that show how long the session has been in them.
pub const TIMED: [Status; 3] = [Status::Permission, Status::Input, Status::Shell];
/// Statuses of a turn that Esc can interrupt.
pub const INTERRUPTIBLE: [Status; 3] = [Status::Working, Status::Permission, Status::Input];

impl Status {
    /// The name this status has on disk and in the journal.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Permission => "permission",
            Status::Input => "input",
            Status::Done => "done",
            Status::Working => "working",
            Status::Shell => "shell",
            Status::Idle => "idle",
            Status::Unknown => "unknown",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Status::Permission => 0,
            Status::Input => 1,
            Status::Done => 2,
            Status::Working => 3,
            Status::Shell => 4,
            Status::Idle => 5,
            Status::Unknown => 6,
        }
    }
}

/// What an agent that waits on the user waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Waiting {
    /// An approval.
    Permission,
    /// An answer.
    Input,
}

/// An agent waiting on the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// `"main"`, a subagent id, or `"notification"`.
    pub agent: String,
    /// What it waits for.
    pub kind: Waiting,
}

/// Where the session's terminal lives in the multiplexer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mux {
    /// The tmux socket name.
    pub socket: String,
    /// The tmux pane id.
    pub pane: String,
}

/// Usage numbers the status line reports.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// Cost so far in dollars.
    pub cost: Option<f64>,
    /// Seconds spent in API calls.
    pub api_duration: Option<f64>,
    /// The model's display name.
    pub model: Option<String>,
    /// Lines added.
    pub added: u64,
    /// Lines removed.
    pub removed: u64,
}

/// One session, as stored on disk.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// The session id.
    pub id: String,
    /// Current status.
    pub status: Status,
    /// Working directory.
    pub cwd: String,
    /// Process id of Claude Code.
    pub pid: Option<i32>,
    /// Start time of that process, to tell a reused pid apart.
    pub pid_start: Option<u64>,
    /// Path to the transcript file.
    pub transcript: Option<PathBuf>,
    /// Background tasks still running at the last stop.
    pub background: u32,
    /// Agents waiting on the user.
    pub pending: Vec<Pending>,
    /// When the session entered its current status.
    pub status_since: f64,
    /// When the last hook arrived.
    pub updated_at: f64,
    /// When the session was first seen.
    pub started_at: f64,
    /// Where its terminal lives, when known.
    pub mux: Option<Mux>,
    /// Whether it runs without a terminal (`claude -p`).
    pub headless: bool,
    /// The session title.
    pub title: String,
    /// Latest usage numbers.
    pub usage: Option<Usage>,
}

/// A background task listed in a `Stop` event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackgroundTask {
    /// Task status, such as `running` or `completed`.
    pub status: Option<String>,
    /// Task kind (`type` in the JSON), such as `shell` or `subagent`.
    pub kind: Option<String>,
}

fn opt_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn lenient_tasks<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<BackgroundTask>, D::Error> {
    let Value::Array(items) = Value::deserialize(de)? else {
        return Ok(Vec::new());
    };
    Ok(items
        .iter()
        .filter_map(Value::as_object)
        .map(|task| BackgroundTask {
            status: opt_string(task.get("status")),
            kind: opt_string(task.get("type")),
        })
        .collect())
}

fn lenient_string<'de, D: Deserializer<'de>>(de: D) -> Result<Option<String>, D::Error> {
    Ok(opt_string(Some(&Value::deserialize(de)?)))
}

/// A hook input as Claude Code sends it. Fields of the wrong type read as absent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Event {
    /// The session id.
    pub session_id: String,
    /// The hook event name, such as `PreToolUse`.
    pub hook_event_name: String,
    /// The subagent the event belongs to; absent for the main agent.
    #[serde(deserialize_with = "lenient_string")]
    pub agent_id: Option<String>,
    /// The tool involved.
    #[serde(deserialize_with = "lenient_string")]
    pub tool_name: Option<String>,
    /// Working directory.
    #[serde(deserialize_with = "lenient_string")]
    pub cwd: Option<String>,
    /// Path to the transcript file.
    #[serde(deserialize_with = "lenient_string")]
    pub transcript_path: Option<String>,
    /// Why a `SessionStart` happened, such as `compact`.
    #[serde(deserialize_with = "lenient_string")]
    pub source: Option<String>,
    /// Why a `SessionEnd` happened.
    #[serde(deserialize_with = "lenient_string")]
    pub reason: Option<String>,
    /// The kind of a `Notification`.
    #[serde(deserialize_with = "lenient_string")]
    pub notification_type: Option<String>,
    /// Background tasks of a `Stop`; anything not a list is empty.
    #[serde(deserialize_with = "lenient_tasks")]
    pub background_tasks: Vec<BackgroundTask>,
}

/// Tools that stop and wait for the user: a question, a plan to approve.
const ASKING_TOOLS: [&str; 2] = ["AskUserQuestion", "ExitPlanMode"];
const FINISHED_TASKS: [&str; 5] = ["completed", "failed", "killed", "stopped", "cancelled"];
const SHELL_TASK: &str = "shell";
const MAIN_AGENT: &str = "main";
/// Stands for a prompt known only from a notification, not from a hook of its own.
const NOTIFIED: &str = "notification";

/// Returns an empty record for a session seen for the first time.
pub fn new_record(id: &str) -> Record {
    Record {
        id: id.to_owned(),
        ..Record::default()
    }
}

fn wait(pending: &mut Vec<Pending>, agent: &str, kind: Waiting) {
    if pending.iter().all(|p| p.agent != agent) {
        pending.push(Pending {
            agent: agent.to_owned(),
            kind,
        });
    }
}

fn done_waiting(pending: &mut Vec<Pending>, agent: &str) {
    pending.retain(|p| p.agent != agent);
}

/// Advances a session record by one hook event.
///
/// `record` is `None` for an unknown session. The input record is not modified.
pub fn apply_event(record: Option<&Record>, event: &Event, now: f64) -> Record {
    let mut r = record
        .cloned()
        .unwrap_or_else(|| new_record(&event.session_id));
    // The agents (the main one, or a subagent) waiting on the user, each with what it waits
    // for. Hooks of parallel tools and subagents interleave, so one agent's tool call must not
    // hide another agent's open prompt.
    let mut pending = std::mem::take(&mut r.pending);
    let agent = event
        .agent_id
        .as_deref()
        .filter(|a| !a.is_empty())
        .unwrap_or(MAIN_AGENT);

    if record.is_none() {
        r.started_at = now;
    }
    if let Some(cwd) = event.cwd.as_deref().filter(|c| !c.is_empty()) {
        r.cwd = cwd.to_owned();
    }
    if let Some(path) = event.transcript_path.as_deref().filter(|p| !p.is_empty()) {
        r.transcript = Some(PathBuf::from(path));
    }

    match event.hook_event_name.as_str() {
        "SessionStart" => {
            // A compaction restarts the context in the middle of a turn: nothing else changes
            if event.source.as_deref() != Some("compact") {
                pending.clear();
                r.status = Status::Idle;
                r.background = 0;
            }
        }
        "UserPromptSubmit" => {
            // The next Stop counts the background tasks again; until then the turn can be
            // interrupted
            pending.clear();
            r.status = Status::Working;
            r.background = 0;
        }
        "PreToolUse" => {
            done_waiting(&mut pending, agent);
            if event
                .tool_name
                .as_deref()
                .is_some_and(|t| ASKING_TOOLS.contains(&t))
            {
                wait(&mut pending, agent, Waiting::Input);
            }
            r.status = Status::Working;
        }
        "PermissionRequest" => wait(&mut pending, agent, Waiting::Permission),
        "PostToolUse" | "PostToolUseFailure" => {
            done_waiting(&mut pending, agent);
            done_waiting(&mut pending, NOTIFIED);
            r.status = Status::Working;
        }
        "Notification" => {
            // Usually echoes a prompt already pending; it only counts when nothing else
            // explains it
            let kind = match event.notification_type.as_deref() {
                Some("permission_prompt") => Some(Waiting::Permission),
                Some("elicitation_dialog") => Some(Waiting::Input),
                _ => None,
            };
            if let (Some(kind), true) = (kind, pending.is_empty()) {
                wait(&mut pending, NOTIFIED, kind);
            }
        }
        "SubagentStop" => done_waiting(&mut pending, agent),
        "Stop" => {
            pending.clear();
            let running: Vec<&BackgroundTask> = event
                .background_tasks
                .iter()
                .filter(|t| {
                    t.status
                        .as_deref()
                        .is_none_or(|s| !FINISHED_TASKS.contains(&s))
                })
                .collect();
            r.background = u32::try_from(running.len()).unwrap_or(u32::MAX);
            r.status = if running.is_empty() {
                Status::Done
            } else if running
                .iter()
                .all(|t| t.kind.as_deref() == Some(SHELL_TASK))
            {
                Status::Shell
            } else {
                Status::Working
            };
        }
        "StopFailure" => {
            // A failed turn is over too
            pending.clear();
            r.status = Status::Done;
        }
        _ => {}
    }

    if !pending.is_empty() {
        r.status = if pending.iter().any(|p| p.kind == Waiting::Permission) {
            Status::Permission
        } else {
            Status::Input
        };
    } else if NEEDS_YOU.contains(&r.status) {
        // The last open prompt went away (a subagent stopped) and the turn goes on
        r.status = Status::Working;
    }
    r.pending = pending;
    r.updated_at = now;
    // Hooks keep coming while a session waits (notifications, subagents), so how long it has
    // been in its status is kept apart from when it last changed
    if record.is_none_or(|old| old.status != r.status) || r.status_since == 0.0 {
        r.status_since = now;
    }
    r
}

/// Settles a finished session the user looked at as idle; callers check it is finished.
pub fn mark_seen(r: &Record, now: f64) -> Record {
    Record {
        status: Status::Idle,
        status_since: now,
        ..r.clone()
    }
}

/// Settles a session the user stopped with Esc, which fires no hook.
pub fn mark_interrupted(r: &Record, now: f64) -> Record {
    Record {
        status: Status::Idle,
        pending: Vec::new(),
        background: 0,
        status_since: now,
        ..r.clone()
    }
}

/// The project folder's name, which names the session everywhere; `?` when there is none.
pub fn project(r: &Record) -> &str {
    let name = r
        .cwd
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    if name.is_empty() {
        "?"
    } else {
        name
    }
}

/// When the session entered its current status.
pub fn since(r: &Record) -> f64 {
    if r.status_since != 0.0 {
        r.status_since
    } else {
        r.updated_at
    }
}

/// Orders sessions by status, then the ones in that status longest first.
pub fn sort_key(r: &Record) -> (u8, f64) {
    (r.status.rank(), since(r))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use crate::state::{
        apply_event, mark_interrupted, mark_seen, new_record, project, since, sort_key,
        BackgroundTask, Event, Pending, Record, Status, Waiting, INTERRUPTIBLE, NEEDS_YOU, TIMED,
    };

    const NOW: f64 = 1000.0;

    #[test]
    fn status_names_match_their_serialized_form() {
        let all = [
            Status::Permission,
            Status::Input,
            Status::Done,
            Status::Working,
            Status::Shell,
            Status::Idle,
        ];
        for status in all {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{}\"", status.as_str())
            );
        }
        assert_eq!(Status::Unknown.as_str(), "unknown");
    }

    fn event(name: &str) -> Event {
        Event {
            session_id: "s1".into(),
            hook_event_name: name.into(),
            cwd: Some("/w/k265".into()),
            ..Event::default()
        }
    }

    fn tool(name: &str, tool_name: &str) -> Event {
        Event {
            tool_name: Some(tool_name.into()),
            ..event(name)
        }
    }

    fn by(agent: &str, e: Event) -> Event {
        Event {
            agent_id: Some(agent.into()),
            ..e
        }
    }

    fn notification(kind: &str) -> Event {
        Event {
            notification_type: Some(kind.into()),
            ..event("Notification")
        }
    }

    fn stop(tasks: &[(&str, &str)]) -> Event {
        let background_tasks = tasks
            .iter()
            .map(|(kind, status)| BackgroundTask {
                status: Some((*status).into()),
                kind: (!kind.is_empty()).then(|| (*kind).into()),
            })
            .collect();
        Event {
            background_tasks,
            ..event("Stop")
        }
    }

    fn run(events: &[Event], record: Option<Record>) -> Record {
        let mut record = record;
        for e in events {
            record = Some(apply_event(record.as_ref(), e, NOW));
        }
        record.expect("at least one event")
    }

    fn pending(agent: &str, kind: Waiting) -> Pending {
        Pending {
            agent: agent.into(),
            kind,
        }
    }

    #[test]
    fn new_session_is_idle() {
        let r = run(
            &[Event {
                source: Some("startup".into()),
                ..event("SessionStart")
            }],
            None,
        );
        assert_eq!(
            (r.id.as_str(), r.status, r.cwd.as_str()),
            ("s1", Status::Idle, "/w/k265")
        );
        assert_eq!(r.started_at, NOW);
    }

    #[test]
    fn a_prompt_or_a_tool_means_working() {
        assert_eq!(
            run(&[event("UserPromptSubmit")], None).status,
            Status::Working
        );
        assert_eq!(
            run(&[tool("PreToolUse", "Edit")], None).status,
            Status::Working
        );
    }

    #[test]
    fn permission_request_needs_permission_until_the_tool_runs() {
        let r = run(
            &[event("UserPromptSubmit"), tool("PermissionRequest", "Bash")],
            None,
        );
        assert_eq!(r.status, Status::Permission);
        let r = run(&[tool("PostToolUse", "Bash")], Some(r));
        assert_eq!(r.status, Status::Working);
    }

    #[test]
    fn questions_and_plans_need_input() {
        for name in ["AskUserQuestion", "ExitPlanMode"] {
            let asked = run(&[tool("PreToolUse", name)], None);
            assert_eq!(asked.status, Status::Input);
            // Their permission request neither counts twice nor turns them into a permission
            let again = run(&[tool("PermissionRequest", name)], Some(asked));
            assert_eq!(again.status, Status::Input);
            assert_eq!(again.pending, vec![pending("main", Waiting::Input)]);
        }
    }

    #[test]
    fn a_permission_outranks_a_question() {
        let asked = run(&[tool("PreToolUse", "AskUserQuestion")], None);
        let both = run(&[by("a1", tool("PermissionRequest", "Bash"))], Some(asked));
        assert_eq!(both.status, Status::Permission);
        let after = run(&[by("a1", tool("PostToolUse", "Bash"))], Some(both));
        assert_eq!(after.status, Status::Input);
    }

    #[test]
    fn notifications() {
        let working = run(&[event("UserPromptSubmit")], None);
        let asked = run(&[notification("permission_prompt")], Some(working.clone()));
        assert_eq!(asked.status, Status::Permission);
        let elicited = run(&[notification("elicitation_dialog")], Some(working.clone()));
        assert_eq!(elicited.status, Status::Input);
        let idle = run(&[notification("idle_prompt")], Some(working));
        assert_eq!(idle.status, Status::Working);
    }

    #[test]
    fn another_agents_tool_use_keeps_a_pending_permission() {
        let asked = run(&[by("a1", tool("PermissionRequest", "Bash"))], None);
        for other in [
            tool("PreToolUse", "Read"),
            by("a2", tool("PostToolUse", "Grep")),
        ] {
            assert_eq!(
                run(&[other], Some(asked.clone())).status,
                Status::Permission
            );
        }
        let r = run(
            &[by("a1", tool("PostToolUse", "Bash"))],
            Some(asked.clone()),
        );
        assert_eq!(r.status, Status::Working);
        let r = run(&[by("a1", event("SubagentStop"))], Some(asked));
        assert_eq!(r.status, Status::Working);
    }

    #[test]
    fn a_notification_already_explained_by_a_prompt_changes_nothing() {
        let asked = run(&[tool("PermissionRequest", "Bash")], None);
        let r = run(&[notification("permission_prompt")], Some(asked.clone()));
        assert_eq!(r.pending, asked.pending);
    }

    #[test]
    fn compaction_keeps_the_turn_going() {
        let working = run(&[event("UserPromptSubmit")], None);
        let compact = Event {
            source: Some("compact".into()),
            ..event("SessionStart")
        };
        assert_eq!(
            run(&[compact], Some(working.clone())).status,
            Status::Working
        );
        let clear = Event {
            source: Some("clear".into()),
            ..event("SessionStart")
        };
        assert_eq!(run(&[clear], Some(working)).status, Status::Idle);
    }

    #[test]
    fn stop_finishes_the_turn() {
        let r = run(&[event("UserPromptSubmit"), stop(&[])], None);
        assert_eq!(r.status, Status::Done);
        assert_eq!(run(&[event("StopFailure")], None).status, Status::Done);
    }

    #[test]
    fn stop_with_background_tasks_keeps_working() {
        let r = run(
            &[stop(&[("", "running"), ("", "completed"), ("", "pending")])],
            None,
        );
        assert_eq!((r.status, r.background), (Status::Working, 2));
    }

    #[test]
    fn stop_with_only_shell_commands_running_waits_on_the_shell() {
        let tasks = [("shell", "running"), ("subagent", "completed")];
        let r = run(&[event("UserPromptSubmit"), stop(&tasks)], None);
        assert_eq!((r.status, r.background), (Status::Shell, 1));
        // The command finishes and wakes Claude, which works again
        let woke = run(&[tool("PreToolUse", "Read")], Some(r.clone()));
        assert_eq!(woke.status, Status::Working);
        // A prompt while the build runs starts a turn that Esc can interrupt
        let again = run(&[event("UserPromptSubmit")], Some(r));
        assert_eq!((again.status, again.background), (Status::Working, 0));
    }

    #[test]
    fn stop_with_a_subagent_running_keeps_working() {
        let tasks = [("shell", "running"), ("subagent", "running")];
        assert_eq!(run(&[stop(&tasks)], None).status, Status::Working);
    }

    #[test]
    fn seen_settles_a_finished_session_as_idle() {
        let seen = mark_seen(&run(&[event("Stop")], None), NOW + 5.0);
        assert_eq!((seen.status, seen.status_since), (Status::Idle, NOW + 5.0));
    }

    #[test]
    fn interrupted_settles_a_session_as_idle_and_clears_its_waits() {
        let asked = run(&[tool("PermissionRequest", "Bash")], None);
        let r = mark_interrupted(&asked, NOW + 5.0);
        assert_eq!(
            (r.status, r.status_since, r.background),
            (Status::Idle, NOW + 5.0, 0)
        );
        assert!(r.pending.is_empty());
    }

    #[test]
    fn apply_event_does_not_modify_its_input() {
        let r = run(&[event("SessionStart")], None);
        let before = r.clone();
        run(&[event("UserPromptSubmit")], Some(r.clone()));
        assert_eq!(r, before);
    }

    #[test]
    fn sort_puts_what_needs_you_first_and_oldest_first() {
        let rec = |status, updated_at| Record {
            status,
            updated_at,
            ..Record::default()
        };
        let a = rec(Status::Working, 1.0);
        let b = rec(Status::Permission, 9.0);
        let c = rec(Status::Permission, 2.0);
        let d = rec(Status::Input, 0.0);
        let mut all = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        all.sort_by(|x, y| sort_key(x).partial_cmp(&sort_key(y)).unwrap());
        assert_eq!(all, vec![c, b, d, a.clone()]);
        // An unknown status sorts last instead of failing
        let old = rec(Status::Unknown, 0.0);
        let idle = rec(Status::Idle, 99.0);
        let mut all = vec![old.clone(), idle.clone(), a.clone()];
        all.sort_by(|x, y| sort_key(x).partial_cmp(&sort_key(y)).unwrap());
        assert_eq!(all, vec![a, idle, old]);
    }

    #[test]
    fn status_since_survives_hooks_that_keep_the_status() {
        let asked = apply_event(None, &tool("PermissionRequest", "Bash"), NOW);
        let later = apply_event(Some(&asked), &notification("permission_prompt"), NOW + 30.0);
        assert_eq!((later.status_since, later.updated_at), (NOW, NOW + 30.0));
        let working = apply_event(Some(&later), &tool("PostToolUse", "Bash"), NOW + 40.0);
        assert_eq!(working.status_since, NOW + 40.0);
    }

    #[test]
    fn project_is_the_folder_name() {
        let at = |cwd: &str| Record {
            cwd: cwd.into(),
            ..Record::default()
        };
        assert_eq!(project(&at("/w/notes/")), "notes");
        assert_eq!(project(&at("/w/notes///")), "notes");
        assert_eq!(project(&at("/")), "?");
        assert_eq!(project(&Record::default()), "?");
    }

    #[test]
    fn since_falls_back_to_updated_at() {
        let r = Record {
            updated_at: 7.0,
            ..Record::default()
        };
        assert_eq!(since(&r), 7.0);
        let r = Record {
            status_since: 3.0,
            ..r
        };
        assert_eq!(since(&r), 3.0);
    }

    #[test]
    fn new_record_is_empty_and_idle() {
        let r = new_record("abc");
        assert_eq!((r.id.as_str(), r.status), ("abc", Status::Idle));
        assert_eq!((r.status_since, r.updated_at, r.background), (0.0, 0.0, 0));
        assert!(r.pending.is_empty() && r.mux.is_none() && r.usage.is_none());
    }

    #[test]
    fn status_groups() {
        assert_eq!(NEEDS_YOU, [Status::Permission, Status::Input]);
        assert_eq!(TIMED, [Status::Permission, Status::Input, Status::Shell]);
        assert_eq!(
            INTERRUPTIBLE,
            [Status::Working, Status::Permission, Status::Input]
        );
    }

    #[test]
    fn unknown_status_loads_and_serializes_harmlessly() {
        let r: Record = serde_json::from_str(r#"{"id":"s","status":"error"}"#).unwrap();
        assert_eq!(r.status, Status::Unknown);
        assert_eq!(serde_json::to_value(r.status).unwrap(), "unknown");
        let back: Record = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn record_round_trips_with_pending() {
        let r = run(&[tool("PermissionRequest", "Bash")], None);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""status":"permission""#));
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), r);
    }

    #[test]
    fn record_loads_with_missing_fields() {
        let r: Record = serde_json::from_str(r#"{"id":"s","cwd":"/a"}"#).unwrap();
        assert_eq!(
            (r.status, r.cwd.as_str(), r.started_at),
            (Status::Idle, "/a", 0.0)
        );
    }

    #[test]
    fn event_background_tasks_are_lenient() {
        let parse = |json: &str| {
            serde_json::from_str::<Event>(json)
                .unwrap()
                .background_tasks
        };
        assert!(parse(r#"{"background_tasks":"nope"}"#).is_empty());
        assert!(parse(r#"{"background_tasks":null}"#).is_empty());
        assert!(parse(r#"{"background_tasks":{"status":"running"}}"#).is_empty());
        assert!(parse(r#"{}"#).is_empty());
        let tasks =
            parse(r#"{"background_tasks":[1,"x",null,{"type":"shell","status":"running"},{}]}"#);
        assert_eq!(
            tasks,
            vec![
                BackgroundTask {
                    status: Some("running".into()),
                    kind: Some("shell".into())
                },
                BackgroundTask::default(),
            ]
        );
    }

    #[test]
    fn event_fields_of_the_wrong_type_read_as_absent() {
        let e: Event =
            serde_json::from_str(r#"{"session_id":"s","agent_id":5,"tool_name":null,"cwd":"/x"}"#)
                .unwrap();
        assert_eq!(
            (e.agent_id, e.tool_name, e.cwd),
            (None, None, Some("/x".into()))
        );
    }

    const NAMES: [&str; 11] = [
        "SessionStart",
        "SessionEnd",
        "UserPromptSubmit",
        "PreToolUse",
        "PermissionRequest",
        "PostToolUse",
        "PostToolUseFailure",
        "Notification",
        "SubagentStop",
        "Stop",
        "StopFailure",
    ];

    fn arb_task() -> impl Strategy<Value = BackgroundTask> {
        (
            prop::option::of(prop::sample::select(vec![
                "running",
                "pending",
                "completed",
                "killed",
            ])),
            prop::option::of(prop::sample::select(vec!["shell", "subagent"])),
        )
            .prop_map(|(status, kind)| BackgroundTask {
                status: status.map(Into::into),
                kind: kind.map(Into::into),
            })
    }

    fn arb_event() -> impl Strategy<Value = Event> {
        (
            prop::sample::select(NAMES.to_vec()),
            prop::option::of(prop::sample::select(vec!["a1", "a2"])),
            prop::option::of(prop::sample::select(vec![
                "Bash",
                "AskUserQuestion",
                "ExitPlanMode",
            ])),
            prop::option::of(prop::sample::select(vec![
                "permission_prompt",
                "elicitation_dialog",
                "idle_prompt",
            ])),
            prop::collection::vec(arb_task(), 0..4),
        )
            .prop_map(|(name, agent, tool, notification, tasks)| Event {
                session_id: "s1".into(),
                hook_event_name: name.into(),
                agent_id: agent.map(Into::into),
                tool_name: tool.map(Into::into),
                notification_type: notification.map(Into::into),
                background_tasks: tasks,
                ..Event::default()
            })
    }

    proptest! {
        #[test]
        fn invariants_hold_for_any_event_sequence(
            events in prop::collection::vec(arb_event(), 1..=40)
        ) {
            let mut record: Option<Record> = None;
            for (i, e) in events.iter().enumerate() {
                let now = 100.0 + i as f64;
                let next = apply_event(record.as_ref(), e, now);
                let mut agents: Vec<&str> = next.pending.iter().map(|p| p.agent.as_str()).collect();
                agents.sort_unstable();
                let count = agents.len();
                agents.dedup();
                prop_assert_eq!(agents.len(), count, "an agent is pending twice");
                let any_permission = next.pending.iter().any(|p| p.kind == Waiting::Permission);
                prop_assert_eq!(next.status == Status::Permission, any_permission);
                prop_assert_eq!(
                    next.status == Status::Input,
                    !next.pending.is_empty() && !any_permission
                );
                let changed = record.as_ref().is_none_or(|r| r.status != next.status);
                let moved = record.as_ref().is_none_or(|r| r.status_since != next.status_since);
                prop_assert_eq!(moved, changed);
                prop_assert_eq!(next.updated_at, now);
                prop_assert_ne!(next.status, Status::Unknown);
                record = Some(next);
            }
        }
    }
}
