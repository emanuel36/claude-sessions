"""Spend, lines and activity totals for the sidebar and the landing page."""

import datetime
import os
import time
from dataclasses import dataclass, field

from claude_sessions.core import journal, state

WEEK_DAYS = 7
HOUR = datetime.timedelta(hours=1)
# The longest a span still open at `now` counts: a session killed mid-turn writes no event
# to end it
MAX_OPEN = 2 * 3600


@dataclass
class Day:
    """What today's journal adds up to."""

    added: int = 0
    removed: int = 0
    sessions: int = 0
    # Seconds sessions spent blocked on the user, an approval or an answer
    waited: float = 0.0


@dataclass
class Totals:
    """What the journal adds up to."""

    today: float = 0.0
    by_project: list[tuple[str, float]] = field(default_factory=list)
    by_model: list[tuple[str, float]] = field(default_factory=list)
    added: int = 0
    removed: int = 0
    sessions: int = 0
    per_session: dict[str, float] = field(default_factory=dict)
    # What each of the last 7 days cost, oldest first, today last
    days: list[tuple[datetime.date, float]] = field(default_factory=list)


class Reader:
    """Reads the journal's last days, keeping the days' readers between calls."""

    def __init__(self):
        self.journals = {}
        self.cached = None

    def totals(self, history, now):
        """Returns the cost, lines and sessions of today and of the last 7 days.

        Args:
            history: The sessions of `files.load_history`, to name each one's project.
            now: Current time as a Unix timestamp.
        """
        today = datetime.date.fromtimestamp(now)
        days = [today - datetime.timedelta(days=n) for n in range(WEEK_DAYS, -1, -1)]
        self.journals = {d: self.journals.get(d) or journal.DayReader() for d in days}
        events = {d: self.journals[d].read(d) for d in days}
        # A reader hands back the same list until its file grows: nothing new, nothing to redo
        inputs = (days, history, *events.values())
        if self.cached and self.cached[0][0] == days:
            if all(a is b for a, b in zip(self.cached[0][1:], inputs[1:], strict=True)):
                return self.cached[1]
        week = journal.window_totals(events, first_day=days[1])
        cwds = {s.id: s.cwd for s in history}
        projects = {}
        for sid, t in week.items():
            if cwds.get(sid) and t["cost"] > 0:
                name = os.path.basename(cwds[sid].rstrip("/"))
                projects[name] = projects.get(name, 0.0) + t["cost"]
        daily = [(d, cost) for d, cost in journal.day_costs(events).items() if d >= days[1]]
        result = Totals(
            # From the same per-day sums as the chart, so the today line and today's column agree
            today=daily[-1][1],
            by_project=_ranked(projects),
            by_model=_ranked(journal.model_costs(events, days[1])),
            added=sum(t["added"] for t in week.values()),
            removed=sum(t["removed"] for t in week.values()),
            sessions=sum(1 for t in week.values() if any(t.values())),
            per_session={sid: t["cost"] for sid, t in week.items()},
            days=daily,
        )
        self.cached = (inputs, result)
        return result

    def hours(self, now):
        """Returns the seconds sessions worked in each hour of today, midnight first.

        Reads what `totals` read last, so call it after `totals` in the same tick.

        Args:
            now: Current time as a Unix timestamp.
        """
        events = {d: r.events for d, r in self.journals.items()}
        return working_hours(events, datetime.date.fromtimestamp(now), now)

    def day(self, now):
        """Returns today's activity, lines, sessions and waiting.

        Reads what `totals` read last, so call it after `totals` in the same tick.

        Args:
            now: Current time as a Unix timestamp.
        """
        today = datetime.date.fromtimestamp(now)
        if today not in self.journals:
            return Day()
        events = {d: r.events for d, r in self.journals.items()}
        return day_summary(events, today, now)


def _ranked(costs):
    """`(name, dollars)` pairs, most expensive first."""
    return sorted(costs.items(), key=lambda item: -item[1])


def working_hours(events, first_day, now):
    """Adds up, by hour of the day, the time sessions spent working from `first_day` on.

    A session works from a `working` event to its next status event; work that began the day
    before counts from midnight. One still working counts up to `now`, `MAX_OPEN` at most.

    Args:
        events: The events of each day, by `datetime.date`.
        first_day: The first day to count.
        now: Current time as a Unix timestamp.

    Returns:
        24 sums of seconds, midnight first.
    """
    hours = [0.0] * 24
    started = {}
    spans = []
    midnight = time.mktime(first_day.timetuple())
    for day in sorted(d for d in events if d >= first_day - datetime.timedelta(days=1)):
        for event in events[day]:
            sid, status = event.get("id"), event.get("status")
            if status is None:
                continue
            if sid in started:
                spans.append((started.pop(sid), event["t"]))
            if status == state.WORKING:
                started[sid] = event["t"]
    spans += [(t, min(now, t + MAX_OPEN)) for t in started.values()]
    for start, end in spans:
        start = max(start, midnight)
        while start < end:
            # The end of the hour `start` falls in, in local time
            t = datetime.datetime.fromtimestamp(start)
            edge = (t.replace(minute=0, second=0, microsecond=0) + HOUR).timestamp()
            stop = min(end, edge)
            hours[t.hour] += stop - start
            start = stop
    return hours


def day_summary(events, today, now):
    """Adds up one day of the journal.

    Args:
        events: The events of each day, by `datetime.date`, including the days before.
        today: The day to add up.
        now: Current time as a Unix timestamp, where a wait still going ends.
    """
    day = Day()
    seen, last = set(), {}
    for event in events.get(today, []):
        sid, status = event.get("id"), event.get("status")
        if status != journal.ENDED:
            seen.add(sid)
        if status is None:
            continue
        before = last.get(sid)
        if before and before[0] in state.NEEDS_YOU:
            day.waited += event["t"] - before[1]
        last[sid] = (status, event["t"])
    day.waited += sum(max(0.0, now - t) for status, t in last.values() if status in state.NEEDS_YOU)
    week = journal.window_totals(events, first_day=today)
    day.added = sum(t["added"] for t in week.values())
    day.removed = sum(t["removed"] for t in week.values())
    day.sessions = len(seen)
    return day
