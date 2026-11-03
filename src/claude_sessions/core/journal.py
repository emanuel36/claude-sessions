"""The history of the day: status changes and costs, one JSON line per event.

Events go to `journal/YYYY-MM-DD.jsonl` under the state home, by the local date of their
time, so the panel's summary survives sessions closing and the panel restarting.
"""

import contextlib
import datetime
import json
import os
import time

from claude_sessions.core import paths

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
            self.file, self.offset = None, 0
            # The same empty list while the file stays missing, so callers can tell nothing changed
            if self.events:
                self.events = []
            return self.events
        complete = raw[: raw.rfind(b"\n") + 1]
        if complete:
            self.offset += len(complete)
            # A new list: what an earlier read returned stays as it was
            self.events = self.events + _parse(complete)
        return self.events


def spend_by_session(day_events, first_day):
    """The cost each session added from `first_day` on.

    Costs are running totals, so a session that began earlier counts only what it grew by:
    its highest cost in the window minus its last cost before it.

    Args:
        day_events: The events of each day, by `datetime.date`.
        first_day: The first day of the window.
    """
    before, peak = {}, {}
    for day in sorted(day_events):
        for event in day_events[day]:
            cost = event.get("cost")
            if not isinstance(cost, int | float):
                continue
            sid = event.get("id")
            if day < first_day:
                before[sid] = cost
            else:
                peak[sid] = max(peak.get(sid, 0.0), cost)
    return {sid: peak[sid] - before.get(sid, 0.0) for sid in peak}
