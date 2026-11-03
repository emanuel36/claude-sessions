"""How a section's rows are laid out: cells of text cut and padded into columns."""

import os
from dataclasses import dataclass, field

from rich.cells import cell_len
from rich.style import Style
from rich.text import Text

from claude_sessions.panel.view import text

# Colours by name, as the tab bar uses them: ANSI numbers, so they follow the terminal palette
COLOURS = {
    "ink": Style(),
    "red": Style(color="red"),
    "yellow": Style(color="yellow"),
    "green": Style(color="green"),
    "blue": Style(color="blue"),
    "soft": Style(color="color(244)"),
    "faint": Style(color="color(240)"),
    "light": Style(color="color(250)"),
}
PICKED = Style(bgcolor="color(237)")
BOLD = Style(bold=True)
MARGIN = 2
GAP = 2
PROJECT_WIDTH = 20
RIGHT_WIDTH = 24
# Narrower than this, recent rows leave out the project folder and the right column shrinks
WIDE = 80


@dataclass
class Cell:
    """A stretch of a line: its text, padded to its width on the side its alignment leaves."""

    content: str | Text
    width: int
    style: Style = field(default_factory=Style)
    align: str = "left"


def space(width):
    return Cell("", width)


def draw(cells, width, base=None):
    """Lays cells out from the left, cutting each to its width, on a line of the full width."""
    line = Text(style=base or Style(), no_wrap=True, end="")
    for cell in cells:
        content = cell.content
        content = content.copy() if isinstance(content, Text) else Text(content, cell.style)
        content.truncate(cell.width)
        pad = " " * (cell.width - content.cell_len)
        line.append_text(Text(pad) + content if cell.align == "right" else content + Text(pad))
    line.append(" " * max(0, width - line.cell_len))
    line.truncate(width)
    return line


@dataclass
class Layout:
    """Where a section's cells go: the title's width, the folder's, the right-hand text's."""

    title: int
    project: int
    right: int


def layout(width, rights, folder):
    """Fits a section's cells into a width; every row of the section shares it.

    Args:
        width: The width of the panel.
        rights: The right-hand texts of the section's rows.
        folder: Whether the rows show their project folder (recent ones) or not (open ones).
    """
    wide = width >= WIDE
    right = max([cell_len(right) for right in rights] + [RIGHT_WIDTH if wide else 0])
    project = PROJECT_WIDTH if wide and folder else 0
    fixed = 2 * MARGIN + 2 + GAP + project + right
    return Layout(max(0, width - fixed), project, right)


def row_line(row, lay, width, now, picked):
    """Draws one session row: mark, title, folder, right-hand text."""
    mark, colour, right = text.status_cell(row, now)
    title_style = COLOURS["ink"] + BOLD if row.window else COLOURS["light"]
    cells = [
        space(MARGIN),
        Cell(mark, 1, COLOURS[colour] + BOLD),
        space(1),
        Cell(text.fit(row.title, lay.title), lay.title, title_style),
    ]
    if lay.project:
        folder = text.fit(os.path.basename(row.cwd.rstrip("/")), lay.project - 1)
        cells += [space(GAP), Cell(folder, lay.project, COLOURS["soft"])]
    align = "left" if row.window else "right"
    cells += [space(GAP), Cell(right, lay.right, COLOURS[colour], align)]
    return draw(cells, width, PICKED if picked else None)


def label_line(label, lay, width):
    """Draws a section's label."""
    return draw([space(MARGIN), Cell(label, 2 + lay.title, COLOURS["soft"])], width)
