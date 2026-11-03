"""What the new-session picker lists and previews."""

import os
import subprocess

COMMITS = 3


def _git(path, *args):
    out = subprocess.run(["git", "-C", path, *args], capture_output=True, text=True, timeout=3)
    return out.stdout.strip() if out.returncode == 0 else None


def preview(path):
    """Returns the text beside a folder in the picker: branch, changed files, last commits.

    Args:
        path: The folder; `~` stands for the home folder.
    """
    path = os.path.expanduser(path)
    try:
        branch = _git(path, "branch", "--show-current")
        if branch is None:
            return "not a git repository"
        changed = len((_git(path, "status", "--porcelain") or "").splitlines())
        commits = _git(path, "log", f"-{COMMITS}", "--format=%h %s") or ""
    except (OSError, subprocess.SubprocessError):
        return "not a git repository"
    return "\n".join([f"branch   {branch or '(detached)'}", f"{changed} changed", "", commits])
