"""The landing page: what `cst` opens with, and what it returns to when no session is left.

Runs as `cs landing` in the first window, which stays for the life of the server: closing the
last session brings the user back to it.
"""

import time
from dataclasses import dataclass, field

from claude_sessions.claude import files, hook, statusline
from claude_sessions.term import tmux
from claude_sessions.ui import folders, landing, new_panel, sidebar, usage, words
from claude_sessions.ui.window_app import WindowApp

_reader = usage.Reader()
# How often the hidden page checks whether it is back on screen, in seconds; tmux also wakes
# it with F12 when its window shows (see tmux.conf)
HIDDEN_POLL = 1.0


@dataclass
class Page:
    """What the landing page shows at a moment."""

    folders: list = field(default_factory=list)
    stats: sidebar.Stats = field(default_factory=lambda: sidebar.Stats({}, 0.0, []))
    opened: int = 0


def load(now):
    """Returns the landing page's contents at a moment.

    Args:
        now: Current time as a Unix timestamp.
    """
    history = files.load_history(stored=True)
    totals = _reader.totals(history, now)
    live = hook.live_sessions(now)
    limits = statusline.load_limits(now)
    day, hours = _reader.day(now), _reader.hours(now)
    stats = sidebar.Stats(limits, totals.today, totals.days, day, totals, hours)
    return Page(
        # A running `claude -p` is no session to resume either, though it is not counted open
        folders.load(history, totals.per_session, {r["id"] for r in live}),
        stats,
        sum(1 for r in live if not r.get("headless")),
    )


class LandingApp(WindowApp):
    """The landing page, redrawn every second while it is on screen."""

    def __init__(self, load=load, window_id=None):
        super().__init__(load, window_id)
        self.page = Page()
        self.picker = new_panel.Picker()
        self.searching = False
        self.now = time.time()

    def tick(self):
        if not self.is_running:
            return
        now = time.time()
        shown = self.tick_once(now)
        # Hidden, it only watches for coming back, to draw at once when it does
        self.set_timer(words.next_tick(now) if shown else HIDDEN_POLL, self.tick)

    def tick_once(self, now):
        """Reads the page's data and redraws, unless another window is the one on screen.

        Returns:
            Whether the page is on screen.
        """
        self.now = now
        # Hidden behind a session, it has nothing to show: skip the reading and the git calls
        if tmux.run("display-message", "-p", "-t", self.window_id, "#{window_active}") == "0":
            return False
        # A failed load leaves the last data, redrawn with the current time
        page = self.read(now)
        if page is not None:
            self.page = page
            self.picker.folders = page.folders
        self.redraw()
        return True

    def draw(self):
        lines = landing.screen(
            self.picker,
            self.searching,
            self.page.opened,
            self.page.stats,
            self.now,
            self.size.width,
            self.size.height,
        )
        self.show(lines)

    def on_key(self, event):
        event.stop()
        event.prevent_default()
        key = event.key
        # tmux sends F12 when this window comes on screen: draw now, not at the next tick
        if key == "f12":
            self.tick_once(time.time())
            return
        if self.searching:
            if key in ("enter", "escape"):
                self.searching = False
                if key == "escape":
                    self.picker.query = ""
            else:
                self.picker.key(key, event.character if event.is_printable else None)
        elif key == "slash":
            self.searching = True
        elif key == "q":
            tmux.run("kill-server")
        elif key == "escape":
            self.picker.query = ""
        elif key in ("up", "down", "left", "right", "enter"):
            row = self.picker.key(key)
            if row is not None:
                tmux.run("new-window", *new_panel.command(row))
        self.redraw()


def main():
    LandingApp().run()
