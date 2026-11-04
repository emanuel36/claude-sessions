"""Words, numbers, bars and styles the sidebar and the landing page share."""

import datetime
import math
import time
import unicodedata

from rich.style import Style
from rich.text import Text

# The shortest delay a redraw timer is given (see `next_tick`)
MIN_TICK = 0.05
# Fixed English names, not the locale's, so the screens read the same everywhere
WEEKDAYS = ("Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun")
MONTHS = ("Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec")

# Colours by name: ANSI numbers, so they follow the terminal palette
STYLES = {
    "ink": Style(),
    "red": Style(color="red"),
    "yellow": Style(color="yellow"),
    "green": Style(color="green"),
    "blue": Style(color="blue"),
    "cyan": Style(color="cyan"),
    "magenta": Style(color="magenta"),
    "soft": Style(color="color(244)"),
    "faint": Style(color="color(240)"),
    "rule": Style(color="color(238)"),
    "mascot": Style(color="color(37)"),
    "strong": Style(color="bright_white", bold=True),
}
CURRENT = Style(bgcolor="color(235)")

MASCOT = (" ▐▛███▜▌", "▝▜█████▛▘", "  ▘▘ ▝▝  ")

# A working session's tint breathes once in PULSE seconds; the screen redraws FPS times a second
PULSE = 1.4
FPS = 8
# Quota bars turn yellow, then red, from these percentages
AMBER, RED = 70, 85


def span(seconds):
    """Says a length of time the way the sidebar shows it.

    Args:
        seconds: The length, or None when unknown.

    Returns:
        `40m` under an hour, else hours and minutes as `1h12`; empty when unknown.
    """
    if seconds is None:
        return ""
    minutes = int(seconds // 60)
    if minutes < 60:
        return f"{minutes}m"
    return f"{minutes // 60}h{minutes % 60:02d}"


def time_left(until, now):
    """Says how long until a moment, such as a usage window's reset.

    Args:
        until: The moment, as a Unix timestamp.
        now: Current time as a Unix timestamp.

    Returns:
        `2h33` under a day, else days and hours as `2d 18h`; `0m` once it has passed.
    """
    seconds = max(0.0, until - now)
    if seconds < 86400:
        return span(seconds)
    return f"{int(seconds // 86400)}d {int(seconds % 86400 // 3600)}h"


def next_tick(now, per_second=1):
    """Seconds until the next redraw: the next whole second, or the next frame when animating.

    Textual drops a one-shot timer whose delay has passed by the time it starts, which ends a
    redraw loop for good; a delay that small waits for the step after instead.

    Args:
        now: Current time as a Unix timestamp.
        per_second: How many redraws a second the screen wants.
    """
    step = 1 / per_second
    delay = step - now % step
    return delay if delay >= MIN_TICK else delay + step


def pulse(now):
    """Where a breathing tint is at a moment: 0.0 at its dimmest, 1.0 at its brightest."""
    return (1 - math.cos(2 * math.pi * now / PULSE)) / 2


def waiting_label(seconds):
    """How long a session has waited, as the sidebar shows it; empty under a minute."""
    if seconds < 60:
        return ""
    if seconds < 3600:
        return f"{int(seconds // 60)}m"
    return f"{int(seconds // 3600)}h"


def money(amount):
    """An amount in dollars, as `$4.12`; empty when unknown."""
    return "" if amount is None else f"${amount:.2f}"


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


def bar_style(pct):
    """The style of a quota bar's filled cells: blue, then yellow, then red as it fills."""
    if pct >= RED:
        return STYLES["red"]
    return STYLES["yellow"] if pct >= AMBER else STYLES["blue"]


def bar(pct, cells, style=None):
    """A share of 100 as a bar of `cells` cells: filled in `style`, empty in faint."""
    filled = max(0, min(cells, round(pct * cells / 100)))
    out = Text()
    out.append("█" * filled, style=style or bar_style(pct))
    out.append("░" * (cells - filled), style=STYLES["faint"])
    return out


def reset_at(resets_at, now):
    """When a quota resets: the time if today, else the weekday and time."""
    t = time.localtime(resets_at)
    hour = time.strftime("%H:%M", t)
    if datetime.date.fromtimestamp(resets_at) == datetime.date.fromtimestamp(now):
        return hour
    return f"{WEEKDAYS[t.tm_wday]} {hour}"


def ago(timestamp, now):
    """How long ago something happened: minutes, hours, 'yesterday', the weekday, or the date."""
    seconds = now - timestamp
    if seconds < 3600:
        return f"{max(1, int(seconds // 60))}m"
    days = (datetime.date.fromtimestamp(now) - datetime.date.fromtimestamp(timestamp)).days
    if days < 1:
        return f"{int(seconds // 3600)}h"
    if days == 1:
        return "yesterday"
    if days < 7:
        return WEEKDAYS[time.localtime(timestamp).tm_wday]
    return short_date(datetime.date.fromtimestamp(timestamp))


def short_date(day):
    """A date as `25 Oct`."""
    return f"{day.day} {MONTHS[day.month - 1]}"


def thousands(n):
    """An integer with a comma between thousands, as `4,210`."""
    return f"{n:,}"
