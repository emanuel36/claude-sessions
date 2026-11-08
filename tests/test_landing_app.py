import asyncio

from claude_sessions.ui import folders, landing_app
from claude_sessions.ui.new_panel import Folder, Session

NOW = 1_700_000_000.0
FOLDERS = [
    Folder("/w/api", "api", NOW, 1, 3.0, [Session("s1", "fix it", NOW - 60, 1.0)]),
    Folder("/w/web", "web", NOW - 9),
]
PAGE = landing_app.Page(FOLDERS, opened=1)


def stub_tmux(monkeypatch, active="1"):
    calls = []

    def run(*args):
        calls.append(args)
        return active if args[0] == "display-message" else ""

    monkeypatch.setattr(landing_app.tmux, "run", run)
    return calls


def drive(keys, load=lambda now: PAGE):
    async def go():
        app = landing_app.LandingApp(load=load, window_id="@1")
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause()
            await pilot.press(*keys)
            await pilot.pause()
        return app

    return asyncio.run(go())


def test_enter_on_a_folder_starts_a_session_there(monkeypatch):
    calls = stub_tmux(monkeypatch)
    drive(["down", "enter"])
    assert ("new-window", "-c", "/w/web", "claude") in calls


def test_right_opens_a_folders_sessions_and_enter_resumes_one(monkeypatch):
    calls = stub_tmux(monkeypatch)
    drive(["right", "down", "enter"])
    assert ("new-window", "-c", "/w/api", "claude --resume s1") in calls


def test_slash_searches_and_enter_ends_the_search(monkeypatch):
    calls = stub_tmux(monkeypatch)
    app = drive(["slash", "w", "e", "b", "enter"])
    assert app.picker.query == "web" and not app.searching
    assert not any(c[0] == "new-window" for c in calls)
    assert [r.path for r in app.picker.shown()] == ["/w/web"]


def test_q_ends_the_server_but_is_typed_while_searching(monkeypatch):
    calls = stub_tmux(monkeypatch)
    drive(["q"])
    assert ("kill-server",) in calls
    calls = stub_tmux(monkeypatch)
    app = drive(["slash", "q"])
    assert ("kill-server",) not in calls and app.picker.query == "q"


def test_never_closes_itself(monkeypatch):
    calls = stub_tmux(monkeypatch)
    drive([])
    assert not any(c[0] == "kill-window" for c in calls)


def test_does_not_read_its_data_while_another_window_is_shown(monkeypatch):
    stub_tmux(monkeypatch, active="0")
    loaded = []
    drive([], load=lambda now: loaded.append(now) or PAGE)
    assert not loaded


def test_failed_load_keeps_the_page_and_logs_once(monkeypatch):
    stub_tmux(monkeypatch)
    logged = []
    monkeypatch.setattr(landing_app.paths, "log_error", lambda: logged.append(1))

    def boom(now):
        raise ValueError("torn file")

    async def go():
        app = landing_app.LandingApp(load=boom, window_id="@1")
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause()
            app.tick_once(NOW)
            assert app.is_running

    asyncio.run(go())
    assert logged == [1]


def test_load_lists_folders_without_the_open_sessions(monkeypatch, tmp_path):
    a = tmp_path / "a"
    a.mkdir()
    history = [
        landing_app.files.HistorySession("open", str(a), 5, "now"),
        landing_app.files.HistorySession("old", str(a), 4, "before"),
    ]
    monkeypatch.setattr(landing_app.files, "load_history", lambda stored=False: history)
    monkeypatch.setattr(landing_app.hook, "live_sessions", lambda now: [{"id": "open"}])
    monkeypatch.setattr(landing_app.statusline, "load_limits", lambda now: {})
    monkeypatch.setattr(folders._titles, "get", lambda sid, path: "")
    page = landing_app.load(NOW)
    assert page.opened == 1 and [s.id for s in page.folders[0].sessions] == ["old"]
