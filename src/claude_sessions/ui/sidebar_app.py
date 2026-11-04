"""The sidebar of a session window: the open sessions, quota and spend, in a narrow pane.

Runs as `cs sidebar` in a tmux pane; a click on a session goes to its window.
"""

import os
import time

from rich.text import Text
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.events import Click
from textual.widgets import Static

from claude_sessions.claude import files, hook, statusline
from claude_sessions.core import paths, state
from claude_sessions.term import tmux
from claude_sessions.ui import sidebar, usage, words

_reader = usage.Reader()


def load(now):
    """Returns the sidebar's contents at a moment.

    Args:
        now: Current time as a Unix timestamp.

    Returns:
        The entries in window order, the quota and spend for the bottom, and the background
        runs still going, oldest first. A window with no live session (the landing page) is
        left out.
    """
    history = files.load_history()
    records = hook.live_sessions(now)
    heads = {w.window_id: w for w in tmux.windows_of(records)}
    entries = []
    for window_id, index, name in tmux.window_entries():
        window = heads.get(window_id)
        if window is None:
            continue
        # The most urgent session sets the status; the first Claude pane's shows the details
        record, lead = window.head, window.lead
        u = record.get("usage") or {}
        # Counted from the session's start, so it grows while the session waits; the status
        # line's own duration only moves when Claude redraws it
        started = record.get("started_at")
        duration = now - started if started else u.get("duration")
        entries.append(
            sidebar.Entry(
                index,
                window_id,
                name,
                lead["status"],
                state.since(lead),
                u.get("cost"),
                duration,
            )
        )
    runs = sorted(
        (
            # Records written before runs kept their start fall back to their last change
            sidebar.Run(state.project(r), r.get("started_at") or state.since(r))
            for r in records
            # Finished, or idle between prompts: not running
            if r.get("headless") and r["status"] not in (state.DONE, state.IDLE)
        ),
        key=lambda run: run.started,
    )
    totals = _reader.totals(history, now)
    stats = sidebar.Stats(statusline.load_limits(now), totals.today, totals.days)
    return entries, stats, runs


class SidebarApp(App):
    """The sidebar, redrawn every second."""

    ENABLE_COMMAND_PALETTE = False
    CSS = "Screen { background: transparent; } #column { width: 100%; height: 100%; }"
    # The sidebar lives as long as its window: no key quits it
    BINDINGS = [
        Binding("ctrl+c", "nothing", show=False, priority=True),
        Binding("ctrl+q", "nothing", show=False, priority=True),
    ]

    def __init__(self, load=load, window_id=None):
        super().__init__(ansi_color=True)
        self.load = load
        self.window_id = window_id
        self.entries, self.stats, self.runs = [], sidebar.Stats({}, 0.0, []), []
        self.lines = []
        # The last failed load that went to the error log; the same one is not logged again
        self.load_error = None
        self.draw_error = None

    def compose(self) -> ComposeResult:
        yield Static(id="column")

    def on_mount(self):
        if self.window_id is None:
            self.window_id = tmux.run(
                "display-message", "-p", "-t", os.environ["TMUX_PANE"], "#{window_id}"
            )
        self.tick()

    def action_nothing(self):
        pass

    def tick(self):
        # A tick that lands while the app shuts down finds its widgets gone
        if not self.is_running:
            return
        now = time.time()
        # Wake on the next second, when the working mark blinks
        self.set_timer(words.next_tick(now), self.tick)
        self.tick_once(now)

    def tick_once(self, now):
        """Reads the sessions, redraws, and closes the window once the sidebar is alone in it."""
        # A failed load leaves the last data, redrawn with the current time (the mark blinks)
        try:
            self.entries, self.stats, self.runs = self.load(now)
        except Exception as e:  # noqa: BLE001 - a file caught mid-change must not stop the sidebar
            self.load_error = self.log_once(e, self.load_error)
        try:
            self.draw(now)
        except Exception as e:  # noqa: BLE001 - a bad entry must not stop the sidebar
            self.draw_error = self.log_once(e, self.draw_error)
        if tmux.run("display-message", "-p", "-t", self.window_id, "#{window_panes}") == "1":
            tmux.run("kill-window", "-t", self.window_id)
            self.exit()

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

    def draw(self, now):
        self.lines = sidebar.column(
            self.entries, self.window_id, self.stats, now, self.size.height, self.runs
        )
        self.query_one("#column", Static).update(Text("\n").join(self.lines))

    def on_click(self, event: Click):
        event.stop()
        entry = sidebar.entry_at(self.entries, event.y, self.size.height, self.stats)
        if entry:
            tmux.run("select-window", "-t", entry.window_id)


def main():
    SidebarApp().run()
