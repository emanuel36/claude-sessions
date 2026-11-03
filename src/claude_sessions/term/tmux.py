"""Where a session runs in tmux, and how to tell tmux about its changes."""

import subprocess

from claude_sessions.core import state

# A hook makes up to 6 tmux calls; together they must stay under its 10 s timeout
TIMEOUT = 1
WINDOW_NAME_LEN = 28


def detect(env):
    """Returns the tmux pane a process runs in, or {} outside tmux.

    Args:
        env: The process environment.
    """
    if not (env.get("TMUX") and env.get("TMUX_PANE")):
        return {}
    return {"socket": env["TMUX"].split(",")[0], "pane": env["TMUX_PANE"]}


def run(*args):
    """Runs a tmux command and returns its output, '' if it fails; never raises.

    Without `-S` the command goes to the server this process runs in.
    """
    try:
        out = subprocess.run(
            ["tmux", *args], capture_output=True, text=True, timeout=TIMEOUT, check=False
        )
    except (OSError, subprocess.SubprocessError):
        return ""
    return out.stdout.rstrip("\n")


def tmux(socket, *args):
    """Runs a tmux command on the given server and returns its output, '' if it fails."""
    return run("-S", socket, *args)


def tmux_pane_in_view(where):
    """Tells whether the pane is the active one in a window someone is looking at."""
    fmt = "#{&&:#{&&:#{pane_active},#{window_active}},#{session_attached}}"
    return tmux(where["socket"], "display-message", "-p", "-t", where["pane"], fmt) == "1"


def notify(record, ended=False):
    """Pushes a status change to the tmux pane the session runs in.

    tmux keeps the status as a pane option that its status line reads, and, while the session
    needs the user, since when as @cs-since, from which the tab counts the wait. Never raises:
    the hook must not break the session.

    Args:
        record: The session record after the change.
        ended: True when the session is closing.
    """
    where = record.get("mux") or {}
    if not where:
        return
    status = None if ended else record["status"]
    since = str(int(state.since(record))) if status in state.NEEDS_YOU else None
    for option, value in (("@claude_status", status), ("@cs-since", since)):
        if value:
            tmux(where["socket"], "set-option", "-p", "-t", where["pane"], option, value)
        else:
            tmux(where["socket"], "set-option", "-p", "-u", "-t", where["pane"], option)
    tmux(where["socket"], "refresh-client", "-S")


def window_name(record):
    """Names a window after its session's project folder."""
    name = state.project(record)
    return name if len(name) <= WINDOW_NAME_LEN else name[: WINDOW_NAME_LEN - 1] + "…"


def name_window(record):
    """Gives a tmux window its session's name, unless the user named the window.

    tmux names a window after its command ("claude") while automatic-rename is on; renaming
    turns that off, so @cs-auto-name remembers the last name set here. A window still on
    either of those is ours to rename; any other name came from the user. Never raises.
    """
    where = record.get("mux") or {}
    if not where or not record.get("cwd"):
        return
    fmt = "#{automatic-rename}\t#{@cs-auto-name}\t#{window_name}"
    fields = tmux(where["socket"], "display-message", "-p", "-t", where["pane"], fmt).split("\t")
    # tmux failed, or the pane is gone
    if len(fields) != 3:
        return
    auto, previous, current = fields
    wanted = window_name(record)
    if (auto == "1" or current == previous) and current != wanted:
        tmux(where["socket"], "rename-window", "-t", where["pane"], wanted)
        tmux(where["socket"], "set-option", "-w", "-t", where["pane"], "@cs-auto-name", wanted)


def settle_if_viewed(record, now):
    """Marks a finished session seen right away when its pane is already on screen."""
    where = record.get("mux") or {}
    if record["status"] != state.DONE or not where:
        return record
    return state.mark_seen(record, now) if tmux_pane_in_view(where) else record
