import datetime
import time

from claude_sessions.claude.files import HistorySession
from claude_sessions.core import journal
from claude_sessions.ui import usage

# Today at noon: the journal prunes days by the real clock, so a fixed date would lose old events
TODAY = datetime.date.today()
NOW = time.mktime((TODAY.year, TODAY.month, TODAY.day, 12, 0, 0, 0, 0, -1))


def ago(days):
    return TODAY - datetime.timedelta(days=days)


def session(sid="s", cwd="/w/api", days=()):
    return HistorySession(sid, cwd, days=set(days))


def log(sid, days_ago, cost, added=0, removed=0):
    t = NOW - days_ago * 86400
    journal.append({"t": t, "id": sid, "cost": cost, "added": added, "removed": removed})


def test_totals_today_week_projects_and_lines():
    log("a", 7, 1.0, 5, 1)
    log("a", 2, 2.0, 10, 2)
    log("b", 1, 0.5)
    log("a", 0, 4.0, 25, 4)
    history = [session("a", "/w/api"), session("b", "/w/web/")]
    t = usage.Reader().totals(history, NOW)
    assert (t.today, t.week) == (2.0, 3.5)
    assert t.by_project == [("api", 3.0), ("web", 0.5)]
    assert (t.added, t.removed, t.sessions) == (20, 3, 2)
    # One amount per day of the week, oldest first, today last
    assert [d for d, _ in t.days] == [ago(n) for n in range(6, -1, -1)]
    assert [v for _, v in t.days] == [0.0, 0.0, 0.0, 0.0, 1.0, 0.5, 2.0]
    assert t.today == t.days[-1][1]  # the today line and today's column agree
    assert t.per_session == {"a": 3.0, "b": 0.5}


def test_totals_are_reused_until_the_journal_grows():
    log("a", 0, 1.0)
    reader, history = usage.Reader(), [session("a")]
    first = reader.totals(history, NOW)
    assert reader.totals(history, NOW) is first
    log("a", 0, 2.0)
    assert reader.totals(history, NOW).today == 2.0


def test_day_summary_adds_up_lines_sessions_and_waiting():
    today = datetime.date(2026, 11, 4)
    at = lambda h, m=0: time.mktime((2026, 11, 4, h, m, 0, 0, 0, -1))  # noqa: E731
    events = {
        datetime.date(2026, 11, 3): [{"t": at(0) - 60, "id": "a", "cost": 1.0, "added": 10}],
        today: [
            {"t": at(9), "id": "a", "status": "input"},
            {"t": at(9, 10), "id": "a", "status": "working"},
            {"t": at(9, 20), "id": "a", "cost": 2.0, "added": 40, "removed": 5},
            {"t": at(10), "id": "b", "status": "permission"},
            {"t": at(11), "id": "old", "status": "ended"},
        ],
    }
    day = usage.day_summary(events, today, now=at(10, 30))
    # a waited 10 minutes for an answer; b is still waiting, 30 minutes so far
    assert day.waited == 600 + 1800
    # Lines count only what the sessions grew by today; an ended session is not one of today's
    assert (day.added, day.removed, day.sessions) == (30, 5, 2)
