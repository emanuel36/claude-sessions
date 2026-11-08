"""The folders Claude ran in, read from its history, for the Alt+n panel and the landing page."""

import os

from claude_sessions.claude import files
from claude_sessions.ui import new_panel

_titles = files.Titles(stored=True)
_scanned = {"history": None, "found": []}


def _scan(history):
    """The sessions worth listing, with their title and folder, read once per history.

    Finding titles stats every transcript; the landing page asks every second, so the scan is
    kept until `files.load_history` hands back a new list. The titles found go to disk, for
    the next process to start from.
    """
    if _scanned["history"] is not history:
        found = []
        for s in history:
            title = _titles.get(s.id, files.transcript_path(s.id, s.cwd)) or s.first_prompt
            # Only commands were typed: nothing to come back to
            if title and os.path.isdir(s.cwd):
                found.append((s, title, s.cwd.rstrip("/") or "/"))
        _titles.save()
        _scanned.update(history=history, found=found)
    return _scanned["found"]


def load(history, costs, opened):
    """Returns the folders that still exist, last used first, with their latest sessions.

    Args:
        history: The sessions of `files.load_history`, newest first.
        costs: Each session's spend over the last 7 days, by id.
        opened: The ids of the sessions open now; they are not offered to resume.
    """
    found = {}
    for s, title, path in _scan(history):
        folder = found.setdefault(
            path, new_panel.Folder(path, os.path.basename(path) or path, s.last)
        )
        folder.last = max(folder.last, s.last)
        folder.count += 1
        folder.spend += costs.get(s.id) or 0.0
        if s.id not in opened:
            folder.sessions.append(new_panel.Session(s.id, title, s.last, costs.get(s.id)))
    for folder in found.values():
        folder.sessions.sort(key=lambda s: -s.last)
    return sorted(found.values(), key=lambda f: -f.last)
