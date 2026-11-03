"""Where a session runs in tmux, and how to tell tmux about its changes."""

import os
import subprocess
from dataclasses import dataclass

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


@dataclass
class Window:
    """A tmux window running Claude, led by its most urgent session (teammates share it)."""

    window_id: str
    lead: dict

    @property
    def status(self):
        return self.lead["status"]

    @property
    def pane(self):
        return self.lead["mux"]["pane"]


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


# What the tab bar's right side counts, in order: status, mark and tmux style
SUMMARY = (
    (state.PERMISSION, "!", "fg=red bold"),
    (state.INPUT, "?", "fg=yellow bold"),
    (state.DONE, "✓", "fg=green"),
)


def summary_text(counts, limits):
    """Returns the tab bar's right side: tabs waiting or finished, and the 5h quota used.

    Args:
        counts: Open sessions per status.
        limits: The usage windows, as `statusline.load_limits` gives them.

    Returns:
        Text with tmux style codes; empty when there is nothing to say.
    """
    tabs = " ".join(
        f"#[{style}]{mark} {counts[status]}"
        for status, mark, style in SUMMARY
        if counts.get(status)
    )
    five = limits.get("five_hour")
    quota = f"5h {round(five['pct'])}%" if five else ""
    if tabs and quota:
        return f"{tabs} #[default]· {quota}"
    return tabs or quota


def toggle_tig(window_id, path, command="tig"):
    """Opens tig in a pane on the right half of a window, or closes the one already there.

    Args:
        window_id: The tmux window the key was pressed in.
        path: The folder of the pane, the project whose history tig shows.
        command: What runs in the new pane.
    """
    panes = table("list-panes", "-t", window_id, "-F", "#{pane_id}\t#{@cs-tig}")
    open_tig = [pane for pane, flag in panes.items() if flag]
    if open_tig:
        run("kill-pane", "-t", open_tig[0])
        return
    repo = subprocess.run(["git", "-C", path, "rev-parse", "--git-dir"], capture_output=True)
    if repo.returncode != 0:
        run("display-message", f" Not a git repository: {path}")
        return
    pane = run(
        "split-window",
        "-h",
        "-p",
        "50",
        "-c",
        path,
        "-t",
        window_id,
        "-P",
        "-F",
        "#{pane_id}",
        command,
    )
    if pane:
        run("set-option", "-p", "-t", pane, "@cs-tig", "1")


def publish_summary(text):
    """Sets the text the tab bar shows before the clock."""
    run("set", "-g", "@cs-summary", text)


def table(*args):
    """Runs a tmux listing whose format is two tab-separated fields, as a dict."""
    pairs = (line.split("\t", 1) for line in run(*args).splitlines())
    return dict(pair for pair in pairs if len(pair) == 2)


def server_socket():
    """Returns the socket of the tmux server this process runs in, '' outside tmux."""
    return (os.environ.get("TMUX") or "").split(",")[0]


def on_server(record, socket):
    """Tells whether a session runs in a pane of the tmux server at `socket`."""
    return bool(socket) and (record.get("mux") or {}).get("socket") == socket


def windows_of(records):
    """Groups live session records into the windows of this tmux server, most urgent first."""
    panes = table("list-panes", "-a", "-F", "#{pane_id}\t#{window_id}")
    socket = server_socket()
    windows = {}
    # Records arrive most urgent first, so the first one in a window leads it
    for r in records:
        window_id = panes.get((r.get("mux") or {}).get("pane"))
        if on_server(r, socket) and window_id and window_id not in windows:
            windows[window_id] = Window(window_id, r)
    return sorted(windows.values(), key=lambda w: state.sort_key(w.lead))


def window_tabs():
    """Returns the tab number of each window of this tmux server, by window id."""
    return table("list-windows", "-a", "-F", "#{window_id}\t#{window_index}")


def select(window):
    """Brings a window, and the pane of its leading session, on screen."""
    run("select-window", "-t", window.window_id)
    run("select-pane", "-t", window.pane)


def new_session_popup():
    """Opens the popup where the user picks a folder for a new session."""
    run("display-popup", "-E", "-w", "80%", "-h", "70%", "-T", " New session ", "cs new --pick")


def next_pending(windows, current=None):
    """Returns the pending window after the current one, wrapping around.

    Pending windows are the ones that need the user, then the ones that finished, in the
    order `windows_of` returns them, so repeated presses walk through all of them.

    Args:
        windows: The windows from `windows_of`.
        current: The id of the window on screen, or None.
    """
    pending = [w for w in windows if w.status in state.NEEDS_YOU]
    pending += [w for w in windows if w.status == state.DONE]
    if not pending:
        return None
    ids = [w.window_id for w in pending]
    if current not in ids:
        return pending[0]
    return pending[(ids.index(current) + 1) % len(pending)]


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
