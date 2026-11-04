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


def test_removing_a_session_logs_its_end(monkeypatch):
    start = noon()
    monkeypatch.setattr(store.time, "time", lambda: start + 60)
    store.write({**state.new_record("s1"), "status": state.PERMISSION, "status_since": start})
    store.remove("s1")
    store.remove("s1")
    assert journal.read_day(datetime.date.fromtimestamp(start)) == [
        {"t": start, "id": "s1", "status": state.PERMISSION},
        {"t": start + 60, "id": "s1", "status": journal.ENDED},
    ]


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


def test_spend_counts_only_cost_added_in_the_window():
    day1, day2 = datetime.date(2026, 1, 1), datetime.date(2026, 1, 2)
    days = {
        day1: [{"t": 1.0, "id": "s", "cost": 3.0}],
        day2: [{"t": 2.0, "id": "s", "cost": 4.0}, {"t": 3.0, "id": "s", "cost": 5.0}],
    }
    assert journal.spend_by_session(days, first_day=day2) == {"s": 2.0}


def test_spend_of_a_session_born_in_the_window_is_its_highest_cost():
    day = datetime.date(2026, 1, 2)
    days = {day: [{"t": 1.0, "id": "s", "cost": 1.5}, {"t": 2.0, "id": "s", "status": "idle"}]}
    assert journal.spend_by_session(days, first_day=day) == {"s": 1.5}


def test_window_totals_grow_from_before_the_window():
    day1, day2 = datetime.date(2026, 1, 1), datetime.date(2026, 1, 2)
    days = {
        day1: [{"t": 1.0, "id": "a", "cost": 1.0, "added": 5, "removed": 1}],
        day2: [
            {"t": 2.0, "id": "a", "cost": 3.0, "added": 25, "removed": 4},
            {"t": 3.0, "id": "b", "cost": 0.5},
        ],
    }
    assert journal.window_totals(days, first_day=day2) == {
        "a": {"cost": 2.0, "added": 20, "removed": 3},
        "b": {"cost": 0.5, "added": 0, "removed": 0},
    }


def test_day_costs_counts_each_days_growth():
    d0, d1, d2 = (datetime.date(2026, 11, n) for n in (1, 2, 3))
    events = {
        d0: [{"id": "a", "cost": 1.0}],
        d1: [{"id": "a", "cost": 2.5}, {"id": "b", "cost": 0.5}],
        d2: [{"id": "a", "cost": 4.0}, {"id": "b", "cost": 0.5}],
    }
    assert journal.day_costs(events) == {d0: 1.0, d1: 2.0, d2: 1.5}
