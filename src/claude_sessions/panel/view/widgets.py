"""The panel's widgets: the sections of sessions."""

from rich.text import Text
from textual.containers import VerticalScroll
from textual.widget import Widget

from claude_sessions.panel.view import text
from claude_sessions.panel.view.layout import label_line, layout, row_line


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


class Body(VerticalScroll, can_focus=False):
    """The scrolling part, between the header and the keys; keys move the cursor, not it."""
