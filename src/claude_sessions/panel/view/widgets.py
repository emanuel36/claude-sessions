"""The panel's widgets: the quota, the sections of sessions, and the spend."""

from rich.text import Text
from textual.containers import VerticalScroll
from textual.widget import Widget

from claude_sessions.panel.view import text
from claude_sessions.panel.view.layout import COLOURS, MARGIN, label_line, layout, row_line

# The gauges and bars of the quota and spend sections, in cells
QUOTA_CELLS = 12
SPEND_NAME = 22


class RowList(Widget):
    """A section of the list: a label, then one line per session."""

    DEFAULT_CSS = "RowList { height: auto; }"

    def __init__(self, label, data=False, **kwargs):
        super().__init__(**kwargs)
        self.label = label
        # Whether the rows have data columns (open sessions) rather than a folder (recent ones)
        self.data = data
        self.rows = []
        self.selected = None
        self.now = 0.0

    def show(self, rows, selected, now):
        """Shows these rows, the selected one highlighted; hidden when there are none."""
        self.rows, self.selected, self.now = rows, selected, now
        self.display = bool(rows)
        self.refresh(layout=True)

    def get_content_height(self, container, viewport, width):
        return 1 + len(self.rows)

    def render(self):
        width = self.size.width
        rights = [text.status_cell(row, self.now)[2] for row in self.rows]
        lay = layout(width, rights, self.data)
        lines = [label_line(self.label, lay, width)]
        for row in self.rows:
            lines.append(row_line(row, lay, width, self.now, row.session_id == self.selected))
        return Text("\n", end="").join(lines)

    def on_click(self, event):
        # A click only selects: a stray click must not open a session; Enter does that
        index = event.y - 1
        if 0 <= index < len(self.rows):
            self.app.select_row(self.rows[index].session_id)


class Quota(Widget):
    """The usage windows in use as gauges, with when each resets and when the 5h one fills."""

    DEFAULT_CSS = "Quota { height: auto; }"
    WINDOWS = (("five_hour", "5h"), ("seven_day", "7d"))

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.limits, self.pace, self.now = {}, {}, 0.0

    def show(self, limits, pace, now):
        """Shows the windows that are known; hidden when there are none."""
        self.limits, self.pace, self.now = limits, pace, now
        self.display = bool(limits)
        self.refresh(layout=True)

    def get_content_height(self, container, viewport, width):
        return sum(name in self.limits for name, _ in self.WINDOWS)

    def render(self):
        lines = []
        for name, label in self.WINDOWS:
            window = self.limits.get(name)
            if not window:
                continue
            pct, colour = window["pct"], COLOURS[text.limit_colour(window["pct"])]
            line = Text(" " * MARGIN + label + "  ", COLOURS["soft"], end="")
            line.append(text.limit_bar(pct, QUOTA_CELLS), colour)
            line.append(f"{round(pct)}%".rjust(5), colour)
            line.append("  " + text.resets(window["resets_at"], self.now), COLOURS["soft"])
            fills = self.pace.get(name)
            # Only worth saying when the window fills before it resets
            if fills is not None and fills < window["resets_at"] - self.now:
                line.append("  " + text.pace_text(fills), COLOURS["yellow"])
            line.no_wrap = True
            line.truncate(self.size.width)
            lines.append(line)
        return Text("\n", end="").join(lines)


class Spend(Widget):
    """What each project cost over the last days: a bar and an amount per project."""

    DEFAULT_CSS = "Spend { height: auto; }"

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.spend = []

    def show(self, spend):
        """Shows `[(project, dollars)]`, largest first; hidden when empty."""
        self.spend = spend
        self.display = bool(spend)
        self.refresh(layout=True)

    def get_content_height(self, container, viewport, width):
        return 1 + len(self.spend)

    def render(self):
        top = max((amount for _, amount in self.spend), default=0) or 1
        lines = [Text(" " * MARGIN + "Spend 7d by project", COLOURS["soft"])]
        for name, amount in self.spend:
            line = Text(" " * MARGIN)
            line.append(text.fit(name, SPEND_NAME - 1).ljust(SPEND_NAME), COLOURS["light"])
            line.append(text.limit_bar(amount * 100 / top, QUOTA_CELLS), COLOURS["blue"])
            line.append("  " + text.money(amount), COLOURS["soft"])
            line.no_wrap = True
            line.truncate(self.size.width)
            lines.append(line)
        return Text("\n", end="").join(lines)


class Body(VerticalScroll, can_focus=False):
    """The scrolling part, between the header and the keys; keys move the cursor, not it."""
