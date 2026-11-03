"""The panel's words and numbers: what a row says, amounts, times, bars, terminal widths."""

import datetime
import time
import unicodedata

from claude_sessions.core import state

# Working sessions blink between these, a frame per second, as the tab bar does
SPIN = "●○"
# What the right of an open row says for each status
STATUS_WORDS = {
    state.PERMISSION: "permission",
    state.INPUT: "question",
    state.WORKING: "working",
    state.SHELL: "shell",
    state.DONE: "finished",
    state.IDLE: "idle",
}
MARKS = {
    state.PERMISSION: ("!", "red"),
    state.INPUT: ("?", "yellow"),
    state.DONE: ("✓", "green"),
    state.SHELL: ("⧗", "cyan"),
    state.IDLE: (" ", "soft"),
}
BAR_CELLS = 20
# Usage limit bars turn amber, then red, from these percentages
AMBER, RED = 70, 85
DAY = 86400


def spin_frame(now):
    """The blink frame for a moment, in step with the tab bar's (● on even seconds)."""
    return SPIN[int(now) % len(SPIN)]


def waiting_label(seconds):
    """How long a session has waited, as its tab shows it; empty under a minute."""
    if seconds < 60:
        return ""
    if seconds < 3600:
        return f"{int(seconds // 60)}m"
    return f"{int(seconds // 3600)}h"


def status_cell(row, now):
    """Returns the mark before a session, its colour name, and the text on its right."""
    w = row.window
    if not w:
        if row.elsewhere:
            return " ", "faint", "open elsewhere"
        return " ", "faint", when(row.last, now)
    if w.status == state.WORKING:
        return spin_frame(now), "blue", f"tab {row.tab}  working"
    mark, colour = MARKS.get(w.status, MARKS[state.IDLE])
    if w.status in state.TIMED:
        waited = waiting_label(now - state.since(w.lead))
        return mark, colour, f"tab {row.tab}  {STATUS_WORDS[w.status]} {waited}".rstrip()
    if w.status == state.DONE:
        return mark, colour, f"tab {row.tab}  finished"
    return mark, colour, f"tab {row.tab}"


def when(timestamp, now):
    """Says when something happened: the time today, 'yesterday', or the weekday."""
    t = time.localtime(timestamp)
    hour = time.strftime("%H:%M", t)
    days = (datetime.date.fromtimestamp(now) - datetime.date.fromtimestamp(timestamp)).days
    if days < 1:
        return hour
    if days == 1:
        return f"yesterday {hour}"
    if days < 7:
        return time.strftime("%a ", t) + hour
    return time.strftime("%d %b", t)


def money(amount):
    """An amount in dollars, as `$4.12`; empty when unknown."""
    return "" if amount is None else f"${amount:.2f}"


def duration(seconds):
    """A length of time in minutes, as `35m` or `1h40m`; empty when unknown."""
    if seconds is None:
        return ""
    minutes = int(seconds // 60)
    if minutes < 60:
        return f"{minutes}m"
    return f"{minutes // 60}h{minutes % 60:02d}m"


def lines(added, removed):
    """Lines added and removed, as `+820` and `−310`; empty when unknown."""
    return (
        "" if added is None else f"+{added}",
        "" if removed is None else f"−{removed}",
    )


def limit_bar(pct, cells=BAR_CELLS):
    """A share of 100 as a bar of filled and empty cells."""
    filled = max(0, min(cells, round(pct * cells / 100)))
    return "▰" * filled + "▱" * (cells - filled)


def pace_text(seconds):
    """Says when a usage limit fills up at the current rate; empty when unknown."""
    return "" if seconds is None else f"hits the limit in ~{duration(seconds)}"


def limit_colour(pct):
    """The colour name of a usage limit bar: grey, then amber, then red as it fills."""
    if pct >= RED:
        return "red"
    return "yellow" if pct >= AMBER else "soft"


def resets(resets_at, now):
    """When a usage limit resets: in how long, under a day away, or else the day and time."""
    if resets_at - now < DAY:
        return f"resets in {duration(resets_at - now)}"
    return time.strftime("resets %a %H:%M", time.localtime(resets_at))


def cells(ch):
    """How many terminal columns a character takes."""
    if unicodedata.combining(ch):
        return 0
    return 2 if unicodedata.east_asian_width(ch) in "WF" else 1


def fit(text, width):
    """Cuts text to a number of terminal columns, marking a cut with an ellipsis."""
    text = " ".join(str(text or "").split())
    if sum(cells(ch) for ch in text) <= width:
        return text
    out, used = [], 0
    for ch in text:
        if used + cells(ch) > width - 1:
            break
        out.append(ch)
        used += cells(ch)
    return "".join(out) + "…" if width > 0 else ""
