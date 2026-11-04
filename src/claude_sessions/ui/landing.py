"""The landing page's text: the folder list on the left, the statistics on the right."""

import time

from rich.text import Text

from claude_sessions.ui import new_panel, sidebar, words
from claude_sessions.ui.words import MONTHS, STYLES, WEEKDAYS

# Between the list and the statistics, which take the sidebar's width on the right
GAP = 4
FOOTER = "↑↓ move · → sessions · Enter new / resume · / search · q quit"


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

    The header, the search line and the folder list fill the left, as wide as the terminal
    lets them, so session titles show whole; the sidebar's statistics sit on the right. A
    terminal too narrow for both shows the list alone.
    """
    right = sidebar.stats_lines(stats, now)
    list_width = width - sidebar.WIDTH - GAP
    if list_width < new_panel.WIDTH:
        right, list_width = [], max(1, width - 1)
    top = [*header(opened, now), Text(), search_line(picker.query, searching), Text()]
    room = max(1, height - len(top) - 2)
    shown = picker.shown()
    left = top + new_panel.list_lines(shown, picker.cursor, picker.expanded, now, room, list_width)
    left += [Text()] * (height - 2 - len(left))
    body = []
    for i, line in enumerate(left[: height - 2]):
        if i < len(right):
            line = line.copy()
            line.pad_right(list_width + GAP - line.cell_len)
            line.append_text(right[i])
        body.append(line)
    return [*body, Text(), _line((" " + words.fit(FOOTER, width - 2), "faint"))]
