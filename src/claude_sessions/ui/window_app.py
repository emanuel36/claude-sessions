"""What the landing page and the sidebar share: a text that lives as long as its tmux window."""

import os

from rich.text import Text
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.widgets import Static

from claude_sessions.core import paths
from claude_sessions.term import tmux


class WindowApp(App):
    """A full-pane text redrawn on a timer, for as long as its tmux window lives.

    No key quits it. A failed load or draw goes to the error log once, not every time it
    repeats. Subclasses define `tick`, which `on_mount` starts, and `draw`.
    """

    ENABLE_COMMAND_PALETTE = False
    CSS = "Screen { background: transparent; } #text { width: 100%; height: 100%; }"
    BINDINGS = [
        Binding("ctrl+c", "nothing", show=False, priority=True),
        Binding("ctrl+q", "nothing", show=False, priority=True),
    ]

    def __init__(self, load, window_id=None):
        super().__init__(ansi_color=True)
        self.load = load
        self.window_id = window_id
        # The last failed load and draw that went to the error log; the same one is not logged
        # again
        self.load_error = None
        self.draw_error = None

    def compose(self) -> ComposeResult:
        yield Static(id="text")

    def on_mount(self):
        if self.window_id is None:
            self.window_id = tmux.run(
                "display-message", "-p", "-t", os.environ["TMUX_PANE"], "#{window_id}"
            )
        self.tick()

    def action_nothing(self):
        pass

    def read(self, now):
        """Returns what `load` gives at a moment, or None when it fails."""
        try:
            return self.load(now)
        except Exception as e:  # noqa: BLE001 - a file caught mid-change must not stop the app
            self.load_error = self.log_once(e, self.load_error)
            return None

    def redraw(self, *args):
        """Draws the text with `draw`; a failed draw keeps the last screen."""
        try:
            self.draw(*args)
        except Exception as e:  # noqa: BLE001 - a bad row must not stop the app
            self.draw_error = self.log_once(e, self.draw_error)

    def show(self, lines):
        """Puts the lines on screen."""
        self.query_one("#text", Static).update(Text("\n").join(lines))

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
