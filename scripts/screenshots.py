"""Renders the screenshots in docs/screenshots from a demo world, without opening a window.

The demo is a tmux server of its own (`tmux -L cs-demo`, its socket in a temporary folder)
with the packaged tmux config, five sessions with neutral names (one per status), their usage
and limits, a journal of the day and a fake Claude Code history. A tmux client attaches to it
inside a pseudo-terminal of a fixed size; its output feeds a `pyte` screen, which Rich exports
to SVG. Nothing touches the user's own tmux server, settings or session records. The
temporary folder follows TMPDIR and is deleted at the end, with the demo server.

Run it with `uv run python scripts/screenshots.py`. Alt+g's shot needs tig, Alt+c's fzf.
"""

import datetime
import fcntl
import importlib.resources
import io
import json
import os
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import uuid

import pyte
from rich.console import Console
from rich.style import Style
from rich.text import Text

from claude_sessions.claude import files, statusline
from claude_sessions.core import journal, paths, state, store
from claude_sessions.term import launcher

COLUMNS, LINES = 120, 36
SOCKET = "cs-demo"
CONF = str(importlib.resources.files("claude_sessions.term") / "data" / "tmux.conf")
OUT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "docs", "screenshots"
)
MINUTE, HOUR, DAY = 60, 3600, 86400

# The open sessions, one per status: tab name, title, status, seconds in it
OPEN = (
    ("api", "Restructure the config loader", state.PERMISSION, 3 * MINUTE),
    ("docs-site", "Fix broken links in the guides", state.INPUT, 12 * MINUTE),
    ("dashboards", "Migrate usage dashboards to v2", state.WORKING, 9 * MINUTE),
    ("release-notes", "Write the release notes", state.DONE, 4 * MINUTE),
    ("infra", "Explore tmux popup styles", state.IDLE, 25 * MINUTE),
)
# What their status lines reported: cost, duration, model, lines added and removed
SPENT = {
    "api": (4.12, 100 * MINUTE, "Opus 5.5", 820, 310),
    "docs-site": (1.30, 35 * MINUTE, "Sonnet 5.5", 44, 12),
    "dashboards": (2.75, 52 * MINUTE, "Opus 5.5", 210, 95),
    "release-notes": (0.08, 4 * MINUTE, "Haiku 5.5", 0, 0),
    "infra": (0.42, 18 * MINUTE, "Opus 5.5", 6, 2),
}
# Closed sessions: folder, title, how long ago, cost (counted in today's total when today)
RECENT = (
    ("sync-service", "Review the retry logic", 2 * HOUR, 2.10),
    ("notes", "Weekly notes", DAY + 3 * HOUR, 0.85),
    ("docs-site", "Update the onboarding guide", 2 * DAY, 1.40),
    ("dashboards", "Profile slow queries", 3 * DAY, 3.05),
)
# Each open session's day, as (fraction of the span drawn, status), before its current status
DAYS = {
    "api": (
        (0.0, state.WORKING),
        (0.18, state.PERMISSION),
        (0.21, state.WORKING),
        (0.55, state.DONE),
        (0.6, state.IDLE),
        (0.7, state.WORKING),
    ),
    "docs-site": (
        (0.25, state.WORKING),
        (0.4, state.INPUT),
        (0.47, state.WORKING),
        (0.8, state.DONE),
        (0.83, state.IDLE),
        (0.88, state.WORKING),
    ),
    "dashboards": (
        (0.1, state.WORKING),
        (0.3, state.DONE),
        (0.42, state.IDLE),
        (0.5, state.WORKING),
        (0.62, state.PERMISSION),
        (0.64, state.WORKING),
        (0.85, state.IDLE),
    ),
    "release-notes": ((0.9, state.WORKING),),
    "infra": ((0.35, state.WORKING), (0.52, state.DONE), (0.58, state.IDLE), (0.75, state.WORKING)),
}
COMMITS = (
    "Add a health check endpoint",
    "Read the config from one place",
    "Retry requests that time out",
    "Log the request id on errors",
    "Split the loader into small functions",
    "Document the config file format",
)
AUTHOR = {
    "GIT_AUTHOR_NAME": "Alex Rivera",
    "GIT_AUTHOR_EMAIL": "alex@example.com",
    "GIT_COMMITTER_NAME": "Alex Rivera",
    "GIT_COMMITTER_EMAIL": "alex@example.com",
}


def demo_env(root):
    """A small, clean environment for the demo: nothing of the user's leaks into the shots."""
    # tmux runs `cs` from the environment this script runs in, as `cst` does with its own
    return {
        "PATH": os.pathsep.join([os.path.dirname(sys.executable), os.environ["PATH"]]),
        "HOME": os.path.join(root, "home"),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "TERM": "xterm-256color",
        "TMUX_TMPDIR": os.path.join(root, "tmux"),
        "CLAUDE_SESSIONS_DIR": os.path.join(root, "run"),
        "CLAUDE_SESSIONS_STATE": os.path.join(root, "state"),
        "CLAUDE_CONFIG_DIR": os.path.join(root, "home", ".claude"),
        "GIT_CONFIG_NOSYSTEM": "1",
    }


def tmux(env, *args):
    """Runs a tmux command on the demo server and returns its output."""
    out = subprocess.run(
        ["tmux", "-L", SOCKET, "-f", CONF, *args],
        env=env,
        capture_output=True,
        text=True,
        check=True,
    )
    return out.stdout.strip()


def project_dir(env, name):
    path = os.path.join(env["HOME"], "projects", name)
    os.makedirs(path, exist_ok=True)
    return path


def write_transcript(cwd, session_id, title):
    path = files.transcript_path(session_id, cwd)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(json.dumps({"type": "summary", "aiTitle": title}, separators=(",", ":")) + "\n")
    return path


def make_repo(env, path, now):
    """Gives a project a short, neutral commit history for Alt+g."""
    git_env = {**env, **AUTHOR}
    subprocess.run(["git", "init", "-q", "-b", "main", path], env=git_env, check=True)
    for i, message in enumerate(COMMITS):
        stamp = f"@{int(now - (len(COMMITS) - i) * 5 * HOUR)} {time.strftime('%z')}"
        with open(os.path.join(path, "CHANGES"), "a") as f:
            f.write(message + "\n")
        subprocess.run(["git", "-C", path, "add", "CHANGES"], env=git_env, check=True)
        subprocess.run(
            ["git", "-C", path, "commit", "-q", "-m", message],
            env={**git_env, "GIT_AUTHOR_DATE": stamp, "GIT_COMMITTER_DATE": stamp},
            check=True,
        )


def start_server(env):
    """Starts the demo tmux server, with the panel in tab 1 as `cst` has it."""
    for key in ("CLAUDE_SESSIONS_DIR", "CLAUDE_SESSIONS_STATE", "CLAUDE_CONFIG_DIR"):
        # The modules of claude_sessions read these too, to write the demo's files
        os.environ[key] = env[key]
        os.makedirs(env[key], exist_ok=True)
    os.makedirs(env["TMUX_TMPDIR"], mode=0o700, exist_ok=True)
    session = ["-d", "-s", launcher.SESSION, "-n", "sessions", "-c", env["HOME"]]
    size = ["-x", str(COLUMNS), "-y", str(LINES)]
    tmux(env, "new-session", *session, *size, launcher.PANEL)
    return tmux(env, "display-message", "-p", "#{socket_path}")


def add_open(env, socket, name, title, status, waited, start, now):
    """Opens a tab for a session, with its record, its tmux status and its day in the journal.

    Returns:
        The session id.
    """
    cwd = project_dir(env, name)
    window = ["-d", "-n", name, "-c", cwd, "-P", "-F", "#{pane_id} #{pane_pid}"]
    # The tab's own `sleep` stands for the Claude process, so the record counts as alive
    pane, pid = tmux(env, "new-window", *window, "sleep 86400").split()
    session_id = str(uuid.uuid4())
    since = now - waited
    for fraction, step in DAYS[name]:
        t = min(start + fraction * (now - start), since - 1)
        journal.append({"t": t, "id": session_id, "status": step})
    journal.append({"t": since, "id": session_id, "status": status})
    spent = dict(zip(("cost", "duration", "model", "added", "removed"), SPENT[name], strict=True))
    journal.append({"t": now - MINUTE, "id": session_id, "cost": spent["cost"]})
    record = {
        **state.new_record(session_id),
        "status": status,
        "cwd": cwd,
        "pid": int(pid),
        "pid_start": store.process_start(int(pid)),
        "transcript": write_transcript(cwd, session_id, title),
        "status_since": since,
        "updated_at": since,
        "mux": {"socket": socket, "pane": pane},
        "usage": spent,
    }
    paths.write_json(os.path.join(paths.state_dir(), f"{session_id}.json"), record)
    tmux(env, "set-option", "-p", "-t", pane, "@claude_status", status)
    if status in state.NEEDS_YOU:
        tmux(env, "set-option", "-p", "-t", pane, "@cs-since", str(int(since)))
    return session_id


def add_closed(env, name, title, ended, cost, start):
    """Leaves a closed session in the history and, if it ran today, in the journal.

    Returns:
        The session id.
    """
    session_id = str(uuid.uuid4())
    write_transcript(project_dir(env, name), session_id, title)
    if ended >= start:
        steps = ((HOUR, state.WORKING), (20 * MINUTE, state.INPUT), (15 * MINUTE, state.WORKING))
        for before, step in steps:
            journal.append({"t": ended - before, "id": session_id, "status": step})
        journal.append({"t": ended, "id": session_id, "cost": cost})
        journal.append({"t": ended, "id": session_id, "status": journal.ENDED})
    return session_id


def build_world(env, now):
    """Starts the demo tmux server and fills the folders the tool reads."""
    socket = start_server(env)
    midnight = datetime.datetime.combine(datetime.date.today(), datetime.time()).timestamp()
    start = max(midnight + MINUTE, now - 7 * HOUR)
    history = []
    for name, title, status, waited in OPEN:
        session_id = add_open(env, socket, name, title, status, waited, start, now)
        history.append((start, session_id, project_dir(env, name), title))
    for name, title, ago, cost in RECENT:
        session_id = add_closed(env, name, title, now - ago, cost, start)
        history.append((now - ago, session_id, project_dir(env, name), title))
    with open(files.history_path(), "w") as f:
        for when, session_id, cwd, title in sorted(history):
            entry = {"display": title, "timestamp": int(when * 1000), "project": cwd}
            f.write(json.dumps({**entry, "sessionId": session_id}) + "\n")
    limits = {
        "five_hour": {"pct": 64, "resets_at": now + 112 * MINUTE},
        "seven_day": {"pct": 31, "resets_at": now + 3 * DAY + 5 * HOUR},
    }
    statusline.save_limits(limits, now)
    make_repo(env, project_dir(env, "api"), now)


class Screen(pyte.Screen):
    """A pyte screen that ignores the private status reports tmux asks for (pyte fails)."""

    def report_device_status(self, mode, private=False, **kwargs):
        if not private:
            super().report_device_status(mode)


class Terminal:
    """A tmux client in a pseudo-terminal, read into a pyte screen."""

    def __init__(self, env):
        self.screen = Screen(COLUMNS, LINES)
        # Answers the client's queries (device attributes), as a real terminal would
        self.screen.write_process_input = self.send
        self.stream = pyte.ByteStream(self.screen)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            try:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", LINES, COLUMNS, 0, 0))
                argv = ["tmux", "-u", "-L", SOCKET, "-f", CONF, "attach", "-t", launcher.SESSION]
                os.execvpe(argv[0], argv, env)
            finally:
                os._exit(127)

    def send(self, data):
        os.write(self.fd, data.encode() if isinstance(data, str) else data)

    def pump(self, seconds):
        """Reads the client's output for a while; returns whether any came."""
        ready, _, _ = select.select([self.fd], [], [], seconds)
        if not ready:
            return False
        try:
            self.stream.feed(os.read(self.fd, 65536))
        except OSError:
            return False
        return True

    def text(self):
        return "\n".join(self.screen.display)

    def wait_for(self, predicate, timeout=20):
        """Waits until a quiet moment in which the screen satisfies the predicate."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            while self.pump(0.15):
                pass
            if predicate(self.screen.display):
                return
        raise TimeoutError(f"the screen never got there:\n{self.text()}")

    def close(self):
        try:
            os.kill(self.pid, signal.SIGTERM)
            os.waitpid(self.pid, 0)
        except OSError:
            pass
        os.close(self.fd)


def colour(name):
    """A pyte colour as Rich takes it: a name, `#rrggbb`, or None for the default."""
    if name == "default":
        return None
    if len(name) == 6 and all(c in "0123456789abcdefABCDEF" for c in name):
        return "#" + name
    bright = name.startswith("bright")
    base = name.removeprefix("bright").replace("brown", "yellow")
    return f"bright_{base}" if bright else base


def row_text(screen, y):
    """One line of the screen as Rich text, cell by cell, with its colours."""
    line = Text(no_wrap=True, end="")
    row = screen.buffer[y]
    for x in range(screen.columns):
        char = row[x]
        # pyte leaves an empty cell after a wide character
        if char.data == "":
            continue
        style = Style(
            color=colour(char.fg),
            bgcolor=colour(char.bg),
            bold=char.bold,
            italic=char.italics,
            underline=char.underscore,
            reverse=char.reverse,
        )
        line.append(char.data, style)
    return line


def save(screen, name, rows, title):
    """Writes these lines of the screen to docs/screenshots as an SVG terminal window."""
    console = Console(record=True, width=COLUMNS, color_system="truecolor", file=io.StringIO())
    for y in rows:
        console.print(row_text(screen, y), overflow="crop", no_wrap=True)
    path = os.path.join(OUT, name)
    with open(path, "w") as f:
        f.write(console.export_svg(title=title, unique_id=name.removesuffix(".svg")))
    print(f"wrote {os.path.relpath(path)}")


def capture(env):
    """Attaches to the demo server, and saves each screenshot once its screen is drawn."""
    term = Terminal(env)
    try:
        # The working tab blinks: wait for the ● frame, with the panel drawn
        term.wait_for(
            lambda d: "● dashboards" in d[-1] and "Recent" in "".join(d) and "$4.12" in "".join(d)
        )
        save(term.screen, "panel.svg", range(LINES), "claude-sessions")

        if shutil.which("fzf"):
            term.send("\x1bc")
            term.wait_for(
                lambda d: "New session in:" in "".join(d) and "~/projects/infra" in "".join(d)
            )
            save(term.screen, "new-session.svg", range(LINES), "Alt+c")
            term.send("\x1b")
            term.wait_for(lambda d: "New session in:" not in "".join(d))
        else:
            print("fzf is missing: new-session.svg was not made")

        if shutil.which("tig"):
            term.send("\x1b2")
            term.wait_for(lambda d: "Claude sessions" not in "".join(d))
            term.send("\x1bg")
            term.wait_for(lambda d: COMMITS[-1] in "".join(d))
            save(term.screen, "history.svg", range(LINES), "Alt+g")
            term.send("q")
        else:
            print("tig is missing: history.svg was not made")
    finally:
        term.close()


def main():
    root = tempfile.mkdtemp(prefix="cs-demo-")
    env = demo_env(root)
    os.makedirs(OUT, exist_ok=True)
    try:
        build_world(env, time.time())
        capture(env)
    finally:
        subprocess.run(["tmux", "-L", SOCKET, "kill-server"], env=env, capture_output=True)
        # The panel may write its folders once more while it exits
        time.sleep(1)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
