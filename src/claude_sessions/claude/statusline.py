"""`cs statusline`: what its input tells, recorded, and the user's own status line after it.

The input tells a session's cost, time, model and lines, and the account's limits. Claude
Code sends these numbers only to the status line command, as JSON on its stdin. Any of them
may be missing or null, early in a session above all.
"""

import json
import os
import re
import subprocess
import sys
import unicodedata

from claude_sessions.core import journal, paths, store
from claude_sessions.term import tmux

# The account's usage windows, as the status line input names them
WINDOWS = ("five_hour", "seven_day")
# What Claude Code's own mode line says for each permission mode; the default mode says nothing
MODES = {
    "acceptEdits": "⏵⏵ accept edits on",
    "plan": "⏸ plan mode on",
    "bypassPermissions": "⏵⏵ bypass permissions on",
}
# Columns Claude Code keeps free around the status line
EDGE = 2
ANSI = re.compile(r"\x1b\[[0-9;]*m")
# Claude Code cancels a status line that runs past its next update anyway
CHAIN_TIMEOUT = 5


def _get(data, *keys):
    """Walks nested objects, giving None where a level is missing or not an object."""
    for key in keys:
        if not isinstance(data, dict):
            return None
        data = data.get(key)
    return data


def _number(value):
    # bool is an int to Python, but never a count or an amount here
    return value if isinstance(value, int | float) and not isinstance(value, bool) else None


def _text(value):
    return value if isinstance(value, str) and value else None


def parse(data):
    """Returns a session's usage from the status line input.

    Args:
        data: The status line input.

    Returns:
        `cost` (USD) and `duration` (seconds), None until known; `model`, the display name
        or else the id, None until known; `added` and `removed` lines, 0 until known;
        `context`, the percentage of the context window in use, None until known.
    """
    duration = _number(_get(data, "cost", "total_duration_ms"))
    return {
        "cost": _number(_get(data, "cost", "total_cost_usd")),
        "duration": None if duration is None else duration / 1000,
        "model": _text(_get(data, "model", "display_name")) or _text(_get(data, "model", "id")),
        "added": int(_number(_get(data, "cost", "total_lines_added")) or 0),
        "removed": int(_number(_get(data, "cost", "total_lines_removed")) or 0),
        "context": _number(_get(data, "context_window", "used_percentage")),
    }


def parse_limits(data):
    """Returns the usage windows the status line input has, as {name: {pct, resets_at}}.

    Args:
        data: The status line input; `resets_at` there is a Unix timestamp.
    """
    limits = {}
    for name in WINDOWS:
        pct = _number(_get(data, "rate_limits", name, "used_percentage"))
        resets_at = _number(_get(data, "rate_limits", name, "resets_at"))
        if pct is not None and resets_at is not None:
            limits[name] = {"pct": pct, "resets_at": resets_at}
    return limits


def _limits_path():
    return os.path.join(paths.state_dir(), paths.LIMITS_FILE)


def load_limits(now):
    """Returns the saved usage windows that have not reset yet.

    Args:
        now: Current time as a Unix timestamp.
    """
    try:
        with open(_limits_path()) as f:
            saved = json.load(f)
    except (OSError, ValueError):
        return {}
    if not isinstance(saved, dict):
        return {}
    return {
        name: window
        for name, window in saved.items()
        if name in WINDOWS
        and isinstance(window, dict)
        and _number(window.get("pct")) is not None
        and (_number(window.get("resets_at")) or 0) > now
    }


def save_limits(limits, now):
    """Saves usage windows; each replaces the saved one of its name, the others stay.

    The limits are the account's, so every session's status line updates the same file. A
    session that has not updated its status line for a while reports an older, lower use of
    the same window: for a window that resets at the same time, the higher use stays.

    Args:
        limits: Windows as `parse_limits` returns them.
        now: Current time as a Unix timestamp.
    """
    if not limits:
        return
    saved = load_limits(now)
    for name, window in limits.items():
        old = saved.get(name)
        if old is None or old["resets_at"] != window["resets_at"] or window["pct"] > old["pct"]:
            saved[name] = window
    paths.write_json(_limits_path(), saved)


def saved_statusline_path():
    """Where `cs install` keeps the user's own `statusLine` setting, to run it in a chain."""
    return os.path.join(paths.state_home(), "statusline.json")


def saved_command():
    """Returns the command of the user's own status line, or None if there is none."""
    try:
        with open(saved_statusline_path()) as f:
            return _text(_get(json.load(f), "command"))
    except (OSError, ValueError):
        return None


def record_usage(data, now):
    """Stores the account's limits, and a session's usage on its record, if it has one."""
    save_limits(parse_limits(data), now)
    session_id = data.get("session_id")
    if not store.is_session_id(session_id):
        return
    with store.locked(session_id):
        record = store.read(session_id)
        if record is not None:
            before = (record.get("usage") or {}).get("cost")
            record["usage"] = parse(data)
            store.write(record)
            cost = record["usage"]["cost"]
            if cost is not None and cost != before:
                usage = record["usage"]
                journal.append(
                    {
                        "t": now,
                        "id": session_id,
                        "cost": cost,
                        "added": usage["added"],
                        "removed": usage["removed"],
                    }
                )


def _cells(text):
    """How many terminal columns a text takes."""
    return sum(
        0 if unicodedata.combining(ch) else 2 if unicodedata.east_asian_width(ch) in "WF" else 1
        for ch in text
    )


def pane_width():
    """Returns the width of the tmux pane the status line is drawn in, or None if unknown."""
    pane = os.environ.get("TMUX_PANE")
    width = tmux.run("display", "-p", "-t", pane, "#{pane_width}") if pane else ""
    return int(width) if width.isdigit() else None


def with_mode(line, mode, width):
    """Pads a line so the mode ends at the right edge; the line alone if there is no room.

    Args:
        line: The status line, which may hold colour codes.
        mode: The text to put on the right.
        width: The columns available.
    """
    room = width - _cells(ANSI.sub("", line)) - _cells(mode)
    return f"{line}{' ' * room}{mode}" if mode and room >= 1 else line


def add_mode(out, data):
    """Puts the session's permission mode on the right of the last line of the status line."""
    record = (
        store.read(data.get("session_id")) if store.is_session_id(data.get("session_id")) else None
    )
    mode = MODES.get((record or {}).get("mode"), "")
    width = pane_width() if mode else None
    if not mode or width is None:
        return out
    *head, last = out.rstrip("\n").split("\n")
    return "\n".join([*head, with_mode(last, mode, width - EDGE)]) + "\n"


def print_statusline(raw, data):
    """Prints the user's own status line, given the same input, or else a short default."""
    command = saved_command()
    if command is None:
        spent = parse(data)
        known = spent["model"] is not None and spent["cost"] is not None
        line = f"{spent['model']}  ${spent['cost']:.2f}" if known else ""
        sys.stdout.write(add_mode(line + "\n", data))
        return
    try:
        out = subprocess.run(
            command, shell=True, input=raw, stdout=subprocess.PIPE, timeout=CHAIN_TIMEOUT
        ).stdout
    except subprocess.TimeoutExpired:
        return
    sys.stdout.flush()
    text = out.decode(errors="replace")
    moded = add_mode(text, data)
    sys.stdout.buffer.write(out if moded == text else moded.encode())
    sys.stdout.buffer.flush()
