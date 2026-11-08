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


ELBOW = "  ⎿  "


def test_column_matches_the_spec_text():
    day = usage.Day(added=1376)
    day.removed, day.sessions, day.waited = 58, 4, 720
    stats = sidebar.Stats(LIMITS, 9.59, WEEK, day)
    lines = sidebar.column(ENTRIES, "@3", stats, NOW, height=50)
    assert len(lines) == 50
    assert all(len(ln.plain) == sidebar.WIDTH == 29 for ln in lines)
    spin = sidebar.spinner(NOW)
    assert body(lines)[:20] == [
        "",
        " " * 9 + " ▐▛███▜▌", " " * 9 + "▝▜█████▛▘", " " * 9 + "  ▘▘ ▝▝", "",
        "• web                    3m", ELBOW + "approve · $3.10 · 40m",
        "",
        spin + " vehicle.k265", ELBOW + "working · $0.90 · 1h12",
        "",
        "• api                    6m", ELBOW + "running · $5.42 · 2h05",
        "",
        "• docs", ELBOW + "done · $0.12 · 15m",
        "",
        "• notes", ELBOW + "$0.05 · 3h00",
        "",
    ]  # fmt: skip
    assert body(lines)[-17:] == [
        "",
        "• Today               $9.59",
        ELBOW + "+1,376 −58 lines",
        "     4 sessions · 12m wait",
        "",
        "• Usage",
        ELBOW + "5h ▄▄▄▄▄▄▄▄  58% 2h33",
        "     7d ▄▄▄▄▄▄▄▄  31% 2d18h",
        "",
        "• Spend 7d           $24.00",
        ELBOW + "We                $0.0",
        "     Th ▄▄▄▄           $2.0",
        "     Fr ▄▄▄▄▄▄▄        $4.0",
        "     Sa ▄▄▄▄▄▄▄▄▄▄     $6.0",
        "     Su ▄▄▄▄▄▄▄▄▄▄▄▄▄▄ $8.0",
        "     Mo ▄▄             $1.0",
        "     Tu ▄▄▄▄▄          $3.0",
    ]  # fmt: skip
    # Today's row is bold
    assert any(s.style == words.STYLES["strong"] for s in lines[-1].spans)


def test_a_week_with_no_spend_draws_empty_rows():
    empty = sidebar.Stats({}, 0.0, [(d, 0.0) for d, _ in WEEK])
    assert body(sidebar.stats_lines(empty, NOW))[-1] == "     Tu                $0.0"


def test_hbar_draws_at_least_one_cell_for_any_spend():
    assert sidebar.hbar(1, 2, 4) == "▄▄"
    assert sidebar.hbar(0.001, 100, 4) == "▄"
    assert sidebar.hbar(0, 0, 4) == ""


def test_the_panel_adds_spend_by_folder_and_the_week():
    day = usage.Day(added=1376, removed=58, sessions=4, waited=720)
    by = [("claude-sessions", 18.4), ("api", 9.3), ("dotfiles", 2.1), ("notes", 2.0), ("x", 1.0)]
    hours = [0.0] * 24
    hours[9] = 3600
    models = [("Opus 5.5", 20.0), ("Sonnet 5.5", 4.0)]
    totals = usage.Totals(by_project=by, by_model=models, added=8940, removed=2115, sessions=38)
    stats = sidebar.Stats(LIMITS, 9.59, WEEK, day, totals, hours)
    lines = [ln.plain.rstrip() for ln in sidebar.panel_lines(stats, NOW)]
    assert all(len(ln.plain) == sidebar.PANEL_WIDTH for ln in sidebar.panel_lines(stats, NOW))
    assert lines[:8] == [
        " • Today                                          $9.59",
        "   ⎿  4 sessions · 12m wait",
        "      " + "·" * 9 + "█" + "·" * 14,
        "      0     6     12    18",
        "",
        " • Lines",
        "   ⎿  today  +1,376 −58",
        "      7d     +8,940 −2,115 · 38 sessions",
    ]
    assert lines[10].endswith("58% resets in 2h33")
    assert lines[10].index("%") == lines[11].index("%")
    assert lines[14] == "   ⎿  Wed                                         $0.00"
    assert lines[18].endswith("▄ $8.00")
    assert lines[23].startswith("   ⎿  claude-sessions  ") and lines[23].endswith("$18.40")
    assert not any(" x " in ln for ln in lines)
    assert lines[-3] == " • By model 7d"
    assert lines[-2].startswith("   ⎿  Opus 5.5         ") and lines[-2].endswith("$20.00")
    assert lines[-1].startswith("      Sonnet 5.5       ") and lines[-1].endswith("$4.00")
    # No work recorded today: no hours under Today
    stats.hours = [0.0] * 24
    assert len(sidebar.panel_lines(stats, NOW)) == len(lines) - 2


def test_active_hours_shade_each_hour_by_the_busiest():
    hours = [0.0] * 24
    hours[9], hours[10], hours[14] = 3600, 1800, 100
    # Wider than the sidebar: only the landing page's panel draws it
    lines = [ln.plain[1:].rstrip() for ln in sidebar.hours_lines(hours)]
    assert lines[0] == "     " + "·" * 9 + "█▒···░" + "·" * 9
    assert lines[1] == "     0     6     12    18"
    assert sidebar.hours_lines([0.0] * 24) == []


def test_name_uses_the_room_the_waiting_time_leaves():
    long = E("@1", "claude-sessions-and-more-of-it", "permission", NOW - 180, 1.0, 60)
    assert body(sidebar.entry_lines(long, False, NOW))[0] == "• claude-sessions-and-m… 3m"
    calm = E("@1", "claude-sessions-and-more-of-it", "working", NOW, 1.0, 60)
    assert body(sidebar.entry_lines(calm, False, NOW))[0][2:] == "claude-sessions-and-more…"


def test_the_model_closes_the_second_line_only_when_it_fits_whole():
    short = E("@1", "web", "idle", NOW, 2.78, 600, "Opus 5.5")
    assert body(sidebar.entry_lines(short, False, NOW))[1] == ELBOW + "$2.78 · 10m · Opus 5.5"
    long = E("@1", "web", "permission", NOW, 67.05, 600, "Sonnet 5.5")
    assert body(sidebar.entry_lines(long, False, NOW))[1] == ELBOW + "approve · $67.05 · 10m"


def test_the_current_entry_has_a_strong_name_and_a_lighter_background():
    first, second = sidebar.entry_lines(ENTRIES[2], current=True, now=NOW)
    assert any(s.style == words.STYLES["strong"] for s in first.spans)
    assert len(first.plain) == len(second.plain) == sidebar.WIDTH
    assert first.style == second.style == words.CURRENT
    for e in ENTRIES:
        assert not sidebar.entry_lines(e, False, NOW)[0].style


def test_the_bullet_has_the_status_colour_and_spins_while_working():
    colours = {
        "permission": "error", "input": "warning", "done": "success", "shell": "permission",
        "idle": "inactive",
    }  # fmt: skip
    for status, colour in colours.items():
        first = sidebar.entry_lines(entry(status, 0), False, NOW)[0]
        assert first.plain[1] == "•" and first.spans[0].style == words.STYLES[colour]
    working = entry("working", 0)
    marks = {sidebar.entry_lines(working, False, NOW + t / 8)[0].plain[1] for t in range(6)}
    assert len(marks) == 6 and "•" not in marks


def test_waiting_time_shows_only_for_timed_statuses_from_one_minute():
    assert "10m" not in sidebar.entry_lines(entry("done", waited=600), False, NOW)[0].plain
    fresh = sidebar.entry_lines(entry("permission", waited=30), False, NOW)[0].plain
    assert fresh.strip() == "• web"


def test_unknown_usage_leaves_parts_out():
    lines = sidebar.entry_lines(E("@1", "new", "working", NOW), False, NOW)
    assert body(lines)[1] == ELBOW + "working"
    only_cost = sidebar.entry_lines(E("@1", "new", "idle", NOW, 0.5), False, NOW)
    assert body(only_cost)[1] == ELBOW + "$0.50"


def test_short_terminal_keeps_stats_and_cuts_sessions():
    lines = sidebar.column(ENTRIES, "@3", STATS, NOW, height=20)
    assert len(lines) == 20 and "Tu" in lines[-1].plain and "web" in lines[5].plain
    assert not any("vehicle" in ln.plain for ln in lines)


def test_no_limits_leaves_quota_out():
    lines = body(sidebar.column(ENTRIES, "@3", sidebar.Stats({}, 0.0, WEEK), NOW, height=30))
    assert "• Spend 7d           $24.00" in lines and "• Usage" not in lines


def test_entry_at_maps_rows_to_entries():
    # The blank lines between entries belong to none
    assert sidebar.entry_at(ENTRIES, 4, 50, STATS) is None
    assert sidebar.entry_at(ENTRIES, 5, 50, STATS).name == "web"
    assert sidebar.entry_at(ENTRIES, 6, 50, STATS).name == "web"
    assert sidebar.entry_at(ENTRIES, 7, 50, STATS) is None
    assert sidebar.entry_at(ENTRIES, 8, 50, STATS).name == "vehicle.k265"
    assert sidebar.entry_at(ENTRIES, 10, 50, STATS) is None
    assert sidebar.entry_at(ENTRIES, 11, 50, STATS).name == "api"
    assert sidebar.entry_at(ENTRIES, 0, 50, STATS) is None
    assert sidebar.entry_at(ENTRIES, 40, 50, STATS) is None


def test_entry_at_ignores_entries_cut_by_a_short_terminal():
    assert sidebar.entry_at(ENTRIES, 5, 20, STATS).name == "web"
    assert sidebar.entry_at(ENTRIES, 8, 20, STATS) is None  # the 2nd entry, not drawn
    no_quota = sidebar.Stats({}, 0.0, WEEK)
    assert sidebar.entry_at(ENTRIES, 8, 20, no_quota).name == "vehicle.k265"  # more room


RUNS = [sidebar.Run("daily-recap", NOW - 150), sidebar.Run("trending-repos", NOW - 70)]


def test_background_runs_show_under_the_sessions():
    lines = sidebar.column(ENTRIES[:2], "@1", STATS, NOW, height=30, runs=RUNS)
    assert body(lines)[10:14] == [
        "",
        "• Background",
        (ELBOW + "daily-recap").ljust(25) + "2m",
        "     trending-repos".ljust(25) + "1m",
    ]
    assert any(s.style == words.STYLES["magenta"] for s in lines[12].spans)


def test_no_runs_no_background_section():
    lines = sidebar.column(ENTRIES[:2], "@1", STATS, NOW, height=30)
    assert not any("Background" in ln.plain for ln in lines)


def test_short_terminal_drops_runs_before_sessions():
    lines = sidebar.column(ENTRIES[:2], "@1", STATS, NOW, 24, RUNS)
    assert len(lines) == 24 and any("vehicle.k265" in ln.plain for ln in lines)
    assert not any("daily-recap" in ln.plain for ln in lines)


def test_today_goes_first_when_the_sessions_need_its_room():
    day = usage.Day(sessions=2)
    stats = sidebar.Stats(LIMITS, 1.0, WEEK, day)
    roomy = body(sidebar.column(ENTRIES[:2], "@1", stats, NOW, height=40))
    assert "• Today               $1.00" in roomy and "     2 sessions · 0m wait" in roomy
    tight = body(sidebar.column(ENTRIES[:2], "@1", stats, NOW, height=23))
    assert "Today" not in "".join(tight) and any("vehicle.k265" in x for x in tight)
    assert "Today" not in "".join(body(sidebar.column(ENTRIES[:2], "@1", STATS, NOW, height=40)))


def test_a_new_run_keeps_its_whole_name():
    name = "x" * sidebar.DETAIL_ROOM
    lines = sidebar.runs_lines([sidebar.Run(name, NOW - 30)], NOW, room=5)
    assert body(lines)[2] == ELBOW + name


def test_the_waiting_time_keeps_a_space_before_it():
    wide = E("@1", "名前名前名前名前名前名前", "permission", NOW - 720, 1.0, 60)
    line = body(sidebar.entry_lines(wide, False, NOW))[0]
    assert line.endswith(" 12m") and len(line) <= sidebar.CONTENT


def test_an_unknown_status_is_listed_with_the_idle_sessions():
    odd, notes = E("@9", "odd", "someday", NOW), E("@5", "notes", "idle", NOW)
    assert sidebar.ordered([odd, notes, ENTRIES[0]]) == [ENTRIES[0], odd, notes]
    rows = sidebar.plan([odd], room=10)
    assert rows == [("first", odd), ("second", odd)]


def test_a_reset_shows_the_time_left():
    assert words.remaining(40 * 60) == "40m"
    assert words.remaining(90 * 60) == "1h30"
    assert words.remaining(3 * 86400 + 4 * 3600 + 59) == "3d4h"
    assert words.remaining(86400 + 59) == "1d"
    assert words.remaining(-5) == "0m"


def test_a_long_model_fits_alone_on_an_empty_line():
    e = E("@1", "web", "idle", NOW, model="x" * sidebar.DETAIL_ROOM)
    assert body(sidebar.entry_lines(e, False, NOW))[1] == ELBOW + "x" * sidebar.DETAIL_ROOM


def test_step_follows_the_listed_order_and_wraps():
    # In window order: docs, web, notes, api, vehicle.k265. Listed: web (needs you), api and
    # vehicle.k265 (running, in window order), docs (done), notes (idle)
    shuffled = [ENTRIES[i] for i in (3, 0, 4, 2, 1)]
    order = ["@1", "@3", "@2", "@4", "@5"]
    for i, window in enumerate(order):
        assert sidebar.step(shuffled, window, 1) == order[(i + 1) % 5]
        assert sidebar.step(shuffled, window, -1) == order[i - 1]
    # From the landing page: down to the first, up to the last
    assert sidebar.step(shuffled, "@0", 1) == "@1"
    assert sidebar.step(shuffled, "@0", -1) == "@5"
    assert sidebar.step([], "@0", 1) is None


def test_today_lines_fit_the_column_with_big_numbers():
    day = usage.Day(added=1_234_567, removed=7_654_321, sessions=123, waited=99 * 3600)
    lines = sidebar.today_lines(day, 1234.5)
    assert all(len(ln.plain) <= 1 + sidebar.CONTENT for ln in lines)


def test_looking_at_a_finished_session_does_not_move_it():
    done = [E("@1", "a", "done", NOW), E("@2", "b", "idle", NOW), E("@3", "c", "done", NOW)]
    seen = [done[0], E("@2", "b", "idle", NOW), E("@3", "c", "idle", NOW)]
    assert [e.window_id for e in sidebar.ordered(done)] == ["@1", "@2", "@3"]
    assert [e.window_id for e in sidebar.ordered(seen)] == ["@1", "@2", "@3"]
