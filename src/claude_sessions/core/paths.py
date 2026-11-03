"""Where everything lives on disk, how files are written there, and the error log.

Paths are resolved on each call, so the variables can change while the process runs (tests
point them at a temporary folder).
"""

import json
import os
import stat
import tempfile
import time
import traceback

ERROR_LOG = "hook-errors.log"


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


def write_text(path, text):
    """Writes a file atomically, so readers never see a partial one."""
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path), prefix=".tmp-")
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(tmp, path)


def write_json(path, value):
    """Writes JSON atomically, so readers never see a partial file."""
    write_text(path, json.dumps(value))


def log_error():
    """Appends the exception being handled to the error log; never raises."""
    try:
        path = os.path.join(state_dir(), ERROR_LOG)
        # A symlink planted in its place must not make the hook write somewhere else
        flags = os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW | os.O_APPEND
        with os.fdopen(os.open(path, flags, 0o600), "w") as log:
            log.write(f"{time.strftime('%F %T')} {traceback.format_exc()}\n")
    except OSError:
        pass
