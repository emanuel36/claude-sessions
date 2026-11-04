import datetime
import time

from claude_sessions.ui import sidebar, words

NOW = time.mktime((2026, 11, 3, 14, 7, 0, 0, 0, -1))  # a Tuesday, an even second
TODAY = datetime.date(2026, 11, 3)
LIMITS = {
    "five_hour": {"pct": 58, "resets_at": NOW + 2 * 3600 + 33 * 60},
    "seven_day": {"pct": 31, "resets_at": time.mktime((2026, 11, 6, 9, 0, 0, 0, 0, -1))},
}
WEEK = [(TODAY - datetime.timedelta(days=6 - i), v) for i, v in enumerate([0, 2, 4, 6, 8, 1, 3])]
STATS = sidebar.Stats(LIMITS, 9.59, WEEK)
E = sidebar.Entry
ENTRIES = [
    E(1, "@1", "web", "permission", NOW - 180, 3.10, 2400),
    E(2, "@2", "vehicle.k265", "working", NOW - 5, 0.90, 4320),
    E(3, "@3", "api", "shell", NOW - 360, 5.42, 7500),
    E(4, "@4", "docs", "done", NOW - 9, 0.12, 900),
    E(5, "@5", "notes", "idle", NOW - 9, 0.05, 10800),
]


def body(lines):
    """The content columns of each line, without the padding or trailing spaces."""
    return [ln.plain[1 : 1 + sidebar.CONTENT].rstrip() for ln in lines]


def entry(status, waited):
    return E(1, "@1", "web", status, NOW - waited, 3.10, 2400)


def test_column_matches_the_spec_text():
    lines = sidebar.column(ENTRIES, "@3", STATS, NOW, height=30)
    assert len(lines) == 30
    assert all(len(ln.plain) == sidebar.WIDTH == 29 for ln in lines)
    assert body(lines)[:15] == [
        " " * 9 + " ▐▛███▜▌", " " * 9 + "▝▜█████▛▘", " " * 9 + "  ▘▘ ▝▝", "",
        "! 1 web                  3m", "    $3.10 · 40m",
        "● 2 vehicle.k265", "    $0.90 · 1h12",
        "⧗ 3 api                  6m", "    $5.42 · 2h05",
        "✓ 4 docs", "    $0.12 · 15m",
        "  5 notes", "    $0.05 · 3h00", "",
    ]  # fmt: skip
    assert body(lines)[-13:] == [
        "─" * 27,
        "5h  ██████████░░░░░░░░ 58%", "    resets in 2h33",
        "7d  ██████░░░░░░░░░░░░ 31%", "    resets in 2d 18h",
        "today $9.59",
        "",
        "spend 7d             $24.00",
        "                ██",
        "            ██  ██",
        "        ██  ██  ██      ▄▄",
        "    ██  ██  ██  ██  ▄▄  ██",
        "We  Th  Fr  Sa  Su  Mo  Tu",
    ]  # fmt: skip


def test_chart_marks_today():
    chart = sidebar.stats_lines(STATS, NOW)[-5:]
    assert any(s.style == words.STYLES["green"] for s in chart[-2].spans)  # today's column
    assert any(s.style == words.STYLES["strong"] for s in chart[-1].spans)  # "Tu"


def test_a_week_with_no_spend_draws_empty_columns():
    empty = sidebar.Stats({}, 0.0, [(d, 0.0) for d, _ in WEEK])
    assert body(sidebar.stats_lines(empty, NOW))[-5:] == [
        "",
        "",
        "",
        "",
        "We  Th  Fr  Sa  Su  Mo  Tu",
    ]


def test_name_uses_the_room_the_waiting_time_leaves():
    long = E(1, "@1", "claude-sessions-and-mo", "permission", NOW - 180, 1.0, 60)
    assert body(sidebar.entry_lines(long, False, NOW))[0] == "! 1 claude-sessions-and… 3m"
    calm = E(1, "@1", "claude-sessions-and-mo", "working", NOW, 1.0, 60)
    assert body(sidebar.entry_lines(calm, False, NOW))[0] == "● 1 claude-sessions-and-mo"


def test_current_entry_has_the_background_and_strong_name():
    first, second = sidebar.entry_lines(ENTRIES[2], current=True, now=NOW)
    assert first.style == words.CURRENT and second.style == words.CURRENT
    assert any(s.style == words.STYLES["strong"] for s in first.spans)
    assert len(first.plain) == len(second.plain) == sidebar.WIDTH


def test_waiting_time_shows_only_for_timed_statuses_from_one_minute():
    assert "10m" not in sidebar.entry_lines(entry("done", waited=600), False, NOW)[0].plain
    fresh = sidebar.entry_lines(entry("permission", waited=30), False, NOW)[0].plain
    assert fresh.strip() == "! 1 web"


def test_unknown_usage_leaves_parts_out():
    lines = sidebar.entry_lines(E(1, "@1", "new", "working", NOW), False, NOW)
    assert lines[1].plain.strip() == ""
    only_cost = sidebar.entry_lines(E(1, "@1", "new", "working", NOW, 0.5), False, NOW)
    assert only_cost[1].plain.strip() == "$0.50"


def test_short_terminal_keeps_stats_and_cuts_sessions():
    lines = sidebar.column(ENTRIES, "@3", STATS, NOW, height=19)
    assert len(lines) == 19 and "Tu" in lines[-1].plain and "! 1 web" in lines[4].plain
    assert not any("vehicle" in ln.plain for ln in lines)


def test_no_limits_leaves_quota_out():
    lines = body(sidebar.column(ENTRIES, "@3", sidebar.Stats({}, 0.0, WEEK), NOW, height=30))
    assert "today $0.00" in lines and not any(ln.startswith("5h") for ln in lines)


def test_entry_at_maps_rows_to_entries():
    assert sidebar.entry_at(ENTRIES, 4, 30, STATS).name == "web"
    assert sidebar.entry_at(ENTRIES, 5, 30, STATS).name == "web"
    assert sidebar.entry_at(ENTRIES, 6, 30, STATS).name == "vehicle.k265"
    assert sidebar.entry_at(ENTRIES, 0, 30, STATS) is None
    assert sidebar.entry_at(ENTRIES, 14, 30, STATS) is None


def test_entry_at_ignores_entries_cut_by_a_short_terminal():
    assert sidebar.entry_at(ENTRIES, 4, 19, STATS).name == "web"
    assert sidebar.entry_at(ENTRIES, 6, 19, STATS) is None  # the 2nd entry, not drawn
    no_quota = sidebar.Stats({}, 0.0, WEEK)
    assert sidebar.entry_at(ENTRIES, 8, 19, no_quota).name == "api"  # no quota: more room


RUNS = [sidebar.Run("daily-recap", NOW - 150), sidebar.Run("trending-repos", NOW - 70)]


def test_background_runs_show_under_the_sessions():
    lines = sidebar.column(ENTRIES[:2], "@1", STATS, NOW, height=30, runs=RUNS)
    assert body(lines)[8:12] == [
        "",
        "Background",
        "↻ daily-recap            2m",
        "↻ trending-repos         1m",
    ]
    assert any(s.style == words.STYLES["magenta"] for s in lines[10].spans)


def test_no_runs_no_background_section():
    lines = sidebar.column(ENTRIES[:2], "@1", STATS, NOW, height=30)
    assert not any("Background" in ln.plain for ln in lines)


def test_short_terminal_drops_runs_before_sessions():
    lines = sidebar.column(ENTRIES[:2], "@1", STATS, NOW, 23, RUNS)
    assert len(lines) == 23 and any("vehicle.k265" in ln.plain for ln in lines)
    assert not any("trending-repos" in ln.plain for ln in lines)


def test_a_new_run_keeps_its_whole_name():
    name = "x" * (sidebar.CONTENT - 2)
    lines = sidebar.runs_lines([sidebar.Run(name, NOW - 30)], NOW, room=5)
    assert body(lines)[2] == "↻ " + name


def test_the_waiting_time_keeps_a_space_before_it():
    wide = E(123, "@1", "名前名前名前名前名前名前", "permission", NOW - 720, 1.0, 60)
    line = body(sidebar.entry_lines(wide, False, NOW))[0]
    assert line.endswith(" 12m") and len(line) <= sidebar.CONTENT
