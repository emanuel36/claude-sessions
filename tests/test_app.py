import asyncio
import itertools
import time

from rich.cells import cell_len

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
        app = panel_app.PanelApp(data)
        async with app.run_test(size=(100, 20)) as pilot:
            await pilot.pause()
            text = "\n".join(screen_text(app))
            assert "Sessions" in text and "Recent" in text and "sync" in text
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


SPENT = {
    "cost": 4.12,
    "duration": 6000.0,
    "model": "Opus 5.5",
    "added": 820,
    "removed": 310,
    "context": 72.0,
}


def usage_rows():
    return [
        loader.Row(
            "a",
            "Restructure the config",
            "/w/a",
            window("@2", state.IDLE),
            "2",
            usage=SPENT,
            branch="feat/x",
        ),
        loader.Row("b", "No status line yet", "/w/b", window("@3", state.IDLE), "3"),
    ]


def test_open_rows_show_usage_columns_under_their_header():
    async def go():
        app = panel_app.PanelApp(StubData(usage_rows()))
        async with app.run_test(size=(140, 20)) as pilot:
            await pilot.pause()
            text = screen_text(app)
            header = next(line for line in text if "Sessions" in line)
            row = next(line for line in text if "Restructure" in line)
            empty = next(line for line in text if "No status line yet" in line)
            assert header.split()[1:] == ["model", "branch", "context", "time", "lines", "cost"]
            assert row.index("Opus 5.5") == header.index("model")
            assert row.index("feat/x") == header.index("branch")
            assert row.index("72%") + 3 == header.index("context") + 7
            assert row.index("$4.12") + 5 == header.index("cost") + 4
            assert "+820 −310" in row and row.index("tab 2") == empty.index("tab 3")
            # A session that has not reported yet leaves its cells empty
            assert empty.split() == ["No", "status", "line", "yet", "tab", "3"]

    run(go())


def test_narrow_panel_shrinks_the_title_then_drops_columns():
    async def go():
        app = panel_app.PanelApp(StubData(usage_rows()))
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.pause()
            header = next(line for line in screen_text(app) if "Sessions" in line)
            assert header.split()[1:] == ["model", "context", "cost"]
        narrow = panel_app.PanelApp(StubData(usage_rows()))
        async with narrow.run_test(size=(40, 20)) as pilot:
            await pilot.pause()
            header = next(line for line in screen_text(narrow) if "Sessions" in line)
            assert header.split()[1:] == ["cost"]

    run(go())


def test_quota_strip_shows_both_windows_and_when_the_5h_one_fills():
    async def go():
        data = StubData(OPEN)
        now = time.time()
        data.snapshot.limits = {
            "five_hour": {"pct": 62, "resets_at": now + 6000},
            "seven_day": {"pct": 35, "resets_at": now + 4 * 86400},
        }
        data.snapshot.pace = {"five_hour": 3600.0, "seven_day": None}
        app = panel_app.PanelApp(data)
        async with app.run_test(size=(100, 20)) as pilot:
            await pilot.pause()
            text = screen_text(app)
            five = next(line for line in text if "5h" in line)
            seven = next(line for line in text if "7d" in line)
            assert "62%" in five and "hits the limit in ~1h00m" in five
            assert "35%" in seven and "hits the limit" not in seven

    run(go())


def test_quota_strip_is_hidden_without_limits():
    async def go():
        app = panel_app.PanelApp(StubData(OPEN))
        async with app.run_test(size=(100, 20)) as pilot:
            await pilot.pause()
            assert not app.query_one("#quota").display

    run(go())


def test_spend_section_lists_the_projects_with_bars_scaled_to_the_largest():
    async def go():
        data = StubData(OPEN)
        data.snapshot.spend = [("api", 31.0), ("docs", 15.5)]
        app = panel_app.PanelApp(data)
        async with app.run_test(size=(100, 24)) as pilot:
            await pilot.pause()
            text = screen_text(app)
            api = next(line for line in text if "api" in line and "$31.00" in line)
            docs = next(line for line in text if "docs" in line and "$15.50" in line)
            assert api.count("▰") == 12 and docs.count("▰") == 6
            assert any("Spend 7d by project" in line for line in text)

    run(go())


def test_a_failed_load_keeps_the_last_snapshot(monkeypatch):
    class Failing(StubData):
        def load(self, now):
            if self.calls:
                raise OSError("gone")
            self.calls += 1
            return self.snapshot

    data = Failing(OPEN)
    data.calls = 0

    async def go():
        app = panel_app.PanelApp(data)
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.pause()
            timers = []
            monkeypatch.setattr(app, "set_timer", lambda *args: timers.append(args))
            # No await in between: the running timer chain cannot fire meanwhile
            app.reload()
            app.reload()
            assert timers == []
            app.tick()
            assert len(timers) == 1
            await pilot.pause()
            assert "Weekly notes" in "\n".join(screen_text(app))

    run(go())


def test_a_failed_load_is_logged_once_per_error(monkeypatch):
    logged = []
    monkeypatch.setattr(panel_app.paths, "log_error", lambda: logged.append(True))
    errors = itertools.chain([OSError("gone")] * 2, itertools.repeat(ValueError("torn")))

    class Failing(StubData):
        def load(self, now):
            raise next(errors)

    async def go():
        app = panel_app.PanelApp(Failing(OPEN))
        async with app.run_test(size=(80, 20)):
            app.reload()
            app.reload()

    run(go())
    assert len(logged) == 2


def bar_of(app, title_start):
    """The column where the coloured cells of a row start, and how many there are."""
    strip = next(
        s
        for s in app.screen._compositor.render_strips()
        if title_start in s.text and "tab" not in s.text
    )
    col, first, count = 0, None, 0
    for seg in strip:
        if seg.style and seg.style.bgcolor and seg.style.bgcolor.number is not None:
            first = col if first is None else first
            count += len(seg.text)
        col += cell_len(seg.text)
    return first, count


def test_a_tick_after_the_app_stopped_does_nothing():
    app = panel_app.PanelApp(StubData(OPEN))

    async def go():
        async with app.run_test(size=(80, 20)) as pilot:
            await pilot.pause()

    run(go())
    # A timer that fires during teardown finds no widgets to draw on
    app.tick()
