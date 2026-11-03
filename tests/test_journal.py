import datetime
import json
import os
import time
from pathlib import Path

from claude_sessions.core import journal, state, store


def noon():
    """Today at noon: events a minute apart stay in today's file."""
    return time.mktime((*time.localtime()[:3], 12, 0, 0, 0, 0, -1))


def day_path(day):
    return os.path.join(journal.journal_dir(), f"{day.isoformat()}.jsonl")


def test_cost_today_takes_last_per_session():
    events = [
        {"t": 1.0, "id": "a", "cost": 1.0},
        {"t": 2.0, "id": "b", "cost": 0.5},
        {"t": 3.0, "id": "a", "status": state.WORKING},
        {"t": 4.0, "id": "a", "cost": 2.25},
    ]
    assert journal.cost_today(events) == 2.75
    assert journal.cost_today(events, ids={"a"}) == 2.25
    assert journal.cost_today([]) == 0.0


def test_waited_sums_permission_and_input_until_now():
    events = [
        {"t": 100.0, "id": "a", "status": state.WORKING},
        {"t": 110.0, "id": "a", "status": state.PERMISSION},
        {"t": 115.0, "id": "a", "cost": 1.0},
        {"t": 170.0, "id": "a", "status": state.WORKING},
        {"t": 200.0, "id": "a", "status": state.INPUT},
        # Logged out of order, as concurrent hooks can: the sum sorts by time
        {"t": 150.0, "id": "b", "status": state.INPUT},
        {"t": 160.0, "id": "b", "status": state.DONE},
    ]
    assert journal.waited(events, now=230.0, live_ids={"a", "b"}) == 60 + 30 + 10


def test_waited_sums_every_wait_of_one_session():
    events = [
        {"t": 100.0, "id": "a", "status": state.PERMISSION},
        {"t": 130.0, "id": "a", "status": state.IDLE},
        {"t": 200.0, "id": "a", "status": state.PERMISSION},
        {"t": 250.0, "id": "a", "status": state.WORKING},
    ]
    assert journal.waited(events, now=300.0, live_ids={"a"}) == 30 + 50


def test_waited_ignores_time_before_the_first_event_today():
    # Waiting since yesterday: today's file starts with the end of that wait
    yesterday = {"t": 50.0, "id": "a", "status": state.PERMISSION}
    today = [{"t": 100.0, "id": "a", "status": state.WORKING}]
    assert journal.waited(today, now=200.0, live_ids={"a"}) == 0.0
    assert journal.waited([yesterday, *today], now=200.0, live_ids={"a"}) == 50.0


def test_a_wait_of_a_vanished_session_ends_at_its_last_event():
    # A reboot wipes the records without an `ended` event
    events = [
        {"t": 100.0, "id": "gone", "status": state.PERMISSION},
        {"t": 130.0, "id": "gone", "cost": 1.0},
        {"t": 100.0, "id": "live", "status": state.INPUT},
    ]
    assert journal.waited(events, now=10_000.0, live_ids={"live"}) == 30 + 9_900


def test_removing_a_waiting_session_ends_its_wait(monkeypatch):
    start = noon()
    monkeypatch.setattr(store.time, "time", lambda: start + 60)
    store.write({**state.new_record("s1"), "status": state.PERMISSION, "status_since": start})
    store.remove("s1")
    store.remove("s1")
    events = journal.read_day(datetime.date.fromtimestamp(start))
    assert events == [
        {"t": start, "id": "s1", "status": state.PERMISSION},
        {"t": start + 60, "id": "s1", "status": journal.ENDED},
    ]
    assert journal.waited(events, now=start + 1000, live_ids=set()) == 60.0


def test_read_day_skips_torn_line():
    day = datetime.date(2026, 11, 2)
    t = time.mktime((2026, 11, 2, 12, 0, 0, 0, 0, -1))
    journal.append({"t": t, "id": "a", "status": state.WORKING})
    with open(day_path(day), "a") as f:
        f.write('{"t": 1.0, "id": "a", "sta\n[1, 2]\n')
    journal.append({"t": t + 1, "id": "a", "cost": 1.5})
    assert journal.read_day(day) == [
        {"t": t, "id": "a", "status": state.WORKING},
        {"t": t + 1, "id": "a", "cost": 1.5},
    ]


def test_read_day_missing_dir_is_empty():
    assert journal.read_day(datetime.date(2026, 11, 2)) == []


def test_store_write_logs_only_status_changes():
    start = noon()
    record = {**state.new_record("s1"), "status": state.WORKING, "status_since": start}
    store.write(record)
    store.write({**record, "updated_at": start + 10})
    store.write({**record, "status": state.DONE, "status_since": start + 20})
    assert journal.read_day(datetime.date.fromtimestamp(start)) == [
        {"t": start, "id": "s1", "status": state.WORKING},
        {"t": start + 20, "id": "s1", "status": state.DONE},
    ]


def test_old_files_pruned():
    today = datetime.date.today()
    os.makedirs(journal.journal_dir())
    old, kept = today - datetime.timedelta(days=8), today - datetime.timedelta(days=7)
    for day in (old, kept, today):
        with open(day_path(day), "w") as f:
            f.write(json.dumps({"t": 1.0, "id": "a", "cost": 1.0}) + "\n")
    # Only the append that starts a file prunes
    journal.append({"t": time.time(), "id": "a", "cost": 2.0})
    assert os.path.exists(day_path(old))
    # A late event of two days ago starts a file, and the age counts from today
    journal.append({"t": time.time() - 2 * 86400, "id": "a", "cost": 2.0})
    assert not os.path.exists(day_path(old))
    assert os.path.exists(day_path(kept)) and os.path.exists(day_path(today))


def test_append_never_raises(tmp_path, monkeypatch):
    blocker = tmp_path / "file"
    blocker.write_text("")
    monkeypatch.setenv("CLAUDE_SESSIONS_STATE", str(blocker))
    journal.append({"t": time.time(), "id": "a", "cost": 1.0})
    journal.append({"t": float("inf"), "id": "a", "cost": 1.0})


def test_day_reader_parses_only_what_was_appended():
    start = noon()
    day = datetime.date.fromtimestamp(start)
    reader = journal.DayReader()
    journal.append({"t": start, "id": "a", "cost": 1.0})
    assert reader.read(day) == [{"t": start, "id": "a", "cost": 1.0}]
    # Rewritten in place at the same size: a reader that started over would see "b"
    path = Path(day_path(day))
    path.write_text(path.read_text().replace('"a"', '"b"'))
    journal.append({"t": start + 1, "id": "a", "cost": 2.0})
    assert [e["id"] for e in reader.read(day)] == ["a", "a"]


def test_day_reader_keeps_a_torn_last_line_for_the_next_read():
    start = noon()
    day = datetime.date.fromtimestamp(start)
    reader = journal.DayReader()
    journal.append({"t": start, "id": "a", "cost": 1.0})
    line = json.dumps({"t": start + 1, "id": "a", "cost": 2.0}) + "\n"
    with open(day_path(day), "a") as f:
        f.write(line[:10])
    assert len(reader.read(day)) == 1
    with open(day_path(day), "a") as f:
        f.write(line[10:])
    assert [e["cost"] for e in reader.read(day)] == [1.0, 2.0]


def test_day_reader_starts_over_on_a_new_day_or_a_shorter_file():
    start = noon()
    day = datetime.date.fromtimestamp(start)
    reader = journal.DayReader()
    journal.append({"t": start, "id": "a", "cost": 1.0})
    journal.append({"t": start + 1, "id": "a", "cost": 2.0})
    assert len(reader.read(day)) == 2
    with open(day_path(day), "w") as f:
        f.write(json.dumps({"t": start, "id": "b", "cost": 3.0}) + "\n")
    assert reader.read(day) == [{"t": start, "id": "b", "cost": 3.0}]
    assert reader.read(day + datetime.timedelta(days=1)) == []
    assert reader.read(day) == [{"t": start, "id": "b", "cost": 3.0}]
