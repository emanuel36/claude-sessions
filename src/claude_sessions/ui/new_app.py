"""`cs new --pick`: the panel that starts a session, in the popup tmux opens for Alt+n."""

import time

from rich.text import Text
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.widgets import Static

from claude_sessions.claude import files, hook
from claude_sessions.term import tmux
from claude_sessions.ui import folders, new_panel, usage

_reader = usage.Reader()


def load(now):
    """Returns the folders Claude ran in that still exist, last used first.

    Args:
        now: Current time as a Unix timestamp.
    """
    history = files.load_history()
    costs = _reader.totals(history, now).per_session
    opened = {r["id"] for r in hook.live_sessions(now)}
    return folders.load(history, costs, opened)


class NewApp(App):
    """The panel: folders by last use, a folder's last sessions under it, typing to search."""

    ENABLE_COMMAND_PALETTE = False
    CSS = "Screen { background: transparent; } #panel { width: 100%; height: 100%; }"
    BINDINGS = [
        Binding("ctrl+c", "quit", show=False, priority=True),
        Binding("ctrl+q", "quit", show=False, priority=True),
    ]

    def __init__(self, load=load):
        super().__init__(ansi_color=True)
        self.load = load
        self.picker = new_panel.Picker()

    def compose(self) -> ComposeResult:
        yield Static(id="panel")

    def on_mount(self):
        # Reading the history takes a moment on a long one: draw the search line first
        self.redraw()
        self.set_timer(0.01, self.read_folders)

    def read_folders(self):
        self.picker.folders = self.load(time.time())
        self.redraw()

    def redraw(self):
        p = self.picker
        lines = new_panel.screen(
            p.shown(),
            p.cursor,
            p.query,
            p.expanded,
            time.time(),
            self.size.height,
        )
        self.query_one("#panel", Static).update(Text("\n").join(lines))

    def on_key(self, event):
        event.stop()
        event.prevent_default()
        if event.key == "escape":
            self.exit()
            return
        character = event.character if event.is_printable else None
        row = self.picker.key(event.key, character)
        if row is not None:
            tmux.run("new-window", *new_panel.command(row))
            self.exit()
            return
        self.redraw()


def main():
    NewApp().run()
