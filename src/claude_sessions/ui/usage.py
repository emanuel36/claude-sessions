"""Spend, lines and activity totals for the sidebar and the landing page."""

import datetime
import os
from dataclasses import dataclass, field

from claude_sessions.core import journal

WEEK_DAYS = 7


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


def activity(history, now, days=14):
    """Returns how many sessions had prompts on each of the last days, oldest first.

    Args:
        history: The sessions of `files.load_history`.
        now: Current time as a Unix timestamp.
        days: How many days to count, today last.
    """
    today = datetime.date.fromtimestamp(now)
    window = [today - datetime.timedelta(days=n) for n in range(days - 1, -1, -1)]
    return [sum(1 for s in history if d in s.days) for d in window]
