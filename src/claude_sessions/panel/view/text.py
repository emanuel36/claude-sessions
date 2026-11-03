"""The panel's words: what a row says, waits and times, terminal widths."""

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
    state.DONE: "finished",
    state.IDLE: "idle",
}
MARKS = {
    state.PERMISSION: ("!", "red"),
    state.INPUT: ("?", "yellow"),
    state.DONE: ("✓", "green"),
    state.IDLE: (" ", "soft"),
}


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
    if w.status in state.NEEDS_YOU:
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
