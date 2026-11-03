import json
import os
import time

from claude_sessions.claude import files, statusline
from claude_sessions.core import journal, paths, state, store
from claude_sessions.panel.data import loader
from claude_sessions.term.tmux import Window

NOW = 1_000_000.0


def window(window_id, session_id, status, since=NOW):
    lead = {"id": session_id, "status": status, "status_since": since, "cwd": f"/w/{session_id}"}
    return Window(window_id, {**lead, "mux": {"pane": "%" + window_id[1:]}})


def live(session_id, **fields):
    pid = os.getpid()
    store.write(
        {
            **state.new_record(session_id),
            "pid": pid,
            "pid_start": store.process_start(pid),
            "cwd": f"/w/{session_id}",
            **fields,
        }
    )


def history(*entries):
    os.makedirs(paths.config_dir(), exist_ok=True)
    with open(files.history_path(), "w") as f:
        for session_id, project, t, display in entries:
            line = {"sessionId": session_id, "project": project, "timestamp": t * 1000}
            f.write(json.dumps({**line, "display": display}) + "\n")


def with_windows(monkeypatch, windows, tabs):
    monkeypatch.setattr(loader.tmux, "windows_of", lambda records: windows)
    monkeypatch.setattr(loader.tmux, "window_tabs", lambda: tabs)


def test_open_sorted_by_tab(monkeypatch):
    windows = [
        window("@5", "b", state.PERMISSION),
        window("@3", "a", state.WORKING),
        window("@9", "c", state.WORKING),
    ]
    with_windows(monkeypatch, windows, {"@3": "3", "@5": "5", "@9": "12"})
    snap = loader.PanelData().load(NOW)
    assert [(r.session_id, r.tab) for r in snap.open] == [("a", "3"), ("b", "5"), ("c", "12")]
    assert snap.counts == {state.PERMISSION: 1, state.WORKING: 2}


def test_open_title_falls_back_to_first_prompt_then_new_session(monkeypatch):
    history(("a", "/w/a", 10, "fix the login"))
    with_windows(monkeypatch, [window("@2", "a", state.IDLE), window("@3", "b", state.IDLE)], {})
    snap = loader.PanelData().load(NOW)
    assert [r.title for r in snap.open] == ["fix the login", "New session"]


def test_recent_skips_open_and_command_only_sessions(monkeypatch):
    history(
        ("open", "/w/a", 50, "still open"),
        ("cmd", "/w/a", 40, "/resume"),
        ("old", "/w/b", 30, "an older one"),
        ("away", "/w/c", 20, "in another terminal"),
    )
    live("away")
    with_windows(monkeypatch, [window("@2", "open", state.IDLE)], {"@2": "2"})
    snap = loader.PanelData().load(NOW)
    assert [(r.session_id, r.title, r.elsewhere) for r in snap.recent] == [
        ("old", "an older one", False),
        ("away", "in another terminal", True),
    ]


def test_a_split_pane_on_this_server_is_open_here_not_elsewhere(monkeypatch):
    monkeypatch.setenv("TMUX", "/s/claude,1,0")
    history(
        ("lead", "/w/a", 50, "the lead"),
        ("split", "/w/a", 40, "beside it"),
        ("away", "/w/b", 30, "on another server"),
    )
    live("lead", mux={"socket": "/s/claude", "pane": "%2"})
    live("split", mux={"socket": "/s/claude", "pane": "%5"})
    live("away", mux={"socket": "/s/other", "pane": "%2"})
    with_windows(monkeypatch, [window("@2", "lead", state.IDLE)], {"@2": "2"})
    now = time.mktime((*time.localtime()[:3], 12, 0, 0, 0, 0, -1))
    for sid, cost in (("lead", 1.0), ("split", 2.0), ("away", 4.0)):
        journal.append({"t": now - 60, "id": sid, "cost": cost})
    snap = loader.PanelData().load(now)
    assert [(r.session_id, r.elsewhere) for r in snap.recent] == [("away", True)]
    assert snap.cost_open == 3.0


def test_recent_keeps_at_most_recent_rows(monkeypatch):
    history(*((f"s{i}", "/w", i, "hi") for i in range(loader.RECENT + 5)))
    with_windows(monkeypatch, [], {})
    assert len(loader.PanelData().load(NOW).recent) == loader.RECENT


def test_filter_rows_matches_all_words_in_title_or_folder():
    rows = [
        loader.Row("1", "Fix the API timeout", "/w/client"),
        loader.Row("2", "Weekly notes", "/w/notes"),
        loader.Row("3", "Review", "/w/api-gateway"),
    ]
    assert [r.session_id for r in loader.filter_rows(rows, "api")] == ["1", "3"]
    assert [r.session_id for r in loader.filter_rows(rows, "API fix")] == ["1"]
    assert [r.session_id for r in loader.filter_rows(rows, "")] == ["1", "2", "3"]


def test_open_rows_carry_usage_and_snapshot_the_limits(monkeypatch):
    spent = {"cost": 4.12, "duration": 6000.0, "model": "Opus", "added": 1, "removed": 2}
    lead = window("@2", "a", state.IDLE)
    lead.lead["usage"] = spent
    with_windows(monkeypatch, [lead, window("@3", "b", state.IDLE)], {})
    statusline.save_limits({"five_hour": {"pct": 64, "resets_at": NOW + 60}}, NOW)
    snap = loader.PanelData().load(NOW)
    assert [r.usage for r in snap.open] == [spent, {}]
    assert snap.limits == {"five_hour": {"pct": 64, "resets_at": NOW + 60}}
    assert loader.PanelData().load(NOW + 60).limits == {}


def test_snapshot_sums_cost_and_waiting_of_today(monkeypatch):
    with_windows(monkeypatch, [window("@2", "a", state.PERMISSION)], {})
    # The journal keeps only recent days: today at noon
    now = time.mktime((*time.localtime()[:3], 12, 0, 0, 0, 0, -1))
    for event in (
        {"t": now - 86400, "id": "a", "cost": 9.0},
        {"t": now - 300, "id": "a", "cost": 1.25},
        {"t": now - 200, "id": "gone", "cost": 2.0},
        {"t": now - 120, "id": "a", "status": state.PERMISSION},
        # Its record vanished without an `ended` event: the wait stops at its last event
        {"t": now - 100, "id": "gone", "status": state.PERMISSION},
    ):
        journal.append(event)
    snap = loader.PanelData().load(now)
    assert (snap.cost_today, snap.cost_open, snap.waited) == (3.25, 1.25, 120.0)
