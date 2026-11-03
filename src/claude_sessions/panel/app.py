"""Tab 1 of the claude-sessions tmux server: the open Claude sessions, then the recent ones.

Enter goes to an open session's tab, or resumes a closed one in a new tab, in its folder.
"""

import os
import shlex
import time

from rich.text import Text
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.geometry import Region
from textual.widgets import Static

from claude_sessions.core import paths
from claude_sessions.panel.data import loader
from claude_sessions.panel.view.layout import BOLD, COLOURS, MARGIN
from claude_sessions.panel.view.widgets import Body, Quota, RowList, Spend
from claude_sessions.term import tmux

MESSAGE_SECONDS = 4
# A resumed session shows up as open once its first hook runs; until then Enter waits
RESUME_SECONDS = 10
EMPTY = "No sessions yet. Press n to start one."
NO_MATCH = "Nothing matches. Esc clears the search."


class PanelApp(App):
    """The panel: the open sessions, then the recent ones, refreshed every second."""

    CSS_PATH = "view/panel.tcss"
    ENABLE_COMMAND_PALETTE = False
    # The panel lives as long as its tab: no key quits it
    BINDINGS = [
        Binding("ctrl+c", "nothing", show=False, priority=True),
        Binding("ctrl+q", "nothing", show=False, priority=True),
    ]

    def __init__(self, data=None):
        super().__init__(ansi_color=True)
        self.data = data or loader.PanelData()
        self.snapshot = loader.Snapshot([], [], {})
        self.selected = None
        # None while not typing a search; the search text stays after Enter until Esc
        self.search = None
        self.filter_text = ""
        self.message = ("", 0.0)
        self.resumed = {}
        # The last text sent to the tab bar; None until the first one goes
        self.summary = None
        # The last failed read that went to the error log; the same one is not logged again
        self.load_error = None
        # The row the list last scrolled to; it scrolls again only when the selection moves
        self.scrolled_to = None

    def compose(self) -> ComposeResult:
        yield Quota(id="quota")
        yield Static(id="search")
        with Body(id="body"):
            yield RowList("Sessions", data=True, id="open")
            yield Spend(id="spend")
            yield RowList("Recent", id="recent")
            yield Static(id="empty")
        yield Static(id="keys")

    def on_mount(self):
        self.tick()

    def action_nothing(self):
        pass

    # data -------------------------------------------------------------------------------------

    def reload(self):
        """Reads the sessions again and redraws; a failed read leaves the last ones on screen."""
        now = time.time()
        try:
            self.snapshot = self.data.load(now)
        except Exception as e:  # noqa: BLE001 - a file caught mid-change must not stop the panel
            # Once a second, a lasting failure would fill the log
            error = f"{type(e).__name__}: {e}"
            if error != self.load_error:
                self.load_error = error
                paths.log_error()
        self.publish_summary()
        self.redraw(now)

    def publish_summary(self):
        """Tells tmux what the tab bar shows on its right; only when it changed."""
        summary = tmux.summary_text(self.snapshot.counts, self.snapshot.limits)
        if summary != self.summary:
            self.summary = summary
            tmux.publish_summary(summary)

    def tick(self):
        # A tick that lands while the app shuts down finds its widgets gone
        if not self.is_running:
            return
        # The only place that schedules: one timer chain, however often reload runs
        now = time.time()
        # Wake on the next second, when the tab bar blinks too
        self.set_timer(1 - now % 1, self.tick)
        self.reload()

    def rows(self):
        """The rows on screen, after the search."""
        return (
            loader.filter_rows(self.snapshot.open, self.filter_text),
            loader.filter_rows(self.snapshot.recent, self.filter_text),
        )

    # drawing ----------------------------------------------------------------------------------

    def redraw(self, now=None):
        now = time.time() if now is None else now
        opened, recent = self.rows()
        ids = [r.session_id for r in opened + recent]
        if self.selected not in ids:
            self.selected = ids[0] if ids else None
        self.query_one("#quota", Quota).show(self.snapshot.limits, self.snapshot.pace, now)
        search = self.query_one("#search", Static)
        search.display = self.search is not None or bool(self.filter_text)
        search.update(self.search_line())
        self.query_one("#open", RowList).show(opened, self.selected, now)
        self.query_one("#spend", Spend).show(self.snapshot.spend)
        self.query_one("#recent", RowList).show(recent, self.selected, now)
        empty = self.query_one("#empty", Static)
        empty.display = not ids
        empty.update(
            Text(" " * MARGIN + (NO_MATCH if self.filter_text else EMPTY), COLOURS["soft"])
        )
        self.query_one("#keys", Static).update(self.keys_line(now))
        if self.selected != self.scrolled_to:
            self.scrolled_to = self.selected
            self.call_after_refresh(self.scroll_to_selected)

    def search_line(self):
        line = Text(" " * MARGIN)
        line.append("/ ", COLOURS["yellow"] + BOLD)
        line.append(self.filter_text + ("▏" if self.search is not None else ""))
        return line

    def keys_line(self, now):
        message, until = self.message
        if now < until:
            return Text(" " * MARGIN + message, COLOURS["yellow"])
        if self.search is not None:
            keys = [("enter", "done"), ("esc", "clear")]
        else:
            keys = [("enter", "open"), ("n", "new session"), ("/", "search")]
        line = Text(" " * MARGIN)
        for i, (key, does) in enumerate(keys):
            line.append(("    " if i else "") + key, COLOURS["light"] + BOLD)
            line.append(" " + does, COLOURS["faint"])
        return line

    def scroll_to_selected(self):
        body = self.query_one("#body", Body)
        for section in self.query(RowList):
            ids = [r.session_id for r in section.rows]
            if self.selected in ids:
                y = section.virtual_region.y + 1 + ids.index(self.selected)
                body.scroll_to_region(Region(0, y, 1, 1), animate=False, immediate=True)

    # actions ----------------------------------------------------------------------------------

    def say(self, message):
        self.message = (message, time.time() + MESSAGE_SECONDS)

    def select_row(self, session_id):
        self.selected = session_id
        self.redraw()

    def move(self, step):
        opened, recent = self.rows()
        ids = [r.session_id for r in opened + recent]
        if not ids:
            return
        index = ids.index(self.selected) if self.selected in ids else 0
        self.selected = ids[max(0, min(len(ids) - 1, index + step))]

    def open_selected(self):
        opened, recent = self.rows()
        row = next((r for r in opened + recent if r.session_id == self.selected), None)
        if row is None:
            return
        now = time.time()
        if row.window:
            tmux.select(row.window)
        elif row.elsewhere:
            self.say("This session is already open in another terminal")
        elif now - self.resumed.get(row.session_id, 0) < RESUME_SECONDS:
            self.say("Opening it…")
        elif not os.path.isdir(row.cwd):
            self.say(f"Cannot resume: {row.cwd} is gone")
        else:
            self.resumed[row.session_id] = now
            tmux.run("new-window", "-c", row.cwd, f"claude --resume {shlex.quote(row.session_id)}")

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
                self.filter_text = self.filter_text[:-1]
            elif event.is_printable and event.character:
                self.filter_text += event.character
        elif key in ("up", "k"):
            self.move(-1)
        elif key in ("down", "j"):
            self.move(1)
        elif key == "enter":
            self.open_selected()
        elif key == "n":
            tmux.new_session_popup()
        elif key == "slash":
            self.search = ""
        elif key == "escape":
            self.filter_text = ""
        self.redraw()


def main():
    PanelApp().run()
