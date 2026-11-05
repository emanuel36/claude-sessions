"""The sidebar column: mascot, sessions by urgency, background runs, and the statistics.

It reads like Claude Code's own transcript: each session is a `⏺` line with a `⎿` line under it.
"""

import time
from dataclasses import dataclass

from rich.style import Style
from rich.text import Text

from claude_sessions.core import state
from claude_sessions.term import tmux
from claude_sessions.ui import words
from claude_sessions.ui.words import STYLES

WIDTH = 29
CONTENT = 27
# What a `⎿` line puts before its text, and the room left after it
ELBOW = "  ⎿  "
INDENT = " " * len(ELBOW)
DETAIL_ROOM = CONTENT - len(ELBOW)
# Quota bars leave room for the label, the percent and the reset time: `5h ████ 58% 19:00`
BAR_CELLS = DETAIL_ROOM - 3 - 5 - 6
# A blank line, the mascot (3) and a blank line: the mascot level with Claude Code's own
HEAD = 5
# Lines of one session entry, not counting the blank line between entries
ENTRY = 2
DAY_NAMES = ("Mo", "Tu", "We", "Th", "Fr", "Sa", "Su")
BLOCKS = " ▁▂▃▄▅▆▇█"
BULLET = "⏺"
# Claude Code's spinner, played forwards and back, one frame per redraw
SPINNER = "·✢✳✶✻✽✻✶✳✢"
BOLD = Style(bold=True)
# The colour of a status: its bullet and its waiting time
COLOURS = {
    state.PERMISSION: "error",
    state.INPUT: "warning",
    state.DONE: "success",
    state.WORKING: "claude",
    state.SHELL: "permission",
}
# What the `⎿` line says before the cost
DETAIL = {
    state.PERMISSION: "approve",
    state.INPUT: "question",
    state.SHELL: "running",
    state.WORKING: "working",
    state.DONE: "done",
}
# The order sessions are listed in, most urgent first; finished, idle and any other status
# share the last place, so looking at a finished session (which makes it idle) never moves it
ORDER = (state.NEEDS_YOU, (state.WORKING, state.SHELL))


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


def load_entries(records):
    """The session windows of this tmux server, in window order, from the live records.

    A window with no live session (the landing page) is left out.
    """
    heads = {w.window_id: w for w in tmux.windows_of(records)}
    entries = []
    for window_id, _, name in tmux.window_entries():
        window = heads.get(window_id)
        if window is None:
            continue
        # The most urgent session sets the status; the first Claude pane's shows the details
        record, lead = window.head, window.lead
        u = record.get("usage") or {}
        entries.append(
            Entry(
                window_id,
                name,
                lead["status"],
                state.since(lead),
                u.get("cost"),
                u.get("api_duration"),
                u.get("model"),
            )
        )
    return entries


def _line(base=None):
    """A line starting with the left padding column."""
    return Text(" ", style=base or "")


def _finish(text):
    """Pads a line to the full width, so a background reaches both edges."""
    text.pad_right(WIDTH - text.cell_len)
    return text


def _right(line, value, style):
    """Ends a line with `value` against the right edge."""
    line.append(" " * (CONTENT - (line.cell_len - 1) - len(value)))
    line.append(value, style=style)


def _bullet(mark, mark_style, title, title_style, value="", value_style=None, base=None):
    """A `⏺` line: the mark, a title cut to fit, and a value on the right."""
    line = _line(base)
    line.append(mark + " ", style=mark_style)
    room = CONTENT - 2 - (len(value) + 1 if value else 0)
    line.append(words.fit(title, room), style=title_style)
    if value:
        _right(line, value, value_style)
    return line


def _elbow(first=True, base=None):
    """The start of a `⎿` line, or of the lines after it, which only indent."""
    line = _line(base)
    line.append(ELBOW if first else INDENT, style=STYLES["subtle"])
    return line


def spinner(now):
    """The spinner's frame at a moment."""
    return SPINNER[int(now * words.FPS) % len(SPINNER)]


def ordered(entries):
    """The entries in the order the sidebar lists them: by urgency, then by window."""
    known = {status for statuses in ORDER for status in statuses}
    out = []
    for statuses in ORDER:
        out += [e for e in entries if e.status in statuses]
    return out + [e for e in entries if e.status not in known]


def step(entries, current, offset):
    """The window `offset` places from `current` in the order the sidebar lists them.

    Wraps around at either end; from a window it does not list (the landing page), down goes
    to the first and up to the last. None when it lists nothing.
    """
    order = [e.window_id for e in ordered(entries)]
    if not order:
        return None
    if current not in order:
        return order[0 if offset > 0 else -1]
    return order[(order.index(current) + offset) % len(order)]


def entry_lines(entry, current, now):
    """The two lines of a session entry, `WIDTH` cells each.

    The first has the status's bullet (the spinner while Claude works), the name and the
    waiting time; the `⎿` line what the session waits on, its cost, Claude's time and the model.
    The current session's lines have a lighter background.
    """
    base = words.CURRENT if current else None
    colour = STYLES[COLOURS.get(entry.status, "inactive")]
    mark = spinner(now) if entry.status == state.WORKING else BULLET
    idle = entry.status not in COLOURS
    name_style = STYLES["strong"] if current else STYLES["inactive" if idle else "ink"]
    label = words.waiting_label(now - entry.since) if entry.status in state.TIMED else ""
    first = _bullet(mark, colour, entry.name, name_style, label, colour + BOLD, base)

    parts = [DETAIL.get(entry.status), words.money(entry.cost), words.span(entry.duration)]
    detail = " · ".join(p for p in parts if p)
    # The model comes last, and only whole: a cut name says nothing
    if entry.model and len(detail) + (3 if detail else 0) + len(entry.model) <= DETAIL_ROOM:
        detail = f"{detail} · {entry.model}" if detail else entry.model
    second = _elbow(base=base)
    second.append(words.fit(detail, DETAIL_ROOM), style=STYLES["inactive"])
    return [_finish(first), _finish(second)]


def runs_lines(runs, now, room):
    """A blank line, the `Background` bullet and one line per run, in `room` lines at most.

    Runs that do not fit are left out; with no room for a run there is no section at all.
    """
    shown = runs[: max(0, room - 2)]
    if not shown:
        return []
    lines = [_line(), _bullet(BULLET, STYLES["magenta"], "Background", STYLES["strong"])]
    for i, run in enumerate(shown):
        label = words.waiting_label(now - run.started)
        line = _elbow(first=not i)
        room = DETAIL_ROOM - (len(label) + 1 if label else 0)
        line.append(words.fit(run.name, room), style=STYLES["inactive"])
        if label:
            _right(line, label, STYLES["magenta"])
        lines.append(line)
    return [_finish(line) for line in lines]


def _block(title, value=""):
    """A statistics block's `⏺` line: its name, and its total on the right."""
    return _bullet(BULLET, STYLES["claude"], title, STYLES["strong"], value, STYLES["strong"])


def _reset(resets_at, now):
    """When a quota resets, in five cells at most: the time within a day, else the weekday."""
    if resets_at - now < 86400:
        return time.strftime("%H:%M", time.localtime(resets_at))
    return words.WEEKDAYS[time.localtime(resets_at).tm_wday]


def today_lines(day, spent):
    """Today: its spend, the lines changed, the sessions and the time spent waiting."""
    changed = _elbow()
    changed.append(f"+{words.thousands(day.added)}", style=STYLES["success"])
    changed.append(f" −{words.thousands(day.removed)}", style=STYLES["error"])
    changed.append(" lines", style=STYLES["inactive"])
    changed.truncate(1 + CONTENT)
    counts = _elbow(first=False)
    plural = "s" * (day.sessions != 1)
    waited = words.span(day.waited) or "0m"
    said = f"{day.sessions} session{plural} · {waited} wait"
    counts.append(words.fit(said, DETAIL_ROOM), style=STYLES["inactive"])
    return [_block("Today", words.money(spent)), changed, counts]


def usage_lines(limits, now):
    """Usage: the 5-hour and 7-day quota bars, with when each resets; none without quota."""
    lines = []
    for label, key in (("5h", "five_hour"), ("7d", "seven_day")):
        quota = limits.get(key)
        if not quota:
            continue
        line = _elbow(first=not lines)
        line.append(f"{label} ", style=STYLES["inactive"])
        line.append_text(words.bar(quota["pct"], BAR_CELLS))
        line.append(f"{round(quota['pct'])}%".rjust(5))
        line.append(" " + _reset(quota["resets_at"], now), style=STYLES["inactive"])
        lines.append(line)
    return [_block("Usage"), *lines] if lines else []


def chart_lines(week):
    """Spend 7d: the total, and a block per day scaled to the most expensive one.

    Args:
        week: `(date, dollars)` for each day, oldest first, today last.
    """
    top = max((v for _, v in week), default=0) or 1
    line = _elbow()
    for _, dollars in week:
        # A day with any spend at least shows the thinnest block
        line.append(BLOCKS[max(1, round(dollars / top * 8))] if dollars else " ", STYLES["claude"])
    if week:
        span = f"{DAY_NAMES[week[0][0].weekday()]} → {DAY_NAMES[week[-1][0].weekday()]}"
        line.append("  " + span, style=STYLES["inactive"])
    return [_block("Spend 7d", words.money(sum(v for _, v in week))), line]


def stats_lines(stats, now, today=True):
    """The statistics at the bottom, under a blank line: Today, Usage and Spend 7d.

    Args:
        stats: What to show.
        now: Current time as a Unix timestamp.
        today: Whether to show the Today block; it goes first when room is short.
    """
    lines = [_line()]
    if today and stats.day is not None:
        lines += today_lines(stats.day, stats.today)
    lines += usage_lines(stats.limits, now)
    lines += chart_lines(stats.week)
    return [_finish(line) for line in lines]


def plan(entries, room):
    """Lays the sessions out, as `(kind, value)` rows, in `room` lines at most.

    The kinds are `blank` and `first` and `second` (the two lines of an entry, which is the
    value). An entry that does not fit, and all after it, are left out.
    """
    rows = []
    for entry in ordered(entries):
        before = [("blank", None)] if rows else []
        if len(rows) + len(before) + ENTRY > room:
            break
        rows += [*before, ("first", entry), ("second", entry)]
    return rows


def bottom_lines(entries, stats, now, height):
    """The statistics that fit under the sessions: Today goes when the sessions need its room."""
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

    Sessions come first, most urgent first, then background runs; the statistics sit at the
    bottom.
    """
    pad = (CONTENT - len(words.MASCOT[0])) // 2
    head = [_finish(_line())]
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
        elif kind == "blank":
            body.append(_finish(_line()))
    body += runs_lines(runs, now, height - HEAD - len(body) - len(bottom))
    filler = [_finish(_line()) for _ in range(max(0, height - HEAD - len(body) - len(bottom)))]
    # A terminal shorter than the stats keeps the bottom of them
    return (head + body + filler + bottom)[-height:]
