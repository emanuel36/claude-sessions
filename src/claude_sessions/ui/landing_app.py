"""The landing page: what `cst` opens with, and what it returns to when no session is left.

Runs as `cs landing` in the first window, which stays for the life of the server: closing the
last session brings the user back to it.
"""

import datetime
import os
import shlex
import subprocess
import time

from rich.text import Text
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.widgets import Static

from claude_sessions.claude import files, hook, statusline
from claude_sessions.core import paths
from claude_sessions.term import tmux
from claude_sessions.ui import landing, usage, words

RECENT = 5
FOLDERS = 4
# How long a folder's git state is trusted before git is asked again
GIT_SECONDS = 5
_git = {}
_reader = usage.Reader()
_titles = files.Titles()


def filter_rows(rows, query):
    """Keeps the rows whose title or folder holds every word of the query."""
    words = query.lower().split()

    def matches(row):
        text = f"{row.title} {os.path.basename(row.cwd.rstrip('/'))}".lower()
        return all(word in text for word in words)

    return [r for r in rows if matches(r)]


def git_state(path):
    """Returns the branch and the number of changed files of a folder.

    Args:
        path: The folder.

    Returns:
        `("", None)` when git fails there (not a repository, or the folder is gone).
    """
    now = time.monotonic()
    cached = _git.get(path)
    if cached and now - cached[0] < GIT_SECONDS:
        return cached[1]
    state = ("", None)
    try:
        branch = subprocess.run(
            ["git", "-C", path, "branch", "--show-current"],
            capture_output=True,
            text=True,
            timeout=2,
        )
        status = subprocess.run(
            ["git", "-C", path, "status", "--porcelain"],
            capture_output=True,
            text=True,
            timeout=2,
        )
        if branch.returncode == 0 and status.returncode == 0:
            state = (branch.stdout.strip(), len(status.stdout.splitlines()))
    except (OSError, subprocess.SubprocessError):
        pass
    _git[path] = (now, state)
    return state


def load(now):
    """Returns the landing page's contents at a moment.

    Args:
        now: Current time as a Unix timestamp.

    Returns:
        The recent sessions, the folders, the usage limits, the week's totals, the sessions per
        day of the last 14 days and the date of the first of them.
    """
    history = files.load_history()
    totals = _reader.totals(history, now)
    opened = {r["id"] for r in hook.live_sessions(now)}
    recent = []
    for s in history:
        if s.id in opened:
            continue
        title = _titles.get(s.id, files.transcript_path(s.id, s.cwd)) or s.first_prompt
        # Only commands were typed (a /resume that was left): nothing to come back to
        if not title:
            continue
        name = os.path.basename(s.cwd.rstrip("/"))
        recent.append(
            landing.Recent(s.id, name, title, totals.per_session.get(s.id), s.last, s.cwd)
        )
        if len(recent) == RECENT:
            break
    home = os.path.expanduser("~")
    folders = []
    for path in files.recent_projects()[:FOLDERS]:
        branch, changed = git_state(path)
        shown = "~" + path[len(home) :] if path == home or path.startswith(home + "/") else path
        folders.append(landing.Folder(shown, branch, changed))
    first = datetime.date.fromtimestamp(now) - datetime.timedelta(days=13)
    return (
        recent,
        folders,
        statusline.load_limits(now),
        totals,
        usage.activity(history, now),
        first,
    )


class LandingApp(App):
    """The landing page, redrawn every second."""

    ENABLE_COMMAND_PALETTE = False
    CSS = "Screen { background: transparent; } #page { width: 100%; height: 100%; }"
    BINDINGS = [
        Binding("ctrl+c", "nothing", show=False, priority=True),
        Binding("ctrl+q", "nothing", show=False, priority=True),
    ]

    def __init__(self, load=load, window_id=None):
        super().__init__(ansi_color=True)
        self.load = load
        self.window_id = window_id
        self.recent, self.folders = [], []
        self.limits, self.totals, self.activity, self.first_day = {}, usage.Totals(), [], None
        self.selected = 0
        # The search text while typing, and the filter it leaves behind
        self.search = None
        self.filter_text = ""
        self.now = time.time()
        # The last failed load that went to the error log; the same one is not logged again
        self.load_error = None
        self.draw_error = None

    def compose(self) -> ComposeResult:
        yield Static(id="page")

    def on_mount(self):
        if self.window_id is None:
            self.window_id = tmux.run(
                "display-message", "-p", "-t", os.environ["TMUX_PANE"], "#{window_id}"
            )
        self.tick()

    def action_nothing(self):
        pass

    def visible(self):
        """The recent sessions the filter lets through."""
        return filter_rows(self.recent, self.filter_text) if self.filter_text else self.recent

    def tick(self):
        if not self.is_running:
            return
        now = time.time()
        self.set_timer(words.next_tick(now), self.tick)
        self.tick_once(now)

    def tick_once(self, now):
        """Reads the page's data and redraws, unless another window is the one on screen."""
        self.now = now
        # Hidden behind a session, it has nothing to show: skip the reading and the git calls
        if tmux.run("display-message", "-p", "-t", self.window_id, "#{window_active}") == "0":
            return
        # A failed load leaves the last data, redrawn with the current time
        try:
            (self.recent, self.folders, self.limits, self.totals, self.activity, self.first_day) = (
                self.load(now)
            )
        except Exception as e:  # noqa: BLE001 - a file caught mid-change must not stop the page
            self.load_error = self.log_once(e, self.load_error)
        self.redraw()

    @staticmethod
    def log_once(exc, last):
        """Logs a failure unless it repeats the last one (once a second, it would fill the log).

        Returns:
            The failure's description, to be passed back as `last` next time.
        """
        error = f"{type(exc).__name__}: {exc}"
        if error != last:
            paths.log_error()
        return error

    def redraw(self):
        """Draws the page; a failed draw keeps the last screen."""
        try:
            self.draw()
        except Exception as e:  # noqa: BLE001 - a bad row must not stop the page
            self.draw_error = self.log_once(e, self.draw_error)

    def draw(self):
        rows = len(self.visible()) + len(self.folders) + 1
        self.selected = max(0, min(self.selected, rows - 1))
        search = self.search if self.search is not None else self.filter_text or None
        left = landing.left_lines(self.visible(), self.folders, self.selected, search, self.now)
        right = (
            landing.right_lines(self.limits, self.totals, self.activity, self.first_day, self.now)
            if self.first_day
            else []
        )
        lines = landing.screen(left, right, self.size.width)
        self.query_one("#page", Static).update(Text("\n").join(lines))

    def folder_path(self, shown):
        return os.path.expanduser(shown)

    def target(self):
        """The selected row as ("recent", row), ("folder", row) or ("other", None)."""
        recent = self.visible()
        if self.selected < len(recent):
            return "recent", recent[self.selected]
        i = self.selected - len(recent)
        if i < len(self.folders):
            return "folder", self.folders[i]
        return "other", None

    def open_selected(self):
        kind, row = self.target()
        if kind == "recent":
            tmux.run("new-window", "-c", row.cwd, f"claude --resume {shlex.quote(row.session_id)}")
        elif kind == "folder":
            tmux.run("new-window", "-c", self.folder_path(row.path), "claude")
        else:
            tmux.new_session_popup()

    def new_session(self):
        kind, row = self.target()
        if kind == "recent":
            tmux.run("new-window", "-c", row.cwd, "claude")
        elif kind == "folder":
            tmux.run("new-window", "-c", self.folder_path(row.path), "claude")
        else:
            tmux.new_session_popup()

    def on_key(self, event):
        event.stop()
        event.prevent_default()
        key = event.key
        if self.search is not None:
            if key == "escape":
                self.search, self.filter_text = None, ""
            elif key == "enter":
                self.search = None
            elif key == "backspace":
                self.search = self.search[:-1]
                self.filter_text = self.search
            elif event.is_printable and event.character:
                self.search += event.character
                self.filter_text = self.search
            self.selected = 0
        elif key in ("up", "k"):
            self.selected -= 1
        elif key in ("down", "j"):
            self.selected += 1
        elif key == "enter":
            self.open_selected()
        elif key == "n":
            self.new_session()
        elif key == "slash":
            self.search = self.filter_text
        elif key == "escape":
            self.filter_text = ""
            self.selected = 0
        elif key == "q":
            tmux.run("kill-server")
        self.redraw()


def main():
    LandingApp().run()
