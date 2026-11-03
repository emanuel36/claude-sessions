"""The panel's widgets: the sections of sessions, and today's bars."""

import datetime

from rich.cells import cell_len
from rich.style import Style
from rich.text import Text
from textual.containers import VerticalScroll
from textual.widget import Widget

from claude_sessions.core import state
from claude_sessions.panel.data import timeline
from claude_sessions.panel.view import text
from claude_sessions.panel.view.layout import COLOURS, MARGIN, label_line, layout, row_line

# The timeline's cells: a space on this background; no background before a session began
CELLS = {
    state.WORKING: Style(bgcolor="blue"),
    state.PERMISSION: Style(bgcolor="red"),
    state.INPUT: Style(bgcolor="yellow"),
    state.DONE: Style(bgcolor="green"),
    state.IDLE: Style(bgcolor="color(238)"),
}
LEGEND = (
    (state.WORKING, "working"),
    (state.PERMISSION, "permission"),
    (state.INPUT, "question"),
    (state.DONE, "finished, not seen"),
    (state.IDLE, "idle"),
)
NAME_WIDTH = 22
HOUR_LABEL = 5


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


def hour_axis(start, end, width):
    """The hour labels for a span drawn `width` cells wide, as (column, text) pairs.

    Labels sit at whole hours, every hour that fits or else every 2, 3, 4, 6 or 12 hours.
    """
    if width <= 0 or end <= start:
        return []
    per_hour = width * 3600 / (end - start)
    step = next((n for n in (1, 2, 3, 4, 6, 12) if per_hour * n >= HOUR_LABEL + 1), 24)
    first = datetime.datetime.fromtimestamp(start).replace(minute=0, second=0, microsecond=0)
    t = first.timestamp()
    labels, taken = [], 0
    while t <= end:
        hour = datetime.datetime.fromtimestamp(t)
        col = round((t - start) * width / (end - start))
        if t >= start and hour.hour % step == 0 and taken <= col <= width - HOUR_LABEL:
            labels.append((col, f"{hour.hour:02d}:00"))
            taken = col + HOUR_LABEL + 1
        t += 3600
    return labels


def name_cell(name, style):
    """A session's name cut and padded to the name column, counting terminal cells."""
    name = text.fit(name, NAME_WIDTH)
    return Text(name + " " * (NAME_WIDTH - cell_len(name)), style)


class Timeline(Widget):
    """Today's bars: the hour axis, a bar per open session coloured by status, a legend."""

    DEFAULT_CSS = "Timeline { height: auto; }"

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.span = None

    def show(self, span):
        """Shows the bars for `(start, end, rows)`; hidden when None."""
        self.span = span
        self.display = span is not None
        self.refresh(layout=True)

    def get_content_height(self, container, viewport, width):
        return 2 + len(self.span[2]) if self.span else 0

    def render(self):
        width = self.size.width
        start, end, rows = self.span
        bar = width - 2 * MARGIN - NAME_WIDTH
        axis = Text(" " * MARGIN, COLOURS["soft"])
        axis.append_text(name_cell("today", COLOURS["soft"]))
        if bar > 0:
            canvas = [" "] * bar
            for col, label in hour_axis(start, end, bar):
                canvas[col : col + HOUR_LABEL] = label
            axis.append("".join(canvas), COLOURS["faint"])
        lines = [axis]
        cut = timeline.timeline({sid: events for _, sid, events in rows}, start, end, max(bar, 0))
        for title, sid, _ in rows:
            line = Text(" " * MARGIN)
            line.append_text(name_cell(title, COLOURS["light"]))
            for status in cut[sid]:
                line.append(" ", CELLS.get(status))
            lines.append(line)
        # From the margin, not under the bars: all five entries must fit in 80 columns
        legend = Text(" " * MARGIN)
        for i, (status, word) in enumerate(LEGEND):
            if i:
                legend.append("  ")
            legend.append("  ", CELLS[status])
            legend.append(" " + word, COLOURS["soft"])
        lines.append(legend)
        return Text("\n", end="").join(draw_cells(line, width) for line in lines)


def draw_cells(line, width):
    """Cuts a line to the width of the widget, so a narrow panel never wraps it."""
    line = line.copy()
    line.no_wrap = True
    line.truncate(width)
    return line


class Body(VerticalScroll, can_focus=False):
    """The scrolling part, between the header and the keys; keys move the cursor, not it."""
