"""The history of the day: status changes and costs, one JSON line per event.

Events go to `journal/YYYY-MM-DD.jsonl` under the state home, by the local date of their
time, so the panel's summary survives sessions closing and the panel restarting.
"""

import contextlib
import datetime
import json
import os
import time

from claude_sessions.core import paths, state

KEEP_DAYS = 7
# The status a session leaves the journal with when its record goes away; not a live status
ENDED = "ended"
SUFFIX = ".jsonl"


def journal_dir():
    """Returns the directory of the day files."""
    return os.path.join(paths.state_home(), "journal")


def _day_path(day):
    return os.path.join(journal_dir(), day.isoformat() + SUFFIX)


def append(event):
    """Adds an event to the file of its day, and deletes the files of past weeks.

    Never raises: the hook and the status line call it, and losing an event is better than
    breaking them.

    Args:
        event: `{"t", "id", "status"}` or `{"t", "id", "cost"}`, `t` a Unix timestamp.
    """
    try:
        path = _day_path(datetime.date.fromtimestamp(event["t"]))
        line = (json.dumps(event) + "\n").encode()
        os.makedirs(journal_dir(), mode=0o700, exist_ok=True)
        # Old files only need a look when a day starts a file, not on every event
        started = not os.path.exists(path)
        # One write on an O_APPEND fd: lines from concurrent hooks never interleave
        fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        try:
            os.write(fd, line)
        finally:
            os.close(fd)
        if started:
            today = datetime.date.fromtimestamp(time.time())
            _prune(today - datetime.timedelta(days=KEEP_DAYS))
    except (OSError, ValueError, OverflowError, TypeError):
        pass


def _prune(oldest):
    """Deletes the day files from before a date."""
    for name in os.listdir(journal_dir()):
        if not name.endswith(SUFFIX):
            continue
        try:
            day = datetime.date.fromisoformat(name.removesuffix(SUFFIX))
        except ValueError:
            continue
        if day < oldest:
            with contextlib.suppress(FileNotFoundError):
                os.unlink(os.path.join(journal_dir(), name))


def read_day(day):
    """Returns the events of a day, in the order they were written.

    Lines that do not parse (one torn by a crash mid-write) are skipped.

    Args:
        day: A `datetime.date`, in local time.
    """
    try:
        with open(_day_path(day), "rb") as f:
            return _parse(f.read())
    except FileNotFoundError:
        return []


def _parse(raw):
    events = []
    for line in raw.splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if isinstance(event, dict) and isinstance(event.get("t"), (int, float)):
            events.append(event)
    return events


class DayReader:
    """Reads a day's events again and again, parsing only the lines added since the last read.

    The panel reads the day every second; a busy day's file takes long to parse whole.
    """

    def __init__(self):
        self.file, self.offset, self.events = None, 0, []

    def read(self, day):
        """Returns the events of a day, as `read_day` does.

        A line still being written (no newline yet) waits for the next read. The reading starts
        over for another day, another file, or a file shorter than what was read.

        Args:
            day: A `datetime.date`, in local time.
        """
        try:
            with open(_day_path(day), "rb") as f:
                st = os.fstat(f.fileno())
                file = (day, st.st_dev, st.st_ino)
                if file != self.file or st.st_size < self.offset:
                    self.file, self.offset, self.events = file, 0, []
                f.seek(self.offset)
                raw = f.read()
        except FileNotFoundError:
            self.file, self.offset, self.events = None, 0, []
            return self.events
        complete = raw[: raw.rfind(b"\n") + 1]
        if complete:
            self.offset += len(complete)
            # A new list: what an earlier read returned stays as it was
            self.events = self.events + _parse(complete)
        return self.events


def cost_today(events, ids=None):
    """The sum, over sessions, of the last cost each one reported.

    Args:
        events: The day's events.
        ids: When given, only these sessions count.
    """
    last = {}
    for event in events:
        if isinstance(event.get("cost"), (int, float)):
            last[event.get("id")] = event["cost"]
    return float(sum(cost for sid, cost in last.items() if ids is None or sid in ids))


def waited(events, now, live_ids):
    """The seconds sessions spent asking for permission or for input.

    A wait ends at the session's next status change, `ended` included. One still open runs to
    `now` only while its session lives; a session that vanished without `ended` (a reboot wipes
    the records) stopped waiting at its last event. Each session counts from its first event of
    the day: a wait that began before it is not seen.

    Args:
        events: The day's events.
        now: Current time as a Unix timestamp.
        live_ids: The sessions that still run.
    """
    last = {}
    for event in events:
        last[event.get("id")] = max(event["t"], last.get(event.get("id"), event["t"]))
    changes = sorted((e for e in events if "status" in e), key=lambda e: e["t"])
    entered = {}
    total = 0.0
    for event in changes:
        sid = event.get("id")
        if sid in entered:
            total += event["t"] - entered.pop(sid)
        if event["status"] in state.NEEDS_YOU:
            entered[sid] = event["t"]
    for sid, t in entered.items():
        total += max(0.0, (now if sid in live_ids else last[sid]) - t)
    return total
