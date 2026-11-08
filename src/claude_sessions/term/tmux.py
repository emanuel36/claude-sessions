"""Where a session runs in tmux, and how to tell tmux about its changes."""

import collections
import os
import subprocess

from claude_sessions.core import state
from claude_sessions.term.launcher import SOCKET

# A hook makes up to 7 tmux calls; together they must stay under its 10 s timeout
TIMEOUT = 1
# Long enough for a session's title; the sidebar fits it to its own width
WINDOW_NAME_LEN = 60
# What Claude Code calls a session that runs in its daemon (moved to the background)
BACKGROUND = "bg"
# How Claude Code names a run without a terminal (`claude -p`, the Agent SDK)
HEADLESS = {"sdk-cli", "sdk-ts", "sdk-py"}
# `claude attach` takes a session's id or its first 8 characters
MIN_ID_PREFIX = 8
# Every pane of the server and its window, in one listing; the window's name comes last, as
# it may hold a tab
PANES_FORMAT = "\t".join(
    (
        "#{pane_id}",
        "#{window_id}",
        "#{pane_index}",
        "#{window_index}",
        "#{@cs-sidebar}",
        "#{window_name}",
    )
)
# Columns of the sidebar pane; tmux's border makes it 30. Not imported from the UI: the hook
# path must not load rich
SIDEBAR_WIDTH = 29


def detect(env, session_id=None):
    """Returns the tmux pane a session runs in, or {} outside tmux.

    A background session runs in Claude Code's daemon, outside tmux, so its pane is the one
    that shows it with `claude attach`.

    Args:
        env: The process environment.
        session_id: The session's id, to find the pane of a background session.
    """
    # A session moved to the background may still carry the tmux pane it left
    # Hooks get no session kind, only the job folder the daemon gives a background session
    if env.get("CLAUDE_JOB_DIR") or env.get("CLAUDE_CODE_SESSION_KIND") == BACKGROUND:
        return attached_pane(session_id) if session_id else {}
    # A `claude -p` started from a session (a start hook, a script) inherits that session's
    # TMUX, but runs in no pane
    if is_headless(env):
        return {}
    if env.get("TMUX") and env.get("TMUX_PANE"):
        return {"socket": env["TMUX"].split(",")[0], "pane": env["TMUX_PANE"]}
    return {}


def is_headless(env):
    """Tells whether the hook comes from a run without a terminal, such as `claude -p`."""
    return env.get("CLAUDE_CODE_ENTRYPOINT") in HEADLESS


def attached_pane(session_id):
    """Returns the claude-sessions pane that runs `claude attach` on a session, or {}."""
    out = run("-L", SOCKET, "list-panes", "-a", "-F", "#{socket_path}\t#{pane_id}\t#{pane_pid}")
    for line in out.splitlines():
        fields = line.split("\t")
        if len(fields) != 3:
            continue
        socket, pane, pid = fields
        # The pane runs claude itself, or a shell that started it
        if any(attaches(_cmdline(p), session_id) for p in (pid, *_children(pid))):
            return {"socket": socket, "pane": pane}
    return {}


def attaches(argv, session_id):
    """Tells whether a command line is `claude attach` on the session, by its id or a prefix."""
    # Not always argv[1]: claude may run through node, or with flags before the subcommand
    if "attach" not in argv:
        return False
    after = argv[argv.index("attach") + 1 :]
    return any(len(a) >= MIN_ID_PREFIX and session_id.startswith(a) for a in after)


def _cmdline(pid):
    """A process's command line as a list; empty when it is gone."""
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as f:
            return f.read().decode(errors="replace").split("\0")[:-1]
    except OSError:
        return []


def _children(pid):
    """The ids of a process's direct children."""
    try:
        with open(f"/proc/{pid}/task/{pid}/children") as f:
            return f.read().split()
    except OSError:
        return []


# Plain classes, not dataclasses: the hook loads this module, and dataclasses take long to load
class Window:
    """A tmux window running Claude; teammates share it.

    `lead` is its most urgent session, which sets the status shown. `leader` is the session in
    the window's first Claude pane, whose model, branch and cost the window shows.
    """

    def __init__(self, window_id, lead, leader=None):
        self.window_id, self.lead, self.leader = window_id, lead, leader

    @property
    def head(self):
        """The session whose details the window shows."""
        return self.leader or self.lead


# A pane of this tmux server, and the window it is in
Pane = collections.namedtuple(
    "Pane", ("pane_id", "window_id", "index", "window_index", "sidebar", "window_name")
)


def panes():
    """Returns the panes of the tmux server this process runs in."""
    out = []
    for line in run("list-panes", "-a", "-F", PANES_FORMAT).splitlines():
        f = line.split("\t", 5)
        if len(f) == 6 and f[2].isdigit() and f[3].isdigit():
            out.append(Pane(f[0], f[1], int(f[2]), int(f[3]), bool(f[4]), f[5]))
    return out


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


def close_window(window_id):
    """Closes a window, first stopping the background sessions its panes show.

    A pane that runs `claude attach <id>` only views a session of Claude's daemon, which would
    outlive the window, so each one is stopped. A failed stop does not keep the window open.

    Args:
        window_id: The tmux window to close.
    """
    for pid in run("list-panes", "-t", window_id, "-F", "#{pane_pid}").split():
        for p in (pid, *_children(pid)):
            argv = _cmdline(p)
            if "attach" not in argv:
                continue
            ids = [a for a in argv[argv.index("attach") + 1 :] if not a.startswith("-")]
            arg = next((a for a in ids if len(a) >= MIN_ID_PREFIX), None)
            if arg is None:
                continue
            try:
                subprocess.run(["claude", "stop", arg], capture_output=True, timeout=10)
            except (OSError, subprocess.SubprocessError):
                pass
    # Left to itself, tmux goes back to the window used before, often the landing page
    if run("display-message", "-p", "-t", window_id, "#{window_active}") == "1":
        neighbour = next_session_window(window_id)
        if neighbour:
            run("select-window", "-t", neighbour)
    run("kill-window", "-t", window_id)


def next_session_window(window_id):
    """The session window to show when one closes: the next one by index, else the one before.

    Session windows are those with a sidebar; None when the closing one is the last of them.
    """
    listed = panes()
    with_sidebar = {p.window_id for p in listed if p.sidebar}
    order = [w for w, _, _ in window_entries(listed) if w in with_sidebar or w == window_id]
    if window_id not in order:
        return None
    i = order.index(window_id)
    after, before = order[i + 1 :], order[:i]
    return (after or before[::-1] or [None])[0]


def add_sidebar(window_id, command="cs sidebar"):
    """Puts the sidebar in a pane on the far left of a window, unless it already has one.

    The pane is marked @cs-sidebar and the window's other pane stays the active one.

    Args:
        window_id: The tmux window to add it to.
        command: What runs in the new pane.
    """
    flags = run("list-panes", "-t", window_id, "-F", "#{@cs-sidebar}")
    if "1" in flags.split():
        return
    pane = run(
        "split-window", "-hbd", "-f", "-l", str(SIDEBAR_WIDTH),
        "-t", window_id, "-P", "-F", "#{pane_id}", command,
    )  # fmt: skip
    if pane:
        run("set-option", "-p", "-t", pane, "@cs-sidebar", "1")


def fit_sidebars():
    """Resizes every sidebar pane back to its width, after a split or a terminal resize."""
    for pane in panes():
        if pane.sidebar:
            run("resize-pane", "-t", pane.pane_id, "-x", str(SIDEBAR_WIDTH))


def server_socket():
    """Returns the socket of the tmux server this process runs in, '' outside tmux."""
    return (os.environ.get("TMUX") or "").split(",")[0]


def on_server(record, socket):
    """Tells whether a session runs in a pane of the tmux server at `socket`."""
    return bool(socket) and (record.get("mux") or {}).get("socket") == socket


def windows_of(records, listed=None):
    """Groups live session records into the windows of this tmux server, most urgent first.

    Args:
        records: The live records, most urgent first.
        listed: The server's panes from `panes`, if already listed.
    """
    socket = server_socket()
    where = {p.pane_id: p for p in (panes() if listed is None else listed)}
    windows, leaders = {}, {}
    # Records arrive most urgent first, so the first one in a window leads it
    for r in records:
        found = where.get((r.get("mux") or {}).get("pane"))
        if not on_server(r, socket) or found is None:
            continue
        window_id, index = found.window_id, found.index
        if window_id not in windows:
            windows[window_id] = Window(window_id, r)
        # The leader is the session in the window's first pane
        if window_id not in leaders or index < leaders[window_id][0]:
            leaders[window_id] = (index, r)
    for window_id, (_, r) in leaders.items():
        windows[window_id].leader = r
    return sorted(windows.values(), key=lambda w: state.sort_key(w.lead))


def window_entries(listed=None):
    """Returns `(window_id, index, name)` of this server's windows, by index.

    Args:
        listed: The server's panes from `panes`, if already listed.
    """
    windows = {}
    for p in panes() if listed is None else listed:
        windows.setdefault(p.window_id, (p.window_id, p.window_index, p.window_name))
    return sorted(windows.values(), key=lambda row: row[1])


def tmux_pane_in_view(where):
    """Tells whether the pane is the active one in a window someone is looking at."""
    fmt = "#{&&:#{&&:#{pane_active},#{window_active}},#{session_attached}}"
    return tmux(where["socket"], "display-message", "-p", "-t", where["pane"], fmt) == "1"


def window_name(record, title=""):
    """Names a window after its session's title, or its project folder until it has one."""
    name = " ".join(title.split()) or state.project(record)
    return name if len(name) <= WINDOW_NAME_LEN else name[: WINDOW_NAME_LEN - 1] + "…"


def name_window(record, title=""):
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
    wanted = window_name(record, title)
    if (auto == "1" or current == previous) and current != wanted:
        tmux(where["socket"], "rename-window", "-t", where["pane"], wanted)
        tmux(where["socket"], "set-option", "-w", "-t", where["pane"], "@cs-auto-name", wanted)


def settle_if_viewed(record, now):
    """Marks a finished session seen right away when its pane is already on screen."""
    where = record.get("mux") or {}
    if record["status"] != state.DONE or not where:
        return record
    return state.mark_seen(record, now) if tmux_pane_in_view(where) else record
