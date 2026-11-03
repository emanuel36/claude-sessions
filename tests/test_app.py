import asyncio

from claude_sessions.core import state
from claude_sessions.panel import app as panel_app
from claude_sessions.panel.data import loader
from claude_sessions.term.tmux import Window


def window(window_id, status):
    return Window(window_id, {"id": window_id, "status": status, "mux": {"pane": "%1"}})


OPEN = [
    loader.Row("a", "Fix the API timeout", "/w/client", window("@2", state.PERMISSION), "2"),
    loader.Row("b", "Weekly notes", "/w/notes", window("@3", state.WORKING), "3"),
]
RECENT = [loader.Row("c", "Review the api retry", "/w/sync", last=1.0)]


class StubData:
    def __init__(self, opened=(), recent=()):
        self.snapshot = loader.Snapshot(list(opened), list(recent), {})

    def load(self, now):
        return self.snapshot


def screen_text(app):
    return [strip.text for strip in app.screen._compositor.render_strips()]


def run(coro):
    asyncio.run(coro)


def test_empty_tiny_terminal():
    async def go():
        app = panel_app.PanelApp(StubData())
        async with app.run_test(size=(40, 12)) as pilot:
            await pilot.pause()
            assert any(
                "No sessions yet. Press n to start one." in line for line in screen_text(app)
            )

    run(go())


def test_header_and_sections():
    async def go():
        data = StubData(OPEN, RECENT)
        data.snapshot.counts = {state.PERMISSION: 1, state.WORKING: 1}
        app = panel_app.PanelApp(data)
        async with app.run_test(size=(100, 20)) as pilot:
            await pilot.pause()
            text = "\n".join(screen_text(app))
            assert "Claude sessions" in text and "! 1 permission" in text and "1 working" in text
            assert "Open" in text and "Recent" in text and "sync" in text
            assert "enter open    n new session    / search" in text

    run(go())


def test_long_title_shrinks_first():
    async def go():
        rows = [loader.Row("a", "x" * 200, "/w/a", window("@2", state.WORKING), "2")]
        app = panel_app.PanelApp(StubData(rows))
        async with app.run_test(size=(80, 12)) as pilot:
            await pilot.pause()
            line = next(line for line in screen_text(app) if "xxx" in line)
            assert "…" in line and "tab 2  working" in line

    run(go())


def test_down_enter_selects_window(monkeypatch):
    selected = []
    monkeypatch.setattr(panel_app.tmux, "select", selected.append)

    async def go():
        app = panel_app.PanelApp(StubData(OPEN, RECENT))
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.press("down", "enter")
            assert selected == [OPEN[1].window]

    run(go())


def test_enter_resumes_a_recent_session_once(monkeypatch, tmp_path):
    calls = []
    monkeypatch.setattr(panel_app.tmux, "run", lambda *args: calls.append(args) or "")
    recent = [loader.Row("c", "Review", str(tmp_path), last=1.0)]

    async def go():
        app = panel_app.PanelApp(StubData(recent=recent))
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.press("enter", "enter")
            await pilot.pause()
            assert "Opening it…" in "\n".join(screen_text(app))

    run(go())
    assert calls == [("new-window", "-c", str(tmp_path), "claude --resume c")]


def test_resume_quotes_the_session_id(monkeypatch, tmp_path):
    calls = []
    monkeypatch.setattr(panel_app.tmux, "run", lambda *args: calls.append(args) or "")
    recent = [loader.Row("c; rm x", "Review", str(tmp_path), last=1.0)]

    async def go():
        app = panel_app.PanelApp(StubData(recent=recent))
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.press("enter")

    run(go())
    assert calls == [("new-window", "-c", str(tmp_path), "claude --resume 'c; rm x'")]


def test_redraw_keeps_the_scroll_until_the_selection_moves():
    recent = [loader.Row(f"r{i}", f"Session {i}", "/w/x", last=1.0) for i in range(40)]

    async def go():
        app = panel_app.PanelApp(StubData(recent=recent))
        async with app.run_test(size=(80, 12)) as pilot:
            await pilot.pause()
            body = app.query_one("#body")
            body.scroll_to(y=20, animate=False, immediate=True)
            app.redraw()
            await pilot.pause()
            assert body.scroll_y == 20
            await pilot.press("j")
            await pilot.pause()
            assert body.scroll_y < 20

    run(go())


def test_enter_on_gone_folder_or_elsewhere_says_why(monkeypatch):
    monkeypatch.setattr(panel_app.tmux, "run", lambda *args: "")
    recent = [
        loader.Row("c", "Gone", "/nonexistent/folder", last=1.0),
        loader.Row("d", "Away", "/", last=1.0, elsewhere=True),
    ]

    async def go():
        app = panel_app.PanelApp(StubData(recent=recent))
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.press("enter")
            assert "Cannot resume: /nonexistent/folder is gone" in "\n".join(screen_text(app))
            await pilot.press("j", "enter")
            text = "\n".join(screen_text(app))
            assert "This session is already open in another terminal" in text

    run(go())


def test_n_opens_new_session(monkeypatch):
    calls = []
    monkeypatch.setattr(panel_app.tmux, "run", lambda *args: calls.append(args) or "")

    async def go():
        app = panel_app.PanelApp(StubData(OPEN))
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.press("n")
            await pilot.pause()
            assert "n new session" in "\n".join(screen_text(app))

    run(go())
    assert [c[0] for c in calls] == ["display-popup"]


def test_search_filters_and_escape_clears():
    async def go():
        app = panel_app.PanelApp(StubData(OPEN, RECENT))
        async with app.run_test(size=(100, 20)) as pilot:
            await pilot.press("slash", "a", "p", "i", "enter")
            text = "\n".join(screen_text(app))
            assert "Fix the API timeout" in text and "Review the api retry" in text
            assert "Weekly notes" not in text
            await pilot.press("escape")
            assert "Weekly notes" in "\n".join(screen_text(app))

    run(go())


def test_search_without_match():
    async def go():
        app = panel_app.PanelApp(StubData(OPEN))
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.press("slash", "z", "z")
            text = "\n".join(screen_text(app))
            assert "Nothing matches. Esc clears the search." in text

    run(go())


def test_click_only_selects(monkeypatch):
    selected = []
    monkeypatch.setattr(panel_app.tmux, "select", selected.append)

    async def go():
        app = panel_app.PanelApp(StubData(OPEN))
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.pause()
            y = next(i for i, line in enumerate(screen_text(app)) if "Weekly notes" in line)
            await pilot.click(offset=(10, y))
            assert selected == [] and app.selected == "b"

    run(go())


def test_ctrl_c_does_not_quit():
    async def go():
        app = panel_app.PanelApp(StubData(OPEN))
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.press("ctrl+c", "ctrl+q")
            await pilot.pause()
            assert app.is_running

    run(go())


def test_a_tick_after_the_app_stopped_does_nothing():
    app = panel_app.PanelApp(StubData(OPEN))

    async def go():
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.pause()

    run(go())
    # A timer that fires during teardown finds no widgets to draw on
    app.tick()
