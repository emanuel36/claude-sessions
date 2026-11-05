"""Words, numbers, bars and styles the sidebar and the landing page share."""

import datetime
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
    "strong": Style(color="bright_white", bold=True),
    # Claude Code's own dark-theme colours, so the sidebar reads as part of it
    "claude": Style(color="#d77757"),
    "success": Style(color="#4eba65"),
    "error": Style(color="#ff6b80"),
    "warning": Style(color="#ffc107"),
    "permission": Style(color="#b1b9f9"),
    "inactive": Style(color="#999999"),
    "subtle": Style(color="#505050"),
}
STYLES["mascot"] = STYLES["claude"]
CURRENT = Style(bgcolor="color(235)")

MASCOT = (" ▐▛███▜▌", "▝▜█████▛▘", "  ▘▘ ▝▝  ")

# While a session works the screen redraws FPS times a second, a spinner frame each time
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
    """The style of a quota bar's filled cells: Claude's orange, then yellow, then red."""
    if pct >= RED:
        return STYLES["error"]
    return STYLES["warning"] if pct >= AMBER else STYLES["claude"]


def bar(pct, cells, style=None):
    """A share of 100 as a bar of `cells` cells: filled in `style`, empty in faint."""
    filled = max(0, min(cells, round(pct * cells / 100)))
    out = Text()
    out.append("█" * filled, style=style or bar_style(pct))
    out.append("░" * (cells - filled), style=STYLES["subtle"])
    return out


def reset_at(resets_at, now):
    """When a quota resets: the time if today, else the weekday and time."""
    t = time.localtime(resets_at)
    hour = time.strftime("%H:%M", t)
    if datetime.date.fromtimestamp(resets_at) == datetime.date.fromtimestamp(now):
        return hour
    return f"{WEEKDAYS[t.tm_wday]} {hour}"


def age(timestamp, now):
    """How long ago, in four cells at most: `5m`, `3h`, `12d` or `40w`."""
    seconds = max(0.0, now - timestamp)
    if seconds < 3600:
        return f"{max(1, int(seconds // 60))}m"
    if seconds < 86400:
        return f"{int(seconds // 3600)}h"
    if seconds < 100 * 86400:
        return f"{int(seconds // 86400)}d"
    return f"{int(seconds // (7 * 86400))}w"


def short_date(day):
    """A date as `25 Oct`."""
    return f"{day.day} {MONTHS[day.month - 1]}"


def thousands(n):
    """An integer with a comma between thousands, as `4,210`."""
    return f"{n:,}"
