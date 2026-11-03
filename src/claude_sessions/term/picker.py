"""What the new-session picker lists and previews."""

import os
import subprocess
import time

from claude_sessions.claude import files

COMMITS = 3
# The id on the line that starts a new session instead of resuming one
NEW = "+"
PREVIEW_SESSIONS = 3
DAY = 86400


def ago(seconds):
    """Says how long ago something was, as `5m ago`, `2h ago` or `3d ago`."""
    if seconds < 3600:
        return f"{max(1, int(seconds // 60))}m ago"
    if seconds < DAY:
        return f"{int(seconds // 3600)}h ago"
    return f"{int(seconds // DAY)}d ago"


def sessions(path, now=None):
    """Returns a folder's earlier sessions, newest first, as `(id, title, seconds ago)`.

    Args:
        path: The folder; `~` stands for the home folder.
        now: Current time as a Unix timestamp; by default the clock's.
    """
    path = os.path.expanduser(path).rstrip("/")
    now = time.time() if now is None else now
    return [
        (s.id, s.first_prompt, now - s.last)
        for s in files.load_history()
        if s.cwd.rstrip("/") == path and s.first_prompt
    ]


def session_lines(path):
    """Returns the lines of the second picker: a new session, then the earlier ones.

    Each line is `id<TAB>text`; an empty list when the folder has no earlier session.
    """
    found = sessions(path)
    if not found:
        return []
    lines = [f"{NEW}\t+ new session"]
    lines += [f"{sid}\t↺ {title}  {ago(seconds)}" for sid, title, seconds in found]
    return lines


def _git(path, *args):
    out = subprocess.run(["git", "-C", path, *args], capture_output=True, text=True, timeout=3)
    return out.stdout.strip() if out.returncode == 0 else None


def _git_lines(path):
    """The branch, the changed files and the last commits of a folder, as lines."""
    try:
        branch = _git(path, "branch", "--show-current")
        if branch is None:
            return ["not a git repository"]
        changed = len((_git(path, "status", "--porcelain") or "").splitlines())
        commits = _git(path, "log", f"-{COMMITS}", "--format=%h %s") or ""
    except (OSError, subprocess.SubprocessError):
        return ["not a git repository"]
    return [f"branch   {branch or '(detached)'}", f"{changed} changed", "", commits]


def preview(path):
    """Returns the text beside a folder in the picker: git state and the latest sessions.

    Args:
        path: The folder; `~` stands for the home folder.
    """
    path = os.path.expanduser(path)
    lines = _git_lines(path)
    earlier = sessions(path)[:PREVIEW_SESSIONS]
    if earlier:
        lines += ["", "sessions"]
        lines += [f"↺ {title}  {ago(seconds)}" for _, title, seconds in earlier]
    return "\n".join(lines)
