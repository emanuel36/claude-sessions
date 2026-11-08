"""The landing page's text: the folder list on the left, the statistics on the right."""

import time

from rich.text import Text

from claude_sessions.ui import new_panel, sidebar, words
from claude_sessions.ui.words import MONTHS, STYLES, WEEKDAYS

# Between the list and the statistics, which take the sidebar's width on the right
GAP = 4
# The folder list grows with the terminal up to this, so ages stay near the names
LIST_MAX = 72
FOOTER = (
    ("↑↓", "move"),
    ("→", "sessions"),
    ("Enter", "new / resume"),
    ("/", "search"),
    ("q", "quit"),
)


def _line(*parts):
    out = Text()
    for text, style in parts:
        out.append(text, style=STYLES[style])
    return out


def header(opened, now):
    """The mascot beside the name, today's date and how many sessions are open."""
    t = time.localtime(now)
    date = f"{WEEKDAYS[t.tm_wday]} {t.tm_mday} {MONTHS[t.tm_mon - 1]}"
    count = f"{opened} session{'s' * (opened != 1)} open"
    mascot = [row.ljust(10) for row in words.MASCOT]
    return [
        _line((" " + mascot[0], "mascot"), ("claude-sessions", "strong")),
        _line((" " + mascot[1], "mascot"), (f"{date} · {count}", "soft")),
        _line((" " + mascot[2], "mascot")),
    ]


def search_line(query, searching):
    """The search line: what was typed, or how to start typing."""
    if searching or query:
        return _line((" ❯ ", "blue"), (query, "ink"), ("▌" if searching else "", "blue"))
    return _line((" ❯ ", "blue"), ("/ to search", "faint"))


def screen(picker, searching, opened, stats, now, width, height):
    """The page as `height` lines of `width` columns.

    The header, the search line and the folder list fill the left, up to `LIST_MAX` columns;
    the wide statistics sit right after it, the two centred together. A terminal too narrow or
    short for them gets the sidebar's statistics, and one too narrow for those the list alone.
    """
    wide = width - sidebar.PANEL_WIDTH - GAP >= new_panel.WIDTH
    panel = sidebar.panel_lines(stats, now) if wide else []
    # The wide statistics also need the height: cut short, they would lose whole blocks, so
    # the spend by model, the last, goes first
    if len(panel) > height - 2:
        panel = sidebar.panel_lines(stats, now, models=False) if wide else []
    if wide and len(panel) <= height - 2:
        right, right_width = panel, sidebar.PANEL_WIDTH
    elif width - sidebar.WIDTH - GAP >= new_panel.WIDTH:
        right, right_width = sidebar.stats_lines(stats, now), sidebar.WIDTH
    else:
        right, right_width = [], 0
    list_width = min(LIST_MAX, width - right_width - GAP if right else width - 1)
    top = [*header(opened, now), Text(), search_line(picker.query, searching), Text()]
    room = max(1, height - len(top) - 2)
    shown = picker.shown()
    left = top + new_panel.list_lines(shown, picker.cursor, picker.expanded, now, room, list_width)
    left += [Text()] * (height - 2 - len(left))
    # The list and the statistics sit together in the middle of the terminal
    used = list_width + (GAP + right_width if right else 0)
    margin = Text(" " * max(0, (width - used) // 2))
    body = []
    for i, line in enumerate(left[: height - 2]):
        if i < len(right):
            line = line.copy()
            line.pad_right(list_width + GAP - line.cell_len)
            line.append_text(right[i])
        body.append(margin + line)
    keys = new_panel.keys_line(FOOTER, width - 1 - len(margin))
    return [*body, Text(), margin + keys]
