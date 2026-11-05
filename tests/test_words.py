import datetime
import time

from claude_sessions.ui import words

NOW = time.mktime((2026, 11, 3, 14, 7, 0, 0, 0, -1))  # a Tuesday


def test_bar_fills_by_rounding_and_colours_by_level():
    b = words.bar(58, 14)
    assert b.plain == "████████░░░░░░"
    assert b.spans[0].style == words.STYLES["claude"]
    assert b.spans[1].style == words.STYLES["subtle"]
    assert words.bar_style(69) == words.STYLES["claude"]
    assert words.bar_style(70) == words.STYLES["warning"]
    assert words.bar_style(85) == words.STYLES["error"]
    assert words.bar(31, 16).plain == "█████░░░░░░░░░░░"


def test_reset_at_today_or_weekday():
    assert words.reset_at(NOW + 2 * 3600 + 33 * 60, NOW) == "16:40"
    friday = time.mktime((2026, 11, 6, 9, 0, 0, 0, 0, -1))
    assert words.reset_at(friday, NOW) == "Fri 09:00"


def test_thousands_uses_a_comma():
    assert words.thousands(4210) == "4,210"
    assert words.thousands(23) == "23"


def test_short_date_names_the_month_in_english():
    assert words.short_date(datetime.date(2026, 10, 25)) == "25 Oct"
    assert words.short_date(datetime.date(2026, 1, 5)) == "5 Jan"


def test_moved_helpers():
    assert words.fit("abcdefghij", 5) == "abcd…"
    assert words.money(4.1) == "$4.10"
    assert words.money(None) == ""
    assert words.waiting_label(30) == ""
    assert words.waiting_label(125) == "2m"


def test_next_tick_waits_for_the_next_second_but_never_almost_nothing():
    assert abs(words.next_tick(10.25) - 0.75) < 1e-9
    # Textual drops a one-shot timer whose delay has already passed when it starts: a delay of
    # a few microseconds would end the redraw loop, so it waits for the second after
    assert abs(words.next_tick(10.9999999) - 1.0000001) < 1e-6
    assert words.next_tick(10.0) == 1.0


def test_span_is_minutes_then_hours_and_minutes():
    assert words.span(59) == "0m"
    assert words.span(2400) == "40m"
    assert words.span(4320) == "1h12"
    assert words.span(10800) == "3h00"
    assert words.span(None) == ""


def test_next_tick_can_wake_on_the_next_frame():
    assert abs(words.next_tick(10.03, 8) - 0.095) < 1e-9
    assert words.next_tick(10.0, 8) == 0.125
