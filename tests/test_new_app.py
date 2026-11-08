import asyncio
import os

from claude_sessions.ui import new_app
from claude_sessions.ui.new_panel import Folder, Session

NOW = 1_700_000_000.0
FOLDERS = [
    Folder(
        "/w/api", "api", NOW, 5, sessions=[Session("s1", "fix it", NOW - 60), Session("s2", "x", 0)]
    ),
    Folder("/w/web", "web", NOW - 9, 2),
]


def stub(monkeypatch):
    calls = []
    monkeypatch.setattr(new_app.tmux, "run", lambda *args: calls.append(args) or "")
    monkeypatch.setattr(os.path, "isdir", lambda p: p in ("/w/api", "/w/web"))
    return calls


def drive(keys):
    async def go():
        app = new_app.NewApp(load=lambda now: FOLDERS)
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.pause()
            await pilot.press(*keys)
            await pilot.pause()
        return app

    return asyncio.run(go())


def test_enter_on_a_folder_starts_a_session_there(monkeypatch):
    calls = stub(monkeypatch)
    drive(["down", "enter"])
    assert calls == [("new-window", "-c", "/w/web", "claude")]


def test_right_opens_the_sessions_and_enter_resumes_one(monkeypatch):
    calls = stub(monkeypatch)
    drive(["right", "down", "enter"])
    assert calls == [("new-window", "-c", "/w/api", "claude --resume s1")]


def test_left_from_a_session_goes_back_to_its_folder_and_closes_it(monkeypatch):
    stub(monkeypatch)
    app = drive(["right", "down", "down", "left"])
    assert app.picker.cursor == 0 and not app.picker.expanded


def test_typing_filters_and_a_typed_path_opens_there(monkeypatch):
    calls = stub(monkeypatch)
    drive(["w", "e", "b", "enter"])
    assert calls == [("new-window", "-c", "/w/web", "claude")]
    calls.clear()
    drive(["slash", "w", "slash", "a", "p", "i", "enter"])
    assert calls == [("new-window", "-c", "/w/api", "claude")]


def test_escape_closes_without_starting_anything(monkeypatch):
    calls = stub(monkeypatch)
    drive(["escape"])
    assert calls == []


def test_load_orders_folders_by_last_use_and_skips_gone_folders(monkeypatch, tmp_path):
    a, b = tmp_path / "a", tmp_path / "b"
    a.mkdir()
    b.mkdir()
    history = [
        files_session("1", str(a), 400, "one"),
        files_session("2", str(b), 300, "two"),
        files_session("3", str(b), 200, "three"),
        files_session("4", str(tmp_path / "gone"), 900, "lost"),
        files_session("5", str(a), 50, ""),
    ]
    monkeypatch.setattr(new_app.files, "load_history", lambda stored=False: history)
    monkeypatch.setattr(new_app.hook, "live_sessions", lambda now: [])
    monkeypatch.setattr(new_app.folders._titles, "get", lambda sid, path: "")
    folders = new_app.load(1000.0)
    assert [f.name for f in folders] == ["a", "b"]
    assert [s.title for s in folders[1].sessions] == ["two", "three"]
    assert (folders[0].count, folders[1].count) == (1, 2)


def files_session(sid, cwd, last, prompt):
    return new_app.files.HistorySession(sid, cwd, last, prompt)


def test_left_returns_to_the_folder_even_when_a_typed_path_matches_it(monkeypatch):
    stub(monkeypatch)
    app = drive(["slash", "w", "slash", "a", "p", "i", "down", "right", "down", "left"])
    assert app.picker.selected().kind == "folder" and not app.picker.expanded


def test_load_lists_a_folders_sessions_newest_first(monkeypatch, tmp_path):
    a = tmp_path / "a"
    a.mkdir()
    history = [files_session("1", str(a), 100, "old"), files_session("2", str(a), 300, "new")]
    monkeypatch.setattr(new_app.files, "load_history", lambda stored=False: history)
    monkeypatch.setattr(new_app.hook, "live_sessions", lambda now: [])
    monkeypatch.setattr(new_app.folders._titles, "get", lambda sid, path: "")
    (folder,) = new_app.load(1000.0)
    assert [s.title for s in folder.sessions] == ["new", "old"] and folder.last == 300


def test_the_folder_scan_is_kept_until_the_history_changes(monkeypatch, tmp_path):
    a = tmp_path / "a"
    a.mkdir()
    history = [files_session("1", str(a), 100, "one")]
    looked = []
    monkeypatch.setattr(new_app.folders._titles, "get", lambda sid, path: looked.append(sid) or "")
    new_app.folders.load(history, {}, set())
    new_app.folders.load(history, {"1": 2.0}, set())
    assert looked == ["1"]
    assert new_app.folders.load(history, {"1": 2.0}, set())[0].spend == 2.0
    new_app.folders.load(list(history), {}, set())
    assert looked == ["1", "1"]
