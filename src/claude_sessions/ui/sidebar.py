"""The sidebar column: mascot, one entry per session, background runs, quota and spend."""

from dataclasses import dataclass

from rich.style import Style
from rich.text import Text

from claude_sessions.core import state
from claude_sessions.ui import words
from claude_sessions.ui.words import STYLES

WIDTH = 29
CONTENT = 27
# Quota bars leave room for the label and for " 100%"
BAR_CELLS = CONTENT - 4 - 5
HEAD = 4  # mascot (3) and a blank line
INDENT = 4
# The spend chart: one column per day of the week, CHART_ROWS tall, with its weekday under it
CHART_ROWS = 4
COLUMN = 2
GAP = 2
DAY_NAMES = ("Mo", "Tu", "We", "Th", "Fr", "Sa", "Su")
BLOCKS = " ▁▂▃▄▅▆▇█"
BOLD = Style(bold=True)
# Mark and colour by status; working sessions take their mark from the blink frame
MARKS = {
    state.PERMISSION: ("!", STYLES["red"] + BOLD),
    state.INPUT: ("?", STYLES["yellow"] + BOLD),
    state.DONE: ("✓", STYLES["green"]),
    state.SHELL: ("⧗", STYLES["cyan"]),
}


@dataclass
class Entry:
    """One session window as the sidebar lists it."""

    number: int
    window_id: str
    name: str
    status: str
    since: float
    cost: float | None = None
    # How long the session has been open, in seconds
    duration: float | None = None


@dataclass
class Stats:
    """What the bottom of the sidebar shows: quota, today's spend and the week's spend."""

    limits: dict
    today: float
    # `(date, dollars)` for each of the last 7 days, oldest first, today last
    week: list


@dataclass
class Run:
    """A Claude run without a terminal (a start hook's `claude -p`), listed while it runs."""

    name: str
    started: float


def _line(base=None):
    """A line starting with the left padding column."""
    return Text(" ", style=base or "")


def _finish(text):
    """Pads a line to the full width, so a background reaches both edges."""
    text.pad_right(WIDTH - text.cell_len)
    return text


def entry_lines(entry, current, now):
    """The two lines of a session entry, `WIDTH` cells each."""
    if entry.status == state.WORKING:
        mark, mark_style = words.spin_frame(now), STYLES["blue"] + BOLD
    else:
        mark, mark_style = MARKS.get(entry.status, (" ", STYLES["soft"]))
    base = words.CURRENT if current else None
    idle = entry.status == state.IDLE
    name_style = STYLES["strong"] if current else STYLES["soft" if idle else "ink"]
    label = words.waiting_label(now - entry.since) if entry.status in state.TIMED else ""

    first = _line(base)
    first.append(mark, style=mark_style)
    first.append(" ")
    first.append(str(entry.number), style=STYLES["faint"])
    first.append(" ")
    # The name takes whatever the mark, the number and the waiting time leave
    room = CONTENT - (first.cell_len - 1) - (len(label) + 1 if label else 0)
    first.append(words.fit(entry.name, room), style=name_style)
    if label:
        first.append(" " * (CONTENT - (first.cell_len - 1) - len(label)))
        first.append(label, style=mark_style)

    parts = [words.money(entry.cost), words.span(entry.duration)]
    second = _line(base)
    detail = words.fit(" · ".join(p for p in parts if p), CONTENT - INDENT)
    second.append(" " * INDENT + detail, style=STYLES["soft"])
    return [_finish(first), _finish(second)]


def runs_lines(runs, now, room):
    """A blank line, the `Background` heading and one line per run, in `room` lines at most.

    Runs that do not fit are left out; with no room for a run there is no section at all.
    """
    shown = runs[: max(0, room - 2)]
    if not shown:
        return []
    heading = _line()
    heading.append("Background", style=STYLES["soft"])
    lines = [_line(), heading]
    for run in shown:
        label = words.waiting_label(now - run.started)
        line = _line()
        line.append("↻ ", style=STYLES["magenta"])
        line.append(words.fit(run.name, CONTENT - 2 - (len(label) + 1 if label else 0)))
        if label:
            line.append(" " * (CONTENT - (line.cell_len - 1) - len(label)))
            line.append(label, style=STYLES["magenta"])
        lines.append(line)
    return [_finish(line) for line in lines]


def stats_lines(stats, now):
    """The separator, quota bars with the time left, today's spend and the week's chart."""
    lines = [_line()]
    lines[0].append("─" * CONTENT, style=STYLES["rule"])
    for label, key in (("5h", "five_hour"), ("7d", "seven_day")):
        quota = stats.limits.get(key)
        if not quota:
            continue
        bars = _line()
        bars.append(f"{label}  ", style=STYLES["soft"])
        bars.append_text(words.bar(quota["pct"], BAR_CELLS))
        bars.append(f" {round(quota['pct'])}%", style=STYLES["soft"])
        left = words.time_left(quota["resets_at"], now)
        reset = _line()
        reset.append(f"    resets in {left}", style=STYLES["faint"])
        lines += [bars, reset]
    spend = _line()
    spend.append("today ", style=STYLES["soft"])
    spend.append(words.money(stats.today), style=STYLES["strong"])
    lines += [spend, _line(), *chart_lines(stats.week)]
    return [_finish(line) for line in lines]


def chart_lines(week):
    """Draws the week's spend: a heading with the total, a column per day, and the weekdays.

    Args:
        week: `(date, dollars)` for each day, oldest first, today last.

    Returns:
        The heading, `CHART_ROWS` rows of columns scaled to the most expensive day, and the
        weekday names, as lines without the right padding.
    """
    total = words.money(sum(v for _, v in week))
    heading = _line()
    heading.append("spend 7d", style=STYLES["soft"])
    heading.append(" " * (CONTENT - len("spend 7d") - len(total)))
    heading.append(total, style=STYLES["strong"])
    top = max((v for _, v in week), default=0) or 1
    rows = []
    for row in range(CHART_ROWS, 0, -1):
        line = _line()
        for i, (_, dollars) in enumerate(week):
            height = dollars / top * CHART_ROWS
            if height >= row:
                cell = BLOCKS[-1]
            elif height > row - 1 and dollars > 0:
                # A partial top: eighths of a cell, at least the thinnest block
                cell = BLOCKS[max(1, round((height - row + 1) * 8))]
            else:
                cell = " "
            today = i == len(week) - 1
            line.append(" " * GAP if i else "")
            line.append(cell * COLUMN, style=STYLES["green" if today else "blue"])
        rows.append(line)
    names = _line()
    for i, (day, _) in enumerate(week):
        today = i == len(week) - 1
        names.append(" " * GAP if i else "")
        names.append(DAY_NAMES[day.weekday()], style=STYLES["strong" if today else "faint"])
    return [heading, *rows, names]


def _drawn(height, stats):
    """How many entries fit above `stats` lines of statistics in `height` lines."""
    return max(0, (height - HEAD - stats) // 2)


def entry_at(entries, y, height, stats):
    """The entry drawn on row `y` of a `height`-line column, or None."""
    index = (y - HEAD) // 2
    shown = _drawn(height, len(stats_lines(stats, 0.0)))
    if y < HEAD or index >= min(len(entries), shown):
        return None
    return entries[index]


def column(entries, current, stats, now, height, runs=()):
    """The whole column: exactly `height` lines of `WIDTH` cells.

    Sessions come first; background runs take what room is left above the statistics.
    """
    pad = (CONTENT - len(words.MASCOT[0])) // 2
    head = []
    for row in words.MASCOT:
        line = _line()
        line.append(" " * pad + row, style=STYLES["mascot"])
        head.append(_finish(line))
    head.append(_finish(_line()))

    bottom = stats_lines(stats, now)
    body = []
    for item in entries[: _drawn(height, len(bottom))]:
        body += entry_lines(item, item.window_id == current, now)
    body += runs_lines(runs, now, height - HEAD - len(body) - len(bottom))
    filler = [_finish(_line()) for _ in range(max(0, height - HEAD - len(body) - len(bottom)))]
    # A terminal shorter than the stats keeps the bottom of them
    return (head + body + filler + bottom)[-height:]
