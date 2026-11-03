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


def test_money():
    assert text.money(4.123) == "$4.12"
    assert text.money(0) == "$0.00"
    assert text.money(None) == ""


def test_duration_formats():
    assert text.duration(240) == "4m"
    assert text.duration(2100) == "35m"
    assert text.duration(6000) == "1h40m"
    assert text.duration(3660) == "1h01m"
    assert text.duration(None) == ""


def test_lines():
    assert text.lines(820, 310) == ("+820", "−310")
    assert text.lines(0, 0) == ("+0", "−0")
    assert text.lines(None, None) == ("", "")


def test_limit_bar_and_colour():
    assert text.limit_bar(64) == "▰" * 13 + "▱" * 7
    assert text.limit_bar(0) == "▱" * 20 and text.limit_bar(120) == "▰" * 20
    assert [text.limit_colour(p) for p in (69.9, 70, 84.9, 85)] == [
        "soft",
        "yellow",
        "yellow",
        "red",
    ]


def test_resets_in_under_a_day_else_weekday_and_time():
    now = time.mktime((2026, 11, 2, 15, 0, 0, 0, 0, -1))
    assert text.resets(now + 6720, now) == "resets in 1h52m"
    friday = time.mktime((2026, 11, 6, 9, 0, 0, 0, 0, -1))
    assert text.resets(friday, now) == "resets Fri 09:00"


def test_pace_text_says_when_the_limit_fills():
    assert text.pace_text(7800) == "hits the limit in ~2h10m"
    assert text.pace_text(None) == ""


def test_limit_bar_takes_a_width():
    assert text.limit_bar(50, 4) == "▰▰▱▱"
