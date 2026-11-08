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
# How often a hidden sidebar checks whether its window is back on screen, in seconds; tmux
# also wakes it with F12 when the window shows (see tmux.conf)
HIDDEN_POLL = 1.0


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
    entries = sidebar.load_entries(records)
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
    stats = sidebar.Stats(statusline.load_limits(now), totals.today, totals.days, _reader.day(now))
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
        self.loaded_at = None
        self.visible = True
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
        self.tick_once(now)
        if not self.visible:
            # Hidden: only watch for the window coming back, to draw it at once when it does
            delay = HIDDEN_POLL
        else:
            # The next frame while a session works, so its spinner turns; else the next second
            busy = any(e.status == state.WORKING for e in self.entries)
            delay = words.next_tick(now, words.FPS if busy else 1)
        self.set_timer(delay, self.tick)

    def tick_once(self, now):
        """Reads the sessions once a second and redraws while the window is on screen.

        Closes the window once the sidebar is alone in it.
        """
        fields = tmux.run(
            "display-message", "-p", "-t", self.window_id, "#{window_panes} #{window_active}"
        ).split()
        if fields[:1] == ["1"]:
            tmux.close_window(self.window_id)
            self.exit()
            return
        # A failed tmux call counts as on screen: better a wasted draw than a stale one
        self.visible = fields[1:] != ["0"]
        if not self.visible:
            # Read again as soon as the window shows
            self.loaded_at = None
            return
        # Between two readings only the spinner moves: redraw with the last data
        if int(now) == self.loaded_at:
            self.redraw(now)
            return
        self.loaded_at = int(now)
        # A failed load leaves the last data, redrawn with the current time
        try:
            self.entries, self.stats, self.runs = self.load(now)
        except Exception as e:  # noqa: BLE001 - a file caught mid-change must not stop the sidebar
            self.load_error = self.log_once(e, self.load_error)
        self.redraw(now)

    def redraw(self, now):
        """Draws the column; a failed draw keeps the last screen."""
        try:
            self.draw(now)
        except Exception as e:  # noqa: BLE001 - a bad entry must not stop the sidebar
            self.draw_error = self.log_once(e, self.draw_error)

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

    def on_key(self, event):
        # tmux sends F12 when this window comes on screen: draw now, not at the next tick
        if event.key == "f12":
            event.stop()
            self.loaded_at = None
            self.tick_once(time.time())

    def on_click(self, event: Click):
        event.stop()
        entry = sidebar.entry_at(self.entries, event.y, self.size.height, self.stats)
        if entry:
            tmux.run("select-window", "-t", entry.window_id)


def main():
    SidebarApp().run()
