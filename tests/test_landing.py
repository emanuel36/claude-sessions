import time

from claude_sessions.ui import landing, new_panel, sidebar, usage
from claude_sessions.ui.new_panel import Folder, Session

NOW = time.mktime((2026, 11, 4, 14, 7, 0, 0, 0, -1))  # a Wednesday
FOLDERS = [Folder("/w/api", "api", NOW - 60, 3, 12.5, [Session("s1", "fix it", NOW - 60, 2.0)])]
LIMITS = {"five_hour": {"pct": 11, "resets_at": NOW + 3600}}
WEEK = [(None, 1.0)] * 0
STATS = sidebar.Stats(LIMITS, 9.5, [], usage.Day(sessions=2))


def plain(lines):
    return [line.plain.rstrip() for line in lines]


def picker(query=""):
    p = new_panel.Picker(lambda path: False)
    p.folders, p.query = FOLDERS, query
    return p


def test_header_names_the_day_and_the_open_sessions():
    lines = plain(landing.header(1, NOW))
    assert lines[0].endswith("claude-sessions")
    assert lines[1].endswith("Wed 4 Nov · 1 session open")
    assert "3 sessions open" in plain(landing.header(3, NOW))[1]


def test_search_line_shows_the_query_or_how_to_start():
    assert landing.search_line("", False).plain == " ❯ / to search"
    assert landing.search_line("ap", True).plain == " ❯ ap▌"


def test_screen_has_the_list_on_the_left_and_the_stats_on_the_right():
    long_title = "a session title long enough to need a wide terminal to show it whole"
    p = picker()
    p.folders = [Folder("/w/api", "api", NOW - 60, 3, 12.5, [Session("s1", long_title, NOW, 2.0)])]
    p.key("right")
    width = 140
    # The list and the statistics together, centred
    margin = (width - landing.LIST_MAX - landing.GAP - sidebar.PANEL_WIDTH) // 2
    lines = [line[margin:] for line in plain(landing.screen(p, False, 2, STATS, NOW, width, 30))]
    assert len(lines) == 30 and lines[4].startswith(" ❯ / to search")
    assert lines[6].startswith(" ▾ api") and lines[7].startswith("   3 sessions · $12.50 7d")
    # The list stops at its widest, the statistics right after it
    assert len(lines[8]) == landing.LIST_MAX and "a session title long enough" in lines[8]
    stats_at = landing.LIST_MAX + landing.GAP
    assert lines[0][stats_at:].strip().startswith("• Today")
    assert lines[-1].startswith(" ↑↓ move")


def test_a_narrow_screen_shows_the_list_alone():
    lines = plain(landing.screen(picker(), False, 2, STATS, NOW, 60, 30))
    assert not any("Today" in line for line in lines) and lines[6].startswith(" ▸ api")


def test_a_middling_screen_falls_back_to_the_sidebar_stats():
    width = new_panel.WIDTH + landing.GAP + sidebar.WIDTH
    lines = plain(landing.screen(picker(), False, 2, STATS, NOW, width, 30))
    assert lines[1][new_panel.WIDTH + landing.GAP :].strip() == "• Today               $9.50"


def test_a_short_screen_falls_back_to_the_sidebar_stats():
    lines = plain(landing.screen(picker(), False, 2, STATS, NOW, 160, 8))
    assert lines[1].endswith("• Today               $9.50")


def test_a_footer_too_long_is_cut_with_an_ellipsis():
    assert new_panel.keys_line(landing.FOOTER, 20).plain == " ↑↓ move · → sessio…"
    assert new_panel.keys_line(new_panel.FOOTER, 48).plain.endswith("· Esc")


def test_a_screen_short_of_room_drops_the_spend_by_model_first():
    totals = usage.Totals(by_project=[("api", 2.0)], by_model=[("Opus", 2.0), ("Haiku", 1.0)])
    stats = sidebar.Stats(LIMITS, 9.5, [], usage.Day(sessions=2), totals)
    full = len(sidebar.panel_lines(stats, NOW))
    lines = plain(landing.screen(picker(), False, 2, stats, NOW, 160, full + 1))
    assert not any("By model" in line for line in lines)
    assert any("By folder" in line for line in lines)
    roomy = plain(landing.screen(picker(), False, 2, stats, NOW, 160, full + 2))
    assert any("By model" in line for line in roomy)
