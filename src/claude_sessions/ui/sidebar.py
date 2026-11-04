"""The sidebar column: mascot, sessions in groups, background runs, and the statistics."""

import time
from dataclasses import dataclass

from rich.style import Style
from rich.text import Text

from claude_sessions.core import state
from claude_sessions.ui import words
from claude_sessions.ui.words import STYLES

WIDTH = 29
CONTENT = 27
# Quota bars leave room for the label, " 100%" and the reset time, as `5h ███ 11% 19:00`
BAR_CELLS = CONTENT - 3 - 5 - 6
HEAD = 4  # mascot (3) and a blank line
# Lines of one session entry, not counting the rule between entries
ENTRY = 2
# The spend chart: one column per day of the week, CHART_ROWS tall, with its weekday under it
CHART_ROWS = 4
COLUMN = 2
GAP = 2
DAY_NAMES = ("Mo", "Tu", "We", "Th", "Fr", "Sa", "Su")
BLOCKS = " ▁▂▃▄▅▆▇█"
BOLD = Style(bold=True)
# The colour of a status: its waiting time, and the heading of its group
COLOURS = {
    state.PERMISSION: "red",
    state.INPUT: "yellow",
    state.DONE: "green",
    state.WORKING: "blue",
    state.SHELL: "cyan",
}
# The background of a session's two lines, by status; a working session's breathes (`tint`)
TINTS = {
    state.PERMISSION: "#3a1f23",
    state.INPUT: "#38311a",
    state.DONE: "#1d3324",
    state.SHELL: "#1a3335",
}
WORKING_DIM, WORKING_BRIGHT = (20, 24, 31), (29, 48, 78)
# How much lighter the current session's tint is, per colour channel
LIFT = 18
# What the second line says before the cost, for the statuses that wait on something
DETAIL = {state.PERMISSION: "approve", state.INPUT: "question", state.SHELL: "running"}
# The sessions' groups, in the order they are listed: heading, statuses and heading colour
GROUPS = (
    ("NEEDS YOU", state.NEEDS_YOU, "red"),
    ("RUNNING", (state.WORKING, state.SHELL), "blue"),
    ("DONE", (state.DONE,), "green"),
    ("IDLE", (state.IDLE,), "soft"),
)


@dataclass
class Entry:
    """One session window as the sidebar lists it."""

    window_id: str
    name: str
    status: str
    since: float
    cost: float | None = None
    # How long Claude has spent answering, in seconds
    duration: float | None = None
    model: str | None = None


@dataclass
class Stats:
    """What the bottom of the sidebar shows: today, the quota and the week's spend."""

    limits: dict
    today: float
    # `(date, dollars)` for each of the last 7 days, oldest first, today last
    week: list
    # What today's journal adds up to (a `usage.Day`); None leaves the TODAY block out
    day: object = None


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


def tint(status, current, now):
    """The background style of a session's lines: its status's colour, breathing if working.

    The current session's is lighter, so it stands out among the others.
    """
    if status == state.WORKING:
        k = words.pulse(now)
        rgb = [round(d + (b - d) * k) for d, b in zip(WORKING_DIM, WORKING_BRIGHT, strict=True)]
    elif status in TINTS:
        rgb = [int(TINTS[status][i : i + 2], 16) for i in (1, 3, 5)]
    else:
        return words.CURRENT if current else None
    if current:
        rgb = [min(255, c + LIFT) for c in rgb]
    return Style(bgcolor="#%02x%02x%02x" % tuple(rgb))


def entry_lines(entry, current, now):
    """The two lines of a session entry, `WIDTH` cells each.

    No mark: the background's colour tells the status. The first line has the name and the
    waiting time; the second what the session waits on, its cost, Claude's time and the model.
    """
    base = tint(entry.status, current, now)
    idle = entry.status == state.IDLE
    name_style = STYLES["strong"] if current else STYLES["soft" if idle else "ink"]
    label = words.waiting_label(now - entry.since) if entry.status in state.TIMED else ""

    first = _line(base)
    # The name takes whatever the waiting time leaves
    room = CONTENT - (len(label) + 1 if label else 0)
    first.append(words.fit(entry.name, room), style=name_style)
    if label:
        first.append(" " * (CONTENT - (first.cell_len - 1) - len(label)))
        first.append(label, style=STYLES[COLOURS.get(entry.status, "soft")] + BOLD)

    parts = [DETAIL.get(entry.status), words.money(entry.cost), words.span(entry.duration)]
    detail = " · ".join(p for p in parts if p)
    # The model comes last, and only whole: a cut name says nothing
    if entry.model and len(detail) + (3 if detail else 0) + len(entry.model) <= CONTENT:
        detail = f"{detail} · {entry.model}" if detail else entry.model
    second = _line(base)
    second.append(words.fit(detail, CONTENT), style=STYLES["soft"])
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


def _rule():
    line = _line()
    line.append("─" * CONTENT, style=STYLES["rule"])
    return line


def _heading(title, value=""):
    """A block's heading: its name in grey, and its total in white on the right."""
    line = _line()
    line.append(title, style=STYLES["soft"])
    if value:
        line.append(" " * (CONTENT - len(title) - len(value)))
        line.append(value, style=STYLES["strong"])
    return line


def _reset(resets_at, now):
    """When a quota resets, in five cells at most: the time within a day, else the weekday."""
    if resets_at - now < 86400:
        return time.strftime("%H:%M", time.localtime(resets_at))
    return words.WEEKDAYS[time.localtime(resets_at).tm_wday]


def today_lines(day, spent):
    """TODAY: its spend, activity by hour, lines changed, time waiting and sessions."""
    top = max(day.hours) or 1
    strip = _line()
    for count in day.hours:
        # An hour with something at least shows the thinnest block
        strip.append(BLOCKS[max(1, round(count / top * 8))] if count else " ", STYLES["blue"])
    axis = _line()
    axis.append("0     6     12    18", style=STYLES["faint"])
    changed = _line()
    changed.append("lines ", style=STYLES["soft"])
    changed.append(f"+{words.thousands(day.added)}", style=STYLES["green"])
    changed.append(f" −{words.thousands(day.removed)}", style=STYLES["red"])
    counts = _line()
    counts.append("waited ", style=STYLES["soft"])
    counts.append(words.span(day.waited) or "0m")
    counts.append(" · sessions ", style=STYLES["soft"])
    counts.append(str(day.sessions))
    return [_rule(), _heading("TODAY", words.money(spent)), strip, axis, changed, counts]


def usage_lines(limits, now):
    """USAGE: the 5-hour and 7-day quota bars, with when each resets; none without quota."""
    lines = []
    for label, key in (("5h", "five_hour"), ("7d", "seven_day")):
        quota = limits.get(key)
        if not quota:
            continue
        line = _line()
        line.append(f"{label} ", style=STYLES["soft"])
        line.append_text(words.bar(quota["pct"], BAR_CELLS))
        line.append(f"{round(quota['pct'])}%".rjust(5))
        line.append(" " + _reset(quota["resets_at"], now), style=STYLES["faint"])
        lines.append(line)
    return [_rule(), _heading("USAGE"), *lines] if lines else []


def stats_lines(stats, now, today=True):
    """The statistics at the bottom: TODAY, USAGE and SPEND 7D, each under a rule.

    Args:
        stats: What to show.
        now: Current time as a Unix timestamp.
        today: Whether to show the TODAY block; it goes first when room is short.
    """
    lines = today_lines(stats.day, stats.today) if today and stats.day is not None else []
    lines += usage_lines(stats.limits, now)
    lines += [_rule(), *chart_lines(stats.week)]
    return [_finish(line) for line in lines]


def chart_lines(week):
    """Draws the week's spend: a heading with the total, a column per day, and the weekdays.

    Args:
        week: `(date, dollars)` for each day, oldest first, today last.

    Returns:
        The heading, `CHART_ROWS` rows of columns scaled to the most expensive day, and the
        weekday names, as lines without the right padding.
    """
    heading = _heading("SPEND 7D", words.money(sum(v for _, v in week)))
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


def plan(entries, room):
    """Lays the sessions out in groups, as `(kind, value)` rows, in `room` lines at most.

    The kinds are `blank`, `heading` (the group's name and colour), `rule`, `first` and `second`
    (the two lines of an entry, which is the value). An entry that does not fit, and all after
    it, are left out.
    """
    rows = []
    known = {status for _, statuses, _ in GROUPS for status in statuses}
    for title, statuses, colour in GROUPS:
        # A status the groups do not name is listed with the idle ones, never dropped
        others = title == GROUPS[-1][0]
        members = [e for e in entries if e.status in statuses or others and e.status not in known]
        for i, entry in enumerate(members):
            if i:
                before = [("rule", None)]
            else:
                before = [("blank", None)] if rows else []
                before.append(("heading", (title, colour)))
            if len(rows) + len(before) + ENTRY > room:
                return rows
            rows += [*before, ("first", entry), ("second", entry)]
    return rows


def bottom_lines(entries, stats, now, height):
    """The statistics that fit under the sessions: TODAY goes when the sessions need its room."""
    full = stats_lines(stats, now)
    if len(plan(entries, height - HEAD - len(full))) == len(plan(entries, height)):
        return full
    return stats_lines(stats, now, today=False)


def entry_at(entries, y, height, stats):
    """The entry drawn on row `y` of a `height`-line column, or None."""
    rows = plan(entries, height - HEAD - len(bottom_lines(entries, stats, 0.0, height)))
    if y < HEAD or y - HEAD >= len(rows):
        return None
    kind, value = rows[y - HEAD]
    return value if kind in ("first", "second") else None


def column(entries, current, stats, now, height, runs=()):
    """The whole column: exactly `height` lines of `WIDTH` cells.

    Sessions come first, in groups, then background runs; the statistics sit at the bottom.
    """
    pad = (CONTENT - len(words.MASCOT[0])) // 2
    head = []
    for row in words.MASCOT:
        line = _line()
        line.append(" " * pad + row, style=STYLES["mascot"])
        head.append(_finish(line))
    head.append(_finish(_line()))

    bottom = bottom_lines(entries, stats, now, height)
    body = []
    for kind, value in plan(entries, height - HEAD - len(bottom)):
        if kind == "first":
            body += entry_lines(value, value.window_id == current, now)
        elif kind == "heading":
            line = _line()
            line.append(value[0], style=STYLES[value[1]])
            body.append(_finish(line))
        elif kind == "rule":
            body.append(_finish(_rule()))
        elif kind == "blank":
            body.append(_finish(_line()))
    body += runs_lines(runs, now, height - HEAD - len(body) - len(bottom))
    filler = [_finish(_line()) for _ in range(max(0, height - HEAD - len(body) - len(bottom)))]
    # A terminal shorter than the stats keeps the bottom of them
    return (head + body + filler + bottom)[-height:]
