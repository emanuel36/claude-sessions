import datetime
import time

from claude_sessions.ui import sidebar, usage, words

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
    E("@1", "web", "permission", NOW - 180, 3.10, 2400),
    E("@2", "vehicle.k265", "working", NOW - 5, 0.90, 4320),
    E("@3", "api", "shell", NOW - 360, 5.42, 7500),
    E("@4", "docs", "done", NOW - 9, 0.12, 900),
    E("@5", "notes", "idle", NOW - 9, 0.05, 10800),
]


def body(lines):
    """The content columns of each line, without the padding or trailing spaces."""
    return [ln.plain[1 : 1 + sidebar.CONTENT].rstrip() for ln in lines]


def entry(status, waited):
    return E("@1", "web", status, NOW - waited, 3.10, 2400)


def test_column_matches_the_spec_text():
    day = usage.Day(hours=[0] * 8 + [1, 3, 5, 8, 6, 2, 1, 0, 4, 7, 9, 5, 2, 0, 0, 0], added=1376)
    day.removed, day.sessions, day.waited = 58, 4, 720
    stats = sidebar.Stats(LIMITS, 9.59, WEEK, day)
    lines = sidebar.column(ENTRIES, "@3", stats, NOW, height=50)
    assert len(lines) == 50
    assert all(len(ln.plain) == sidebar.WIDTH == 29 for ln in lines)
    rule = "─" * 27
    assert body(lines)[:22] == [
        " " * 9 + " ▐▛███▜▌", " " * 9 + "▝▜█████▛▘", " " * 9 + "  ▘▘ ▝▝", "",
        "NEEDS YOU",
        "web                      3m", "approve · $3.10 · 40m",
        "",
        "RUNNING",
        "vehicle.k265", "$0.90 · 1h12", rule,
        "api                      6m", "running · $5.42 · 2h05",
        "",
        "DONE",
        "docs", "$0.12 · 15m",
        "",
        "IDLE",
        "notes", "$0.05 · 3h00",
    ]  # fmt: skip
    assert body(lines)[-17:] == [
        rule,
        "TODAY                 $9.59",
        "        ▁▃▄▇▅▂▁ ▄▆█▄▂",
        "0     6     12    18",
        "lines +1,376 −58",
        "waited 12m · sessions 4",
        rule,
        "USAGE",
        "5h ████████░░░░░  58% 16:40",
        "7d ████░░░░░░░░░  31% Fri",
        rule,
        "SPEND 7D             $24.00",
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
    long = E("@1", "claude-sessions-and-more-of-it", "permission", NOW - 180, 1.0, 60)
    assert body(sidebar.entry_lines(long, False, NOW))[0] == "claude-sessions-and-mor… 3m"
    calm = E("@1", "claude-sessions-and-more-of-it", "working", NOW, 1.0, 60)
    assert body(sidebar.entry_lines(calm, False, NOW))[0] == "claude-sessions-and-more-o…"


def test_the_model_closes_the_second_line_only_when_it_fits_whole():
    short = E("@1", "web", "working", NOW, 2.78, 5340, "Opus 5.5")
    assert body(sidebar.entry_lines(short, False, NOW))[1] == "$2.78 · 1h29 · Opus 5.5"
    long = E("@1", "web", "permission", NOW, 67.05, 600, "Sonnet 5.5")
    assert body(sidebar.entry_lines(long, False, NOW))[1] == "approve · $67.05 · 10m"


def test_current_entry_has_a_strong_name_and_every_status_its_tint():
    first, second = sidebar.entry_lines(ENTRIES[2], current=True, now=NOW)
    assert any(s.style == words.STYLES["strong"] for s in first.spans)
    assert len(first.plain) == len(second.plain) == sidebar.WIDTH
    for e in ENTRIES[:4]:
        lines = sidebar.entry_lines(e, False, NOW)
        assert lines[0].style == lines[1].style and lines[0].style.bgcolor is not None
    idle = sidebar.entry_lines(ENTRIES[4], False, NOW)
    assert not idle[0].style
    # An idle session is only highlighted when it is the current one
    assert sidebar.entry_lines(ENTRIES[4], True, NOW)[0].style == words.CURRENT


def test_a_working_tint_breathes_and_the_others_stand_still():
    tints = {sidebar.tint("working", False, NOW + t / 8) for t in range(12)}
    assert len(tints) > 4
    assert sidebar.tint("permission", False, NOW) == sidebar.tint("permission", False, NOW + 0.3)


def test_waiting_time_shows_only_for_timed_statuses_from_one_minute():
    assert "10m" not in sidebar.entry_lines(entry("done", waited=600), False, NOW)[0].plain
    fresh = sidebar.entry_lines(entry("permission", waited=30), False, NOW)[0].plain
    assert fresh.strip() == "web"


def test_unknown_usage_leaves_parts_out():
    lines = sidebar.entry_lines(E("@1", "new", "working", NOW), False, NOW)
    assert lines[1].plain.strip() == ""
    only_cost = sidebar.entry_lines(E("@1", "new", "working", NOW, 0.5), False, NOW)
    assert only_cost[1].plain.strip() == "$0.50"


def test_short_terminal_keeps_stats_and_cuts_sessions():
    lines = sidebar.column(ENTRIES, "@3", STATS, NOW, height=20)
    assert len(lines) == 20 and "Tu" in lines[-1].plain and "web" in lines[5].plain
    assert not any("vehicle" in ln.plain for ln in lines)


def test_no_limits_leaves_quota_out():
    lines = body(sidebar.column(ENTRIES, "@3", sidebar.Stats({}, 0.0, WEEK), NOW, height=30))
    assert "SPEND 7D             $24.00" in lines and "USAGE" not in lines


def test_entry_at_maps_rows_to_entries():
    # Row 4 is the NEEDS YOU heading; the rules, blanks and headings belong to no entry
    assert sidebar.entry_at(ENTRIES, 4, 50, STATS) is None
    assert sidebar.entry_at(ENTRIES, 5, 50, STATS).name == "web"
    assert sidebar.entry_at(ENTRIES, 6, 50, STATS).name == "web"
    assert sidebar.entry_at(ENTRIES, 7, 50, STATS) is None
    assert sidebar.entry_at(ENTRIES, 9, 50, STATS).name == "vehicle.k265"
    assert sidebar.entry_at(ENTRIES, 11, 50, STATS) is None
    assert sidebar.entry_at(ENTRIES, 12, 50, STATS).name == "api"
    assert sidebar.entry_at(ENTRIES, 0, 50, STATS) is None
    assert sidebar.entry_at(ENTRIES, 40, 50, STATS) is None


def test_entry_at_ignores_entries_cut_by_a_short_terminal():
    assert sidebar.entry_at(ENTRIES, 5, 20, STATS).name == "web"
    assert sidebar.entry_at(ENTRIES, 9, 20, STATS) is None  # the 2nd entry, not drawn
    no_quota = sidebar.Stats({}, 0.0, WEEK)
    assert sidebar.entry_at(ENTRIES, 9, 20, no_quota).name == "vehicle.k265"  # more room


RUNS = [sidebar.Run("daily-recap", NOW - 150), sidebar.Run("trending-repos", NOW - 70)]


def test_background_runs_show_under_the_sessions():
    lines = sidebar.column(ENTRIES[:2], "@1", STATS, NOW, height=30, runs=RUNS)
    assert body(lines)[11:15] == [
        "",
        "Background",
        "↻ daily-recap            2m",
        "↻ trending-repos         1m",
    ]
    assert any(s.style == words.STYLES["magenta"] for s in lines[13].spans)


def test_no_runs_no_background_section():
    lines = sidebar.column(ENTRIES[:2], "@1", STATS, NOW, height=30)
    assert not any("Background" in ln.plain for ln in lines)


def test_short_terminal_drops_runs_before_sessions():
    lines = sidebar.column(ENTRIES[:2], "@1", STATS, NOW, 24, RUNS)
    assert len(lines) == 24 and any("vehicle.k265" in ln.plain for ln in lines)
    assert not any("trending-repos" in ln.plain for ln in lines)


def test_today_goes_first_when_the_sessions_need_its_room():
    day = usage.Day(sessions=2)
    stats = sidebar.Stats(LIMITS, 1.0, WEEK, day)
    roomy = body(sidebar.column(ENTRIES[:2], "@1", stats, NOW, height=40))
    assert "TODAY                 $1.00" in roomy and "waited 0m · sessions 2" in roomy
    tight = body(sidebar.column(ENTRIES[:2], "@1", stats, NOW, height=26))
    assert "TODAY" not in "".join(tight) and "vehicle.k265" in tight
    assert "TODAY" not in "".join(body(sidebar.column(ENTRIES[:2], "@1", STATS, NOW, height=40)))


def test_a_new_run_keeps_its_whole_name():
    name = "x" * (sidebar.CONTENT - 2)
    lines = sidebar.runs_lines([sidebar.Run(name, NOW - 30)], NOW, room=5)
    assert body(lines)[2] == "↻ " + name


def test_the_waiting_time_keeps_a_space_before_it():
    wide = E("@1", "名前名前名前名前名前名前", "permission", NOW - 720, 1.0, 60)
    line = body(sidebar.entry_lines(wide, False, NOW))[0]
    assert line.endswith(" 12m") and len(line) <= sidebar.CONTENT


def test_an_unknown_status_is_listed_with_the_idle_sessions():
    rows = sidebar.plan([E("@9", "odd", "someday", NOW)], room=10)
    assert ("heading", ("IDLE", "soft")) in rows and ("first", rows[1][1]) in rows
    assert rows[1][1].name == "odd"


def test_the_current_session_has_a_lighter_tint():
    assert sidebar.tint("done", True, NOW) != sidebar.tint("done", False, NOW)
    assert sidebar.tint("done", False, NOW).bgcolor.name == "#1d3324"


def test_a_reset_within_a_day_shows_its_time_even_tomorrow():
    tonight = time.mktime((2026, 11, 3, 23, 30, 0, 0, 0, -1))
    assert sidebar._reset(tonight + 90 * 60, tonight) == "01:00"
    assert sidebar._reset(tonight + 3 * 86400, tonight) == "Fri"


def test_a_long_model_fits_alone_on_an_empty_line():
    e = E("@1", "web", "idle", NOW, model="x" * sidebar.CONTENT)
    assert body(sidebar.entry_lines(e, False, NOW))[1] == "x" * sidebar.CONTENT
