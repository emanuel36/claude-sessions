"""`cs hook` and what follows from Claude Code's events: the live sessions, as they really are.

Esc fires no hook, so a session the user interrupted is settled here, when it is read.
"""

import os

from claude_sessions.claude import files
from claude_sessions.core import state, store
from claude_sessions.term import tmux

# Events after which the window takes the session's folder name
NAMING_EVENTS = {"SessionStart", "UserPromptSubmit"}
# How a session starts when it takes the place of another in the same Claude process
REPLACING = {"clear", "resume"}
# Esc fires no hook, so these statuses can outlive an interrupted turn or a rejected prompt
INTERRUPTIBLE = (state.WORKING, *state.NEEDS_YOU)
# A working session with no hook for this long is checked for an Esc interrupt
QUIET_SECONDS = 3.0


def claude_pid(env):
    """Returns the PID of the Claude process that ran the hook."""
    pid = env.get("CLAUDE_PID")
    return int(pid) if pid and pid.isdigit() else os.getppid()


def handle_event(event, env, now):
    """Applies one hook event to its session record and tells tmux.

    Args:
        event: The hook input.
        env: The environment the hook inherited from Claude Code.
        now: Current time as a Unix timestamp.
    """
    session_id = event.get("session_id")
    with store.locked(session_id):
        if event.get("hook_event_name") == "SessionEnd":
            # The lock stays: a hook may be waiting on it, and the dead-process sweep removes it
            store.remove(session_id, keep_lock=True)
            return
        record = state.apply_event(store.read(session_id), event, now)
        pid = claude_pid(env)
        record.update(pid=pid, pid_start=store.process_start(pid), mux=tmux.detect(env, session_id))
        # A run without a terminal (a start hook's `claude -p`) is listed apart from the sessions
        record["headless"] = tmux.is_headless(env)
        # Only hook input carries the mode; it stays until a later event tells another one
        record["mode"] = event.get("permission_mode") or record.get("mode")
        record = tmux.settle_if_viewed(record, now)
        store.write(record)
        # Told under the lock, so overlapping hooks reach tmux in the order they hit the disk
        if event.get("hook_event_name") in NAMING_EVENTS:
            tmux.name_window(record)
    if event.get("hook_event_name") == "SessionStart" and event.get("source") in REPLACING:
        store.remove_replaced(record)


def live_sessions(now):
    """Returns the records of all live sessions, ordered for display.

    Records of dead processes are deleted, and sessions the user interrupted with Esc (in the
    middle of a turn, or at a prompt) are settled as idle on disk.

    Args:
        now: Current time as a Unix timestamp.
    """
    records = []
    for record in store.load_all(now):
        if (
            record["status"] in INTERRUPTIBLE
            and not record.get("background")
            and now - record.get("updated_at", 0) > QUIET_SECONDS
            and files.was_interrupted(record.get("transcript"))
        ):
            record = settle_interrupted(record["id"], now) or record
        records.append(record)
    # Settling changes a status, and with it the order
    return sorted(records, key=state.sort_key)


def settle_interrupted(session_id, now):
    """Writes an Esc interrupt to disk, unless a hook changed the session meanwhile."""
    with store.locked(session_id):
        current = store.read(session_id)
        if current is None or current["status"] not in INTERRUPTIBLE:
            return current
        settled = state.mark_interrupted(current, now)
        store.write(settled)
    return settled


def seen(pane, socket, now):
    """Marks the finished session in a tmux pane as seen on disk.

    Args:
        pane: The pane's id, such as `%3`.
        socket: The socket of the pane's tmux server.
        now: Current time as a Unix timestamp.
    """
    for r in live_sessions(now):
        where = r.get("mux", {})
        # Pane ids repeat across tmux servers; only the server's own session counts
        if where.get("pane") != pane or where.get("socket") != socket:
            continue
        with store.locked(r["id"]):
            current = store.read(r["id"])
            if current is None or current["status"] != state.DONE:
                continue
            current = state.mark_seen(current, now)
            store.write(current)
