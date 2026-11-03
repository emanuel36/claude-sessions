"""What the panel shows: the open sessions, then the recent ones, without any drawing."""

import datetime
import os
import subprocess
import time
from collections import Counter
from dataclasses import dataclass, field

from claude_sessions.claude import files, hook, statusline
from claude_sessions.core import journal
from claude_sessions.term import tmux

RECENT = 50
NEW_SESSION = "New session"
# The usage windows' lengths in seconds, as the status line input names them
WINDOW_SECONDS = {"five_hour": 5 * 3600, "seven_day": 7 * 86400}
# The days the spend section adds up, today included
SPEND_DAYS = 7
# How long a folder's branch is trusted before git is asked again
BRANCH_SECONDS = 5
_branches = {}


def branch_of(cwd):
    """Returns the branch checked out in a folder, or "" if it is not a git repository.

    Args:
        cwd: The folder; one that is gone gives "".
    """
    now = time.monotonic()
    cached = _branches.get(cwd)
    if cached and now - cached[0] < BRANCH_SECONDS:
        return cached[1]
    try:
        out = subprocess.run(
            ["git", "-C", cwd, "branch", "--show-current"],
            capture_output=True,
            text=True,
            timeout=2,
        )
        branch = out.stdout.strip() if out.returncode == 0 else ""
    except (OSError, subprocess.SubprocessError):
        branch = ""
    _branches[cwd] = (now, branch)
    return branch


def pace(window, window_seconds, now):
    """Returns the seconds until a usage window is full at its average rate so far.

    Args:
        window: `{"pct", "resets_at"}`, as `statusline.load_limits` gives it.
        window_seconds: The length of the window.
        now: Current time as a Unix timestamp.

    Returns:
        None when nothing was used yet or the window is over.
    """
    elapsed = now - (window["resets_at"] - window_seconds)
    if window["resets_at"] <= now or elapsed <= 0 or window["pct"] <= 0:
        return None
    return (100 - window["pct"]) * elapsed / window["pct"]


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
    # The branch checked out in `cwd`; empty outside a git repository
    branch: str = ""


@dataclass
class Snapshot:
    """Everything the panel draws at one moment."""

    open: list[Row]
    recent: list[Row]
    # Open sessions per status
    counts: dict[str, int]
    # The account's usage windows that have not reset, as statusline.load_limits gives them
    limits: dict = field(default_factory=dict)
    # Seconds until each usage window is full at its average rate, as `pace` gives it
    pace: dict = field(default_factory=dict)
    # Dollars spent in the last SPEND_DAYS days per project folder, largest first
    spend: list[tuple[str, float]] = field(default_factory=list)


class PanelData:
    """Reads the live sessions and Claude Code's history into snapshots for the panel."""

    def __init__(self):
        self.titles = files.Titles()
        self.recent, self.recent_key = [], None
        # The parsed history is a new list each time the file changes
        self.history = None
        # One reader per day: past days are parsed once, today's only for its new lines
        self.journals = {}

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
                    branch=branch_of(r.get("cwd", "")),
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
        limits = statusline.load_limits(now)
        return Snapshot(
            opened,
            self.recent,
            dict(counts),
            limits,
            pace={n: pace(w, WINDOW_SECONDS[n], now) for n, w in limits.items()},
            spend=self.spend(known, now),
        )

    def spend(self, known, now):
        """The last days' cost per project folder, from the journal, largest first."""
        today = datetime.date.fromtimestamp(now)
        days = [today - datetime.timedelta(days=n) for n in range(SPEND_DAYS - 1, -1, -1)]
        # One day before the window gives the cost the sessions had already run up
        days.insert(0, days[0] - datetime.timedelta(days=1))
        self.journals = {d: self.journals.get(d) or journal.DayReader() for d in days}
        events = {d: self.journals[d].read(d) for d in days}
        totals = {}
        for sid, cost in journal.spend_by_session(events, first_day=days[1]).items():
            if sid in known and known[sid].cwd and cost > 0:
                name = os.path.basename(known[sid].cwd.rstrip("/"))
                totals[name] = totals.get(name, 0.0) + cost
        return sorted(totals.items(), key=lambda item: -item[1])

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
