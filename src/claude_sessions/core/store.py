"""Session records on disk: one JSON file per live Claude Code session."""

import contextlib
import fcntl
import json
import os

from claude_sessions.core import paths, state

STALE_LOCK_SECONDS = 60


def _is_record(name):
    return name.endswith(".json")


def is_session_id(value):
    """Tells whether a value can name a session record: letters, digits, `-` and `_`."""
    return isinstance(value, str) and bool(value) and all(c.isalnum() or c in "-_" for c in value)


def _path(session_id):
    if not is_session_id(session_id):
        raise ValueError(f"bad session id {session_id!r}")
    return os.path.join(paths.state_dir(), f"{session_id}.json")


@contextlib.contextmanager
def locked(session_id):
    """Serializes updates to one session; hooks for parallel tool calls can overlap."""
    with open(_path(session_id) + ".lock", "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


def read(session_id):
    """Returns the record of a session, or None if there is none."""
    try:
        with open(_path(session_id)) as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return None


def write(record):
    """Writes a record atomically, so readers never see a partial file."""
    paths.write_json(_path(record["id"]), record)


def remove(session_id, keep_lock=False):
    """Deletes a session's record and, unless a hook may still hold it, its lock."""
    with contextlib.suppress(FileNotFoundError):
        os.unlink(_path(session_id))
    if not keep_lock:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(_path(session_id) + ".lock")


def remove_replaced(record):
    """Deletes the records of sessions this one replaced in the same Claude process.

    /clear and /resume start another session in the same process; the one before it ends
    without a SessionEnd, and would otherwise linger until the process exits. The lock stays,
    as on SessionEnd: a hook of the old session may be waiting on it.
    """
    for name in os.listdir(paths.state_dir()):
        if not _is_record(name) or name[:-5] == record["id"]:
            continue
        with locked(name[:-5]):
            other = read(name[:-5])
            if other and (other.get("pid"), other.get("pid_start")) == (
                record.get("pid"),
                record.get("pid_start"),
            ):
                remove(other["id"], keep_lock=True)


def process_start(pid):
    """Returns the start time of a process from /proc, or None if it is gone."""
    try:
        with open(f"/proc/{pid}/stat") as f:
            text = f.read()
    except (OSError, TypeError):
        return None
    return int(text[text.rindex(")") + 2 :].split()[19])


def is_alive(record):
    """Tells whether the Claude process behind a record still runs (and is the same one)."""
    pid = record.get("pid")
    return pid is not None and process_start(pid) == record.get("pid_start")


def load_all(now):
    """Returns the records of all live sessions, ordered for display.

    Records of dead processes are deleted, and so are the locks ended sessions left behind.

    Args:
        now: Current time as a Unix timestamp.
    """
    records = []
    names = set(os.listdir(paths.state_dir()))
    for name in names:
        if name.endswith(".json.lock") and name[:-5] not in names:
            _drop_stale_lock(os.path.join(paths.state_dir(), name), now)
        if not _is_record(name):
            continue
        record = read(name[:-5])
        if record is None:
            continue
        if not is_alive(record):
            # A process that dies without SessionEnd (a closed window, a crash) still ends
            remove(record["id"])
            continue
        records.append(record)
    return sorted(records, key=state.sort_key)


def _drop_stale_lock(path, now):
    """Removes the lock an ended session left behind, once no hook can still be waiting."""
    with contextlib.suppress(FileNotFoundError):
        if now - os.path.getmtime(path) > STALE_LOCK_SECONDS:
            os.unlink(path)
