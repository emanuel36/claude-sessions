"""Spend, lines and activity totals for the sidebar and the landing page."""

import datetime
import os
from dataclasses import dataclass, field

from claude_sessions.core import journal, state

WEEK_DAYS = 7


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
    week: float = 0.0
    by_project: list[tuple[str, float]] = field(default_factory=list)
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
            week=sum(t["cost"] for t in week.values()),
            by_project=sorted(projects.items(), key=lambda item: -item[1]),
            added=sum(t["added"] for t in week.values()),
            removed=sum(t["removed"] for t in week.values()),
            sessions=sum(1 for t in week.values() if any(t.values())),
            per_session={sid: t["cost"] for sid, t in week.items()},
            days=daily,
        )
        self.cached = (inputs, result)
        return result

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
