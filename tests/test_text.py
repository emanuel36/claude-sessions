import time

from claude_sessions.core import state
from claude_sessions.panel.data.loader import Row
from claude_sessions.panel.view import text
from claude_sessions.term.tmux import Window

NOW = 1_000_000.0


def window(window_id, session_id, status, since=NOW):
    lead = {"id": session_id, "status": status, "status_since": since, "cwd": f"/w/{session_id}"}
    return Window(window_id, {**lead, "mux": {"pane": "%" + window_id[1:]}})


def test_status_cell_permission():
    row = Row("s", "t", "/w", window("@2", "s", state.PERMISSION, NOW - 200), tab="2")
    assert text.status_cell(row, NOW) == ("!", "red", "tab 2  permission 3m")


def test_status_cell_other_statuses():
    def cell(status):
        return text.status_cell(Row("s", "t", "/w", window("@3", "s", status), "3"), NOW)

    assert cell(state.INPUT) == ("?", "yellow", "tab 3  question")
    assert cell(state.DONE) == ("✓", "green", "tab 3  finished")
    assert cell(state.IDLE) == (" ", "soft", "tab 3")
    assert text.status_cell(Row("s", "t", "/w", elsewhere=True), NOW)[2] == ("open elsewhere")
    assert text.status_cell(Row("s", "t", "/w", last=NOW - 60), NOW) == (
        " ",
        "faint",
        text.when(NOW - 60, NOW),
    )


def test_status_cell_working_blinks():
    row = Row("s", "t", "/w", window("@2", "s", state.WORKING), tab="2")
    assert text.status_cell(row, 10.9) == ("●", "blue", "tab 2  working")
    assert text.status_cell(row, 11.2)[0] == "○"


def test_waiting_label():
    assert text.waiting_label(30) == ""
    assert text.waiting_label(125) == "2m"
    assert text.waiting_label(7300) == "2h"


def test_when_today_yesterday_weekday_date():
    now = time.mktime((2026, 11, 2, 15, 0, 0, 0, 0, -1))
    assert text.when(now - 3600, now) == "14:00"
    assert text.when(now - 24 * 3600, now) == "yesterday 15:00"
    assert text.when(now - 3 * 24 * 3600, now).startswith("Fri ")
    assert text.when(now - 30 * 24 * 3600, now) == "03 Oct"


def test_fit_counts_terminal_columns():
    assert text.fit("abc", 5) == "abc"
    assert text.fit("abcdef", 4) == "abc…"
    # Wide characters take two columns each
    assert text.fit("日本語です", 5) == "日本…"
    assert text.fit("x", 0) == ""
