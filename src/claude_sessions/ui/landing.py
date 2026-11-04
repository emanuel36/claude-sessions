"""The landing page's text: the left column, the right column and the screen around them."""

from dataclasses import dataclass

from rich.text import Text

from claude_sessions.ui import words
from claude_sessions.ui.words import STYLES

LEFT_WIDTH = 66
# The widest right row: a quota line with a weekday reset, `7d ... 31%  resets Fri 09:00`
RIGHT_WIDTH = 43
GAP = 4
FOOTER = "↑↓ choose · Enter resume · n new session · / search · q quit"
BLOCKS = " ▁▂▃▄▅▆▇█"
OTHER = "other folder…"


@dataclass
class Recent:
    """A session to resume."""

    session_id: str
    project: str
    title: str
    cost: float | None
    last: float
    cwd: str


@dataclass
class Folder:
    """A folder to start a session in; `changed` is None outside a git repository."""

    path: str
    branch: str
    changed: int | None


def _line(*parts):
    """A line from (text, style name) pairs."""
    out = Text()
    for text, style in parts:
        out.append(text, style=STYLES[style])
    return out


def _heading(label, hint=None):
    parts = [(label, "soft")]
    return [Text(), _line(*parts, *([("  " + hint, "faint")] if hint else []))]


def _git_state(changed):
    if changed is None:
        return ""
    if changed == 0:
        return "clean"
    return f"{changed} changed"


def left_lines(recent, folders, selected, search, now):
    """The mascot, the recent sessions and the folders for a new session.

    Args:
        recent: `Recent` rows, newest first.
        folders: `Folder` rows.
        selected: Index of the selected row: recent rows, then folders, then `other folder…`.
        search: The search text, or None when not searching.
        now: Current time as a Unix timestamp.
    """
    lines = [_line((row, "mascot")) for row in words.MASCOT[:2]]
    lines.append(_line((words.MASCOT[2] + "  ", "mascot"), ("claude-sessions", "strong")))
    lines += _heading("Recent sessions", "/ " + (search if search is not None else "search"))
    for i, r in enumerate(recent):
        name = "strong" if i == selected else "ink"
        lines.append(
            _line(
                ("› " if i == selected else "  ", name),
                (r.project[:16].ljust(17), name),
                (words.fit(r.title, 30).ljust(31), "soft"),
                (words.money(r.cost).ljust(6), "strong"),
                (" " + words.ago(r.last, now), "faint"),
            )
        )
    lines += _heading("New session in…", "n")
    width = max((len(f.path) for f in folders), default=0) + 2
    for j, f in enumerate(folders):
        on = len(recent) + j == selected
        parts = [("› " if on else "  ", "strong"), (f.path.ljust(width), "strong" if on else "ink")]
        if f.changed is not None:
            parts += [(words.fit(f.branch, 10).ljust(11), "cyan"), (_git_state(f.changed), "faint")]
        lines.append(_line(*parts))
    on = selected == len(recent) + len(folders)
    lines.append(_line(("› " if on else "  ", "strong"), (OTHER, "strong" if on else "ink")))
    return lines


def right_lines(limits, totals, activity, first_day, now):
    """Usage, spend per project, sessions per day and the week's lines.

    Args:
        limits: Quota readings as `{"five_hour": {"pct", "resets_at"}, "seven_day": ...}`.
        totals: The `Totals` of the last 7 days.
        activity: Sessions per day, oldest first.
        first_day: The date of the first activity day.
        now: Current time as a Unix timestamp.
    """
    lines = _heading("Usage")
    for label, key in (("5h", "five_hour"), ("7d", "seven_day")):
        limit = limits.get(key)
        if not limit:
            continue
        pct = round(limit["pct"])
        line = _line((label.ljust(5), "soft"))
        line.append_text(words.bar(pct, 16))
        line.append(f" {pct}%".rjust(4), style=STYLES["ink"])
        line.append("  resets " + words.reset_at(limit["resets_at"], now), style=STYLES["faint"])
        lines.append(line)
    lines.append(
        _line(
            ("today ", "soft"),
            (words.money(totals.today), "strong"),
            ("   7 days ", "soft"),
            (words.money(totals.week), "strong"),
        )
    )
    lines += _heading("Spend 7d by project")
    top = max((amount for _, amount in totals.by_project), default=0)
    for name, amount in totals.by_project:
        line = _line((name[:16].ljust(17), "ink"))
        line.append_text(words.bar(amount * 100 / top if top else 0, 14, STYLES["blue"]))
        line.append(" " + words.money(amount), style=STYLES["strong"])
        lines.append(line)
    lines += _heading("Sessions per day (14 days)")
    blocks = [BLOCKS[min(8, n + 2)] if n else " " for n in activity]
    lines.append(_line((" ".join(blocks), "green")))
    width = 2 * len(activity) - 1
    start = words.short_date(first_day)
    lines.append(_line((start + "today".rjust(width - len(start)), "faint")))
    lines += _heading("Week")
    lines.append(
        _line(
            (f"+{words.thousands(totals.added)}", "green"),
            (" ", "ink"),
            (f"-{words.thousands(totals.removed)}", "red"),
            (f" lines in {totals.sessions} sessions", "soft"),
        )
    )
    return lines


def _pad(line, width):
    out = line.copy()
    out.pad_right(max(0, width - out.cell_len))
    return out


def screen(left, right, width):
    """The two columns side by side, or stacked when the terminal is too narrow, and the footer."""
    if width >= LEFT_WIDTH + GAP + RIGHT_WIDTH:
        rows = max(len(left), len(right))
        left = left + [Text()] * (rows - len(left))
        right = right + [Text()] * (rows - len(right))
        body = []
        for a, b in zip(left, right, strict=True):
            row = _pad(a, LEFT_WIDTH + GAP)
            row.append_text(b)
            body.append(row)
    else:
        body = left + right
    return body + [Text(), _line((FOOTER, "faint"))]
