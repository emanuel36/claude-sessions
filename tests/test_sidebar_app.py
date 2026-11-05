import asyncio

from claude_sessions.core import state
from claude_sessions.term import tmux
from claude_sessions.ui import sidebar, sidebar_app
from claude_sessions.ui.sidebar import Entry

NOW = 1_700_000_000.0
ENTRIES = [
    Entry("@1", "client", state.PERMISSION, NOW - 30),
    Entry("@2", "notes", state.DONE, NOW - 60),
    Entry("@3", "api", state.SHELL, NOW - 90),
]
LIMITS = {
    "five_hour": {"pct": 58, "resets_at": NOW + 3600},
    "seven_day": {"pct": 31, "resets_at": NOW + 86400},
}
STATS = sidebar.Stats(LIMITS, 9.59, [])


def screen_text(app):
    return [strip.text for strip in app.screen._compositor.render_strips()]


def make_app(window_id="@3"):
    return sidebar_app.SidebarApp(load=lambda now: (ENTRIES, STATS, []), window_id=window_id)


def stub_tmux(monkeypatch, panes="2"):
    calls = []

    def run(*args):
        calls.append(args)
        return panes if args[0] == "display-message" else ""

    monkeypatch.setattr(sidebar_app.tmux, "run", run)
    return calls


def test_app_draws_the_column_and_highlights_its_window(monkeypatch):
    stub_tmux(monkeypatch)

    async def go():
        app = make_app()
        async with app.run_test(size=(sidebar.WIDTH, 30)) as pilot:
            await pilot.pause()
            lines = screen_text(app)
            # client needs you, then api runs, then notes is done
            assert lines[8].startswith(" ⏺ api")
            strip = app.screen._compositor.render_strips()[8]
            assert any(seg.style.bgcolor for seg in strip if "api" in seg.text)

    asyncio.run(go())


def test_click_selects_that_window(monkeypatch):
    calls = stub_tmux(monkeypatch)

    async def go():
        app = make_app()
        async with app.run_test(size=(sidebar.WIDTH, 30)) as pilot:
            await pilot.pause()
            await pilot.click("#column", offset=(5, 11))
            await pilot.pause()

    asyncio.run(go())
    assert ("select-window", "-t", "@2") in calls


def test_last_pane_left_kills_its_window(monkeypatch):
    calls = stub_tmux(monkeypatch, panes="1")

    async def go():
        app = make_app()
        async with app.run_test(size=(sidebar.WIDTH, 30)) as pilot:
            await pilot.pause()
            await pilot.pause()

    asyncio.run(go())
    assert ("kill-window", "-t", "@3") in calls


def test_failed_load_keeps_the_last_lines_and_logs_once(monkeypatch):
    stub_tmux(monkeypatch)
    logged, fail = [], []
    monkeypatch.setattr(sidebar_app.paths, "log_error", lambda: logged.append(1))

    def load(now):
        if fail:
            raise ValueError("mid-write")
        return ENTRIES, STATS, []

    async def go():
        app = sidebar_app.SidebarApp(load=load, window_id="@3")
        async with app.run_test(size=(sidebar.WIDTH, 30)) as pilot:
            await pilot.pause()
            fail.append(1)
            app.tick_once(NOW)
            app.tick_once(NOW)
            assert "api" in screen_text(app)[8]

    asyncio.run(go())
    assert logged == [1]


def test_failed_render_keeps_the_app_alive_and_logs_once(monkeypatch):
    stub_tmux(monkeypatch)
    logged = []
    monkeypatch.setattr(sidebar_app.paths, "log_error", lambda: logged.append(1))

    def boom(now):
        raise ValueError("bad entry")

    async def go():
        app = make_app()
        async with app.run_test(size=(sidebar.WIDTH, 30)) as pilot:
            await pilot.pause()
            app.draw = boom
            app.tick_once(NOW)
            app.tick_once(NOW)
            assert app.is_running
            assert "api" in screen_text(app)[8]

    asyncio.run(go())
    assert logged == [1]


def test_failed_load_redraws_the_last_data_with_the_current_time(monkeypatch):
    stub_tmux(monkeypatch)
    monkeypatch.setattr(sidebar_app.paths, "log_error", lambda: None)
    drawn, fail = [], []

    def load(now):
        if fail:
            raise ValueError("mid-write")
        return ENTRIES, STATS, []

    async def go():
        app = sidebar_app.SidebarApp(load=load, window_id="@3")
        async with app.run_test(size=(sidebar.WIDTH, 30)) as pilot:
            await pilot.pause()
            fail.append(1)
            app.draw = drawn.append
            app.tick_once(NOW)
            assert app.entries == ENTRIES

    asyncio.run(go())
    assert drawn == [NOW]


def test_load_and_draw_errors_are_logged_separately(monkeypatch):
    stub_tmux(monkeypatch)
    logged = []
    monkeypatch.setattr(sidebar_app.paths, "log_error", lambda: logged.append(1))

    def boom(now):
        raise ValueError("x")

    async def go():
        app = make_app()
        async with app.run_test(size=(sidebar.WIDTH, 30)) as pilot:
            await pilot.pause()
            app.load = boom
            app.draw = boom
            for _ in range(2):
                app.tick_once(NOW)

    asyncio.run(go())
    assert logged == [1, 1]


def test_load_orders_by_window_index_and_skips_windows_without_sessions(monkeypatch):
    record = {
        "status": state.WORKING,
        "status_since": NOW - 5,
        "cwd": "/w/web",
        "started_at": NOW - 900,
        "usage": {"model": "Opus 4", "context": 12.0, "cost": 1.5, "api_duration": 900.0},
        "mux": {"pane": "%4"},
    }
    monkeypatch.setattr(sidebar_app.hook, "live_sessions", lambda now: [record])
    monkeypatch.setattr(sidebar_app.tmux, "windows_of", lambda r: [tmux.Window("@4", record)])
    monkeypatch.setattr(
        sidebar_app.tmux, "window_entries", lambda: [("@1", 1, "api"), ("@4", 2, "web")]
    )
    monkeypatch.setattr(sidebar_app.statusline, "load_limits", lambda now: {"x": 1})
    monkeypatch.setattr(sidebar_app.files, "load_history", lambda: [])

    entries, stats, runs = sidebar_app.load(NOW)
    # The time beside the cost is what Claude spent answering, not the time since the start
    assert entries == [Entry("@4", "web", state.WORKING, NOW - 5, 1.5, 900.0, "Opus 4")]
    assert stats.limits == {"x": 1} and stats.today == 0 and len(stats.week) == 7


def test_window_entries_lists_windows_by_index(monkeypatch):
    out = "@4\t2\tweb\n@1\t1\tapi"
    monkeypatch.setattr(tmux, "run", lambda *args: out)
    assert tmux.window_entries() == [("@1", 1, "api"), ("@4", 2, "web")]


def test_load_shows_the_most_urgent_status_with_the_first_panes_details(monkeypatch):
    def record(session_id, status, model, cost, pane):
        return {
            "id": session_id,
            "status": status,
            "status_since": NOW - 5,
            "cwd": "/w/web",
            "usage": {"model": model, "cost": cost},
            "mux": {"socket": "/s/claude", "pane": pane},
        }

    leader = record("lead", state.IDLE, "Opus 4", 1.0, "%2")
    mate = record("mate", state.PERMISSION, "Sonnet 4", 9.0, "%3")
    monkeypatch.setenv("TMUX", "/s/claude,1,0")
    monkeypatch.setattr(sidebar_app.hook, "live_sessions", lambda now: [mate, leader])
    monkeypatch.setattr(sidebar_app.tmux, "run", lambda *args: "%2\t@1\t2\n%3\t@1\t3\n%1\t@1\t1")
    monkeypatch.setattr(sidebar_app.tmux, "window_entries", lambda: [("@1", 1, "web")])
    monkeypatch.setattr(sidebar_app.statusline, "load_limits", lambda now: {})
    monkeypatch.setattr(sidebar_app.files, "load_history", lambda: [])

    [entry] = sidebar_app.load(NOW)[0]
    assert (entry.status, entry.cost) == (state.PERMISSION, 1.0)


def test_load_lists_headless_runs_until_they_finish(monkeypatch):
    run = {
        "id": "r1",
        "status": state.WORKING,
        "cwd": "/tmp/daily-recap",
        "headless": True,
        "started_at": NOW - 120,
        "mux": {},
    }
    done = {**run, "id": "r2", "status": state.DONE, "cwd": "/tmp/trending-repos"}
    waiting = {**run, "id": "r4", "status": state.IDLE, "cwd": "/tmp/sdk-app"}
    old = {**run, "id": "r3", "cwd": "/tmp/old"}
    del old["started_at"]
    old["status_since"] = NOW - 60
    monkeypatch.setattr(sidebar_app.hook, "live_sessions", lambda now: [run, done, old, waiting])
    monkeypatch.setattr(sidebar_app.tmux, "windows_of", lambda r: [])
    monkeypatch.setattr(sidebar_app.tmux, "window_entries", lambda: [])
    monkeypatch.setattr(sidebar_app.statusline, "load_limits", lambda now: {})
    monkeypatch.setattr(sidebar_app.files, "load_history", lambda: [])
    runs = sidebar_app.load(NOW)[2]
    assert runs == [sidebar.Run("daily-recap", NOW - 120), sidebar.Run("old", NOW - 60)]


def test_the_tmux_pane_is_as_wide_as_the_column():
    assert tmux.SIDEBAR_WIDTH == sidebar.WIDTH


def test_f12_from_tmux_draws_at_once(monkeypatch):
    stub_tmux(monkeypatch)
    loads = []

    async def go():
        app = sidebar_app.SidebarApp(
            load=lambda now: loads.append(now) or (ENTRIES, STATS, []), window_id="@3"
        )
        async with app.run_test(size=(sidebar.WIDTH, 30)) as pilot:
            await pilot.pause()
            before = len(loads)
            await pilot.press("f12")
            await pilot.pause()
            assert len(loads) == before + 1

    asyncio.run(go())
