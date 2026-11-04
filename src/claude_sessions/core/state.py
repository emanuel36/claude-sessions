"""Session state machine driven by Claude Code hook events.

Pure: no I/O. A session record is a plain dict so it round-trips through JSON unchanged.
"""

PERMISSION = "permission"
INPUT = "input"
DONE = "done"
WORKING = "working"
# The turn ended with only shell commands left running in the background, such as a build
SHELL = "shell"
IDLE = "idle"

# Display order: what needs the user first
STATUSES = (PERMISSION, INPUT, DONE, WORKING, SHELL, IDLE)
# Blocked on the user: an approval, or an answer
NEEDS_YOU = (PERMISSION, INPUT)
# Statuses that show how long the session has been in them
TIMED = (*NEEDS_YOU, SHELL)

# Tools that stop and wait for the user: a question, a plan to approve
ASKING_TOOLS = {"AskUserQuestion", "ExitPlanMode"}
NOTIFICATIONS = {"permission_prompt": PERMISSION, "elicitation_dialog": INPUT}
FINISHED_TASKS = {"completed", "failed", "killed", "stopped", "cancelled"}
SHELL_TASK = "shell"
MAIN_AGENT = "main"
# Stands for a prompt known only from a notification, not from a hook of its own
NOTIFIED = "notification"


def running_tasks(tasks):
    """Returns the background tasks that have not finished."""
    if not isinstance(tasks, list):
        return []
    return [t for t in tasks if isinstance(t, dict) and t.get("status") not in FINISHED_TASKS]


def new_record(session_id):
    """Returns an empty record for a session seen for the first time."""
    return {
        "id": session_id,
        "status": IDLE,
        "cwd": "",
        "pid": None,
        "pid_start": None,
        "transcript": None,
        "background": 0,
        "pending": [],
        "status_since": 0.0,
        "updated_at": 0.0,
        "mux": {},
    }


def apply_event(record, event, now):
    """Advances a session record by one hook event.

    Args:
        record: The current record, or None for an unknown session.
        event: The hook input as Claude Code sends it.
        now: Current time as a Unix timestamp.

    Returns:
        A new record; the input record is not modified.
    """
    r = dict(record or new_record(event.get("session_id")))
    # The agents (the main one, or a subagent) waiting on the user, each with what it waits
    # for. Hooks of parallel tools and subagents interleave, so one agent's tool call must not
    # hide another agent's open prompt.
    pending = [list(p) for p in r.get("pending") or []]
    name = event.get("hook_event_name")
    agent = event.get("agent_id") or MAIN_AGENT

    def wait(who, kind):
        if all(w != who for w, _ in pending):
            pending.append([who, kind])

    def done_waiting(who):
        pending[:] = [p for p in pending if p[0] != who]

    if record is None:
        r["started_at"] = now
    if event.get("cwd"):
        r["cwd"] = event["cwd"]
    if event.get("transcript_path"):
        r["transcript"] = event["transcript_path"]

    if name == "SessionStart":
        # A compaction restarts the context in the middle of a turn: nothing else changes
        if event.get("source") != "compact":
            pending.clear()
            r.update(status=IDLE, background=0)
    elif name == "UserPromptSubmit":
        # The next Stop counts the background tasks again; until then the turn can be interrupted
        pending.clear()
        r.update(status=WORKING, background=0)
    elif name == "PreToolUse":
        done_waiting(agent)
        if event.get("tool_name") in ASKING_TOOLS:
            wait(agent, INPUT)
        r["status"] = WORKING
    elif name == "PermissionRequest":
        wait(agent, PERMISSION)
    elif name in ("PostToolUse", "PostToolUseFailure"):
        done_waiting(agent)
        done_waiting(NOTIFIED)
        r["status"] = WORKING
    elif name == "Notification":
        # Usually echoes a prompt already pending; it only counts when nothing else explains it
        kind = NOTIFICATIONS.get(event.get("notification_type"))
        if kind and not pending:
            wait(NOTIFIED, kind)
    elif name == "SubagentStop":
        done_waiting(agent)
    elif name == "Stop":
        pending.clear()
        running = running_tasks(event.get("background_tasks"))
        r["background"] = len(running)
        if not running:
            r["status"] = DONE
        elif all(t.get("type") == SHELL_TASK for t in running):
            r["status"] = SHELL
        else:
            r["status"] = WORKING
    elif name == "StopFailure":
        # A failed turn is over too
        pending.clear()
        r["status"] = DONE

    if pending:
        r["status"] = PERMISSION if any(k == PERMISSION for _, k in pending) else INPUT
    elif r["status"] in NEEDS_YOU:
        # The last open prompt went away (a subagent stopped) and the turn goes on
        r["status"] = WORKING
    r["pending"] = pending
    r["updated_at"] = now
    # Hooks keep coming while a session waits (notifications, subagents), so how long it has
    # been in its status is kept apart from when it last changed
    if record is None or record.get("status") != r["status"] or not r.get("status_since"):
        r["status_since"] = now
    return r


def mark_seen(record, now):
    """Settles a finished session the user looked at as idle; callers check it is finished."""
    return {**record, "status": IDLE, "status_since": now}


def mark_interrupted(record, now):
    """Settles a session the user stopped with Esc, which fires no hook."""
    return {**record, "status": IDLE, "pending": [], "background": 0, "status_since": now}


def project(record):
    """The project folder's name, which names the session everywhere."""
    return (record.get("cwd") or "").rstrip("/").rsplit("/", 1)[-1] or "?"


def since(record):
    """When the session entered its current status."""
    return record.get("status_since") or record.get("updated_at", 0.0)


def sort_key(record):
    """Orders sessions by status, then the ones in that status longest first."""
    status = record.get("status")
    # A record with a status this version does not know sorts last instead of failing
    rank = STATUSES.index(status) if status in STATUSES else len(STATUSES)
    return rank, since(record)
