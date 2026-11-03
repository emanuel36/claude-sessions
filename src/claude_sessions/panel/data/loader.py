"""What the panel shows: the open sessions, then the recent ones, without any drawing."""

import datetime
import os
from collections import Counter
from dataclasses import dataclass, field

from claude_sessions.claude import files, hook, statusline
from claude_sessions.core import journal
from claude_sessions.term import tmux

RECENT = 50
NEW_SESSION = "New session"


@dataclass
class Row:
    """A line of the list: an open session (with its window) or a recent one."""

    session_id: str
    title: str
    cwd: str
    window: tmux.Window | None = None
    tab: str = ""
    last: float = 0.0
    # Running on another tmux server, or outside tmux
    elsewhere: bool = False
    # What the session's status line reports (cost, model, lines), once known
    usage: dict = field(default_factory=dict)


@dataclass
class Snapshot:
    """Everything the panel draws at one moment."""

    open: list[Row]
    recent: list[Row]
    # Open sessions per status
    counts: dict[str, int]
    # The account's usage windows that have not reset, as statusline.load_limits gives them
    limits: dict = field(default_factory=dict)
    # Spent today, in all sessions and in the open ones, from the journal
    cost_today: float = 0.0
    cost_open: float = 0.0
    # Seconds sessions spent asking for permission or input today
    waited: float = 0.0
    # Today's bars: when the first open session started, now, and each open session's title,
    # id and journal events (the app cuts them into cells to fit its width); None when
    # there is nothing to draw yet
    timeline: tuple[float, float, list[tuple[str, str, list[dict]]]] | None = None


class PanelData:
    """Reads the live sessions and Claude Code's history into snapshots for the panel."""

    def __init__(self):
        self.titles = files.Titles()
        self.recent, self.recent_key = [], None
        # The parsed history is a new list each time the file changes
        self.history = None
        self.journal = journal.DayReader()

    def load(self, now):
        """Returns the panel's contents at a moment.

        Args:
            now: Current time as a Unix timestamp.
        """
        history = files.load_history()
        records = hook.live_sessions(now)
        tabs = tmux.window_tabs()
        known = {s.id: s for s in history}
        opened = []
        for w in tmux.windows_of(records):
            r = w.lead
            entry = known.get(r["id"])
            title = (
                self.titles.get(r["id"], r.get("transcript"))
                or (entry.first_prompt if entry else "")
                or NEW_SESSION
            )
            opened.append(
                Row(
                    r["id"],
                    title,
                    r.get("cwd", ""),
                    w,
                    tabs.get(w.window_id, ""),
                    usage=r.get("usage") or {},
                )
            )
        opened.sort(key=lambda row: int(row.tab) if row.tab.isdigit() else 0)
        # A session in a split pane runs here too, though another one leads its window
        socket = tmux.server_socket()
        here = {row.session_id for row in opened}
        here |= {r["id"] for r in records if tmux.on_server(r, socket)}
        elsewhere = {r["id"] for r in records} - here
        # The recent list changes only with the history or with what is open
        key = (frozenset(here), frozenset(elsewhere))
        if history is not self.history or key != self.recent_key:
            self.recent = self.recent_rows(history, here, elsewhere)
            self.history, self.recent_key = history, key
        counts = Counter(row.window.status for row in opened)
        events = self.journal.read(datetime.date.fromtimestamp(now))
        return Snapshot(
            opened,
            self.recent,
            dict(counts),
            statusline.load_limits(now),
            cost_today=journal.cost_today(events),
            cost_open=journal.cost_today(events, ids=here),
            waited=journal.waited(events, now, here | elsewhere),
            timeline=self.timeline(opened, events, now),
        )

    def timeline(self, opened, events, now):
        """The span and rows of today's bars for the open sessions, or None when empty."""
        by_id = {row.session_id: [] for row in opened}
        for e in events:
            if e.get("id") in by_id:
                by_id[e["id"]].append(e)
        times = [e["t"] for session in by_id.values() for e in session]
        if not times or min(times) >= now:
            return None
        rows = [(r.title, r.session_id, by_id[r.session_id]) for r in opened]
        return min(times), now, rows

    def recent_rows(self, history, here, elsewhere):
        rows = []
        for s in history:
            if s.id in here:
                continue
            path = files.transcript_path(s.id, s.cwd)
            title = self.titles.get(s.id, path) or s.first_prompt
            # Only commands were typed (a /resume that was left): nothing to come back to
            if not title:
                continue
            rows.append(Row(s.id, title, s.cwd, last=s.last, elsewhere=s.id in elsewhere))
            if len(rows) == RECENT:
                break
        return rows


def filter_rows(rows, query):
    """Keeps the rows whose title or folder holds every word of the query."""
    words = query.lower().split()

    def matches(row):
        text = f"{row.title} {os.path.basename(row.cwd.rstrip('/'))}".lower()
        return all(word in text for word in words)

    return [r for r in rows if matches(r)]
