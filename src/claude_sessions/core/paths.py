"""Where everything lives on disk, how files are written there, and the error log.

Paths are resolved on each call, so the variables can change while the process runs (tests
point them at a temporary folder).
"""

import contextlib
import json
import os
import stat
import tempfile
import time
import traceback

# The account's usage limits share the records' directory, but are not a session record
LIMITS_FILE = "limits.json"
ERROR_LOG = "hook-errors.log"
LOG_BYTES = 1024 * 1024


def state_dir():
    """Returns the directory that holds the session records, creating it if needed.

    Raises:
        OSError: Without XDG_RUNTIME_DIR, the folder in the shared temporary folder is a
            symlink, belongs to another user, or others can write to it: someone else made it
            first.
    """
    base = os.environ.get("CLAUDE_SESSIONS_DIR")
    if base:
        os.makedirs(base, mode=0o700, exist_ok=True)
        return base
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime:
        base = os.path.join(runtime, "claude-sessions")
        os.makedirs(base, mode=0o700, exist_ok=True)
        return base
    base = os.path.join(tempfile.gettempdir(), f"claude-sessions-{os.getuid()}")
    os.makedirs(base, mode=0o700, exist_ok=True)
    # lstat: a symlink planted there would lead the records into a folder someone else picked
    st = os.lstat(base)
    if (
        not stat.S_ISDIR(st.st_mode)
        or st.st_uid != os.getuid()
        or st.st_mode & (stat.S_IWGRP | stat.S_IWOTH)
    ):
        raise OSError(f"{base} is not a private folder of this user")
    return base


def state_home():
    """Returns the directory for what outlives a reboot, such as the saved status line."""
    base = os.environ.get("CLAUDE_SESSIONS_STATE")
    if base:
        return base
    xdg = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
    return os.path.join(xdg, "claude-sessions")


def config_dir():
    """Returns the folder where Claude Code keeps its files."""
    return os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")


def write_text(path, text):
    """Writes a file atomically, so readers never see a partial one.

    A failed write leaves no temporary file behind.
    """
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path), prefix=".tmp-")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(text)
        os.replace(tmp, path)
    except BaseException:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(tmp)
        raise


def write_json(path, value):
    """Writes JSON atomically, so readers never see a partial file."""
    write_text(path, json.dumps(value))


def log_error():
    """Appends the exception being handled to the error log; never raises.

    The log starts over once it passes LOG_BYTES, so it cannot grow without end.
    """
    try:
        path = os.path.join(state_dir(), ERROR_LOG)
        full = os.path.exists(path) and os.path.getsize(path) > LOG_BYTES
        # A symlink planted in its place must not make the hook write somewhere else
        flags = os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW | (os.O_TRUNC if full else os.O_APPEND)
        with os.fdopen(os.open(path, flags, 0o600), "w") as log:
            log.write(f"{time.strftime('%F %T')} {traceback.format_exc()}\n")
    except OSError:
        pass
