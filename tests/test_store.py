import json
import os
from pathlib import Path

import pytest

from claude_sessions.core import paths, state, store


def live_record(session_id="s1", **fields):
    pid = os.getpid()
    return {
        **state.new_record(session_id),
        "pid": pid,
        "pid_start": store.process_start(pid),
        **fields,
    }


def test_write_then_read_round_trips():
    store.write(live_record(status=state.DONE))
    assert store.read("s1")["status"] == state.DONE


def test_bad_session_ids_are_rejected():
    with pytest.raises(ValueError):
        store.read("../etc/passwd")


def test_dead_sessions_are_dropped_and_deleted():
    store.write(live_record("alive"))
    store.write({**live_record("dead"), "pid_start": -1})
    assert [r["id"] for r in store.load_all(0)] == ["alive"]
    assert store.read("dead") is None


def test_locks_left_by_ended_sessions_are_swept():
    lock = Path(paths.state_dir(), "gone.json.lock")
    lock.write_text("")
    store.load_all(os.path.getmtime(lock) + 1)
    assert lock.exists()
    store.load_all(os.path.getmtime(lock) + store.STALE_LOCK_SECONDS + 1)
    assert not lock.exists()


def test_load_all_skips_the_limits_file():
    store.write(live_record("s1"))
    with open(os.path.join(paths.state_dir(), paths.LIMITS_FILE), "w") as f:
        json.dump({"five_hour": {"pct": 1, "resets_at": 2}}, f)
    assert [r["id"] for r in store.load_all(0)] == ["s1"]
    store.remove_replaced({"id": "s2", "pid": -1, "pid_start": -1})
    assert sorted(os.listdir(paths.state_dir())) == ["limits.json", "s1.json", "s1.json.lock"]
