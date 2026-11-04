import asyncio
import datetime

from claude_sessions.ui import landing_app
from claude_sessions.ui.landing import Folder, Recent
from claude_sessions.ui.usage import Totals

NOW = 1_700_000_000.0
RECENT = [
    Recent("sid-api", "api", "Fix the login", 1.5, NOW - 60, "/w/api"),
    Recent("sid-web", "web", "Redo the footer", None, NOW - 120, "/w/web"),
]
FOLDERS = [
    Folder("~/workspace/api", "main", 0),
    Folder("~/workspace/web", "dev", 2),
]
DATA = (RECENT, FOLDERS, {}, Totals(), [0] * 14, datetime.date(2023, 11, 1))


def stub_tmux(monkeypatch, windows="1"):
    calls = []

    def run(*args):
        calls.append(args)
        return windows if args[0] == "display-message" else ""

    monkeypatch.setattr(landing_app.tmux, "run", run)
    monkeypatch.setattr(landing_app.tmux, "new_session_popup", lambda: calls.append(("popup",)))
    monkeypatch.setattr(landing_app.os.path, "expanduser", lambda p: p.replace("~", "/home/u", 1))
    return calls


def drive(keys, load=lambda now: DATA):
    async def go():
        app = landing_app.LandingApp(load=load, window_id="@1")
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause()
            await pilot.press(*keys)
            await pilot.pause()
        return app

    return asyncio.run(go())


def test_enter_on_recent_resumes_in_a_new_window(monkeypatch):
    calls = stub_tmux(monkeypatch)
    drive(["enter"])
    assert ("new-window", "-c", "/w/api", "claude --resume sid-api") in calls


def test_n_on_a_folder_starts_a_new_session_there(monkeypatch):
    calls = stub_tmux(monkeypatch)
    drive(["down"] * 2 + ["n"])
    assert ("new-window", "-c", "/home/u/workspace/api", "claude") in calls


def test_n_on_a_recent_session_starts_in_its_folder(monkeypatch):
    calls = stub_tmux(monkeypatch)
    drive(["j", "n"])
    assert ("new-window", "-c", "/w/web", "claude") in calls


def test_enter_on_other_folder_opens_the_picker(monkeypatch):
    calls = stub_tmux(monkeypatch)
    drive(["down"] * 4 + ["enter"])
    assert ("popup",) in calls


def test_q_ends_the_server(monkeypatch):
    calls = stub_tmux(monkeypatch)
    drive(["q"])
    assert ("kill-server",) in calls


def test_never_closes_itself(monkeypatch):
    calls = stub_tmux(monkeypatch, windows="1")
    drive([])
    assert not any(c[0] == "kill-window" for c in calls)


def test_does_not_read_its_data_while_another_window_is_shown(monkeypatch):
    stub_tmux(monkeypatch, windows="0")
    loaded = []
    drive([], load=lambda now: loaded.append(now) or DATA)
    assert not loaded


def test_slash_filters_recent_sessions(monkeypatch):
    stub_tmux(monkeypatch)
    app = drive(["slash", "f", "o", "o", "t", "enter"])
    assert [r.session_id for r in app.visible()] == ["sid-web"]
    drive(["slash", "f", "escape"])


def test_failed_load_keeps_the_last_screen(monkeypatch):
    stub_tmux(monkeypatch)
    logged = []
    monkeypatch.setattr(landing_app.paths, "log_error", lambda: logged.append(1))

    def boom(now):
        raise OSError("gone")

    async def go():
        app = landing_app.LandingApp(load=lambda now: DATA, window_id="@1")
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause()
            app.load = boom
            app.tick_once(NOW)
            app.tick_once(NOW)
        assert app.recent == RECENT and logged == [1]

    asyncio.run(go())


def test_failed_render_keeps_the_app_alive_and_logs_once(monkeypatch):
    stub_tmux(monkeypatch)
    logged = []
    monkeypatch.setattr(landing_app.paths, "log_error", lambda: logged.append(1))

    def boom():
        raise ValueError("bad row")

    async def go():
        app = landing_app.LandingApp(load=lambda now: DATA, window_id="@1")
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause()
            app.draw = boom
            app.tick_once(NOW)
            app.tick_once(NOW)
            assert app.is_running

    asyncio.run(go())
    assert logged == [1]


def test_failed_load_redraws_the_last_data_with_the_current_time(monkeypatch):
    stub_tmux(monkeypatch)
    monkeypatch.setattr(landing_app.paths, "log_error", lambda: None)

    def boom(now):
        raise OSError("gone")

    async def go():
        app = landing_app.LandingApp(load=lambda now: DATA, window_id="@1")
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause()
            app.load = boom
            app.tick_once(NOW + 7200)
            assert app.recent == RECENT and app.now == NOW + 7200
            await pilot.pause()
            text = "\n".join(strip.text for strip in app.screen._compositor.render_strips())
            assert "yesterday" not in text and "2h" in text.split("Fix the login")[1][:60]

    asyncio.run(go())


def test_load_and_draw_errors_are_logged_separately(monkeypatch):
    stub_tmux(monkeypatch)
    logged = []
    monkeypatch.setattr(landing_app.paths, "log_error", lambda: logged.append(1))

    def boom(now):
        raise OSError("gone")

    def boom_draw():
        raise ValueError("bad row")

    async def go():
        app = landing_app.LandingApp(load=lambda now: DATA, window_id="@1")
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause()
            app.load, app.draw = boom, boom_draw
            for _ in range(2):
                app.tick_once(NOW)

    asyncio.run(go())
    assert logged == [1, 1]


def test_a_failed_draw_on_a_key_keeps_the_app_alive(monkeypatch):
    stub_tmux(monkeypatch)
    logged = []
    monkeypatch.setattr(landing_app.paths, "log_error", lambda: logged.append(1))

    def boom():
        raise ValueError("bad row")

    async def go():
        app = landing_app.LandingApp(load=lambda now: DATA, window_id="@1")
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause()
            app.draw = boom
            await pilot.press("down", "down")
            assert app.is_running and app.selected == 2

    asyncio.run(go())
    assert logged == [1]


def test_load_skips_open_sessions_and_commands_only_ones(monkeypatch, tmp_path):
    sessions = [
        files_session("a", "/w/a", 5, "first a"),
        files_session("b", "/w/b", 4, "first b"),
        files_session("c", "/w/c", 3, ""),
    ]
    monkeypatch.setattr(landing_app.files, "load_history", lambda: sessions)
    monkeypatch.setattr(landing_app.files, "recent_projects", lambda: ["/w/a"])
    monkeypatch.setattr(landing_app.hook, "live_sessions", lambda now: [{"id": "a"}])
    monkeypatch.setattr(landing_app.statusline, "load_limits", lambda now: {})
    monkeypatch.setattr(landing_app, "git_state", lambda path: ("main", 0))
    recent, folders, *_ = landing_app.load(NOW)
    assert [r.session_id for r in recent] == ["b"]
    assert folders[0].branch == "main" and folders[0].changed == 0


def files_session(sid, cwd, last, prompt):
    return landing_app.files.HistorySession(sid, cwd, last, prompt)


def test_git_state_counts_changes_and_is_none_outside_git(monkeypatch):
    landing_app._git.clear()

    class Done:
        def __init__(self, code, out):
            self.returncode, self.stdout = code, out

    def run(cmd, **kw):
        return Done(0, "main\n" if "branch" in cmd else " M a\n?? b\n")

    monkeypatch.setattr(landing_app.subprocess, "run", run)
    assert landing_app.git_state("/x") == ("main", 2)
    landing_app._git.clear()
    monkeypatch.setattr(landing_app.subprocess, "run", lambda cmd, **kw: Done(128, ""))
    assert landing_app.git_state("/y") == ("", None)


def test_filter_rows_matches_all_words_in_title_or_folder():
    rows = [
        Recent("1", "client", "Fix the API timeout", None, NOW, "/w/client"),
        Recent("2", "notes", "Weekly notes", None, NOW, "/w/notes"),
        Recent("3", "api-gateway", "Review", None, NOW, "/w/api-gateway"),
    ]
    assert [r.session_id for r in landing_app.filter_rows(rows, "api")] == ["1", "3"]
    assert [r.session_id for r in landing_app.filter_rows(rows, "API fix")] == ["1"]
    assert [r.session_id for r in landing_app.filter_rows(rows, "")] == ["1", "2", "3"]
