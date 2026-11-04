import time
from datetime import date

from claude_sessions.ui import landing
from claude_sessions.ui.usage import Totals

NOW = time.mktime((2026, 11, 3, 14, 7, 0, 0, 0, -1))
DAY = 86400
R = landing.Recent
RECENT = [
    R("1", "api", "refatora o cliente HTTP para retry com backoff", 5.42, NOW - 2 * 3600, "/a"),
    R("2", "vehicle.k265", "comunicação com tablet (VP-562, VP-568)", 8.10, NOW - 5 * 3600, "/v"),
    R("3", "web", "corrige o fluxo de login com SSO", 3.10, NOW - DAY, "/w"),
    R("4", "claude-sessions", "estado para comando shell em background", 4.77, NOW - DAY, "/c"),
    R("5", "docs", "atualiza o guia de instalação", 0.42, NOW - 6 * DAY, "/d"),
]
F = landing.Folder
FOLDERS = [
    F("~/workspace/api", "main", 3),
    F("~/workspace/vehicle.k265", "VP-562", 0),
    F("~/workspace/web", "feat/login", 1),
    F("~/Documents/claude-sessions", "main", 0),
]
LIMITS = {
    "five_hour": {"pct": 58, "resets_at": NOW + 2 * 3600 + 33 * 60},
    "seven_day": {"pct": 31, "resets_at": time.mktime((2026, 11, 6, 9, 0, 0, 0, 0, -1))},
}
TOTALS = Totals(
    today=9.59,
    week=84.30,
    by_project=[
        ("api", 31.20),
        ("vehicle.k265", 22.75),
        ("web", 12.04),
        ("claude-sessions", 10.90),
        ("docs", 2.10),
    ],
    added=4210,
    removed=1388,
    sessions=23,
)
ACTIVITY = [2, 4, 3, 0, 0, 5, 6, 4, 3, 5, 0, 1, 4, 3]
D20_10 = date(2026, 10, 20)


def test_left_column_matches_the_spec():
    lines = [x.plain.rstrip() for x in landing.left_lines(RECENT, FOLDERS, 0, None, NOW)]
    assert lines == [
        " ▐▛███▜▌", "▝▜█████▛▘", "  ▘▘ ▝▝    claude-sessions", "",
        "Recent sessions  / search",
        "› api              refatora o cliente HTTP para … $5.42  2h",
        "  vehicle.k265     comunicação com tablet (VP-56… $8.10  5h",
        "  web              corrige o fluxo de login com … $3.10  yesterday",
        "  claude-sessions  estado para comando shell em … $4.77  yesterday",
        "  docs             atualiza o guia de instalação  $0.42  Wed",
        "", "New session in…  n",
        "  ~/workspace/api              main       3 changed",
        "  ~/workspace/vehicle.k265     VP-562     clean",
        "  ~/workspace/web              feat/login 1 changed",
        "  ~/Documents/claude-sessions  main       clean",
        "  other folder…",
    ]  # fmt: skip


def test_right_column_matches_the_spec():
    lines = [x.plain.rstrip() for x in landing.right_lines(LIMITS, TOTALS, ACTIVITY, D20_10, NOW)]
    assert lines == [
        "", "Usage", "5h   █████████░░░░░░░ 58%  resets 16:40",
        "7d   █████░░░░░░░░░░░ 31%  resets Fri 09:00", "today $9.59   7 days $84.30",
        "", "Spend 7d by project",
        "api              ██████████████ $31.20", "vehicle.k265     ██████████░░░░ $22.75",
        "web              █████░░░░░░░░░ $12.04", "claude-sessions  █████░░░░░░░░░ $10.90",
        "docs             █░░░░░░░░░░░░░ $2.10",
        "", "Sessions per day (14 days)", "▄ ▆ ▅     ▇ █ ▆ ▅ ▇   ▃ ▆ ▅",
        "20 Oct" + " " * 16 + "today",
        "", "Week", "+4,210 -1,388 lines in 23 sessions",
    ]  # fmt: skip


def test_every_left_row_fits_the_left_column():
    for selected in (0, 4):
        lines = landing.left_lines(RECENT, FOLDERS, selected, None, NOW)
        assert all(x.cell_len <= landing.LEFT_WIDTH for x in lines)


def test_right_column_is_as_wide_as_its_widest_row():
    lines = landing.right_lines(LIMITS, TOTALS, ACTIVITY, D20_10, NOW)
    widths = [x.cell_len for x in lines]
    assert max(widths) == landing.RIGHT_WIDTH


def test_selection_moves_to_folders_and_other_folder():
    lines = [x.plain for x in landing.left_lines(RECENT, FOLDERS, 9, None, NOW)]
    assert lines[-1].startswith("› other folder…")
    lines = [x.plain for x in landing.left_lines(RECENT, FOLDERS, 6, None, NOW)]
    assert lines[-4].startswith("› ~/workspace/vehicle.k265")


def test_search_replaces_the_hint():
    lines = [x.plain for x in landing.left_lines(RECENT[:1], FOLDERS, 0, "api", NOW)]
    assert lines[4].startswith("Recent sessions  / api")


def test_not_a_git_folder_shows_no_branch_or_state():
    lines = landing.left_lines([], [landing.Folder("~/notes", "", None)], 1, None, NOW)
    assert lines[-2].plain.rstrip() == "  ~/notes"


def test_unknown_cost_leaves_the_cell_blank():
    one = [landing.Recent("1", "api", "x", None, NOW - 7200, "/a")]
    line = landing.left_lines(one, [], 0, None, NOW)[5].plain
    assert line.rstrip() == "› api              x" + " " * 30 + " " * 6 + " 2h"


def test_no_limits_leaves_quota_out():
    lines = [x.plain for x in landing.right_lines({}, TOTALS, ACTIVITY, D20_10, NOW)]
    assert not any(x.startswith("5h") for x in lines)
    assert any("today $9.59" in x for x in lines)


def test_screen_sits_side_by_side_when_wide_and_stacks_when_narrow():
    left = landing.left_lines(RECENT, FOLDERS, 0, None, NOW)
    right = landing.right_lines(LIMITS, TOTALS, ACTIVITY, D20_10, NOW)
    wide = landing.screen(left, right, 120)
    assert wide[1].plain[66:70] == " " * 4 and wide[1].plain[70:75] == "Usage"
    assert wide[-1].plain == landing.FOOTER and wide[-2].plain == ""
    narrow = landing.screen(left, right, 80)
    assert len(narrow) == len(left) + len(right) + 2
    assert narrow[len(left)].plain == ""


def test_a_long_branch_is_cut_before_the_git_state():
    folder = landing.Folder("~/notes", "feat/sidebar-landing", 2)
    line = landing.left_lines([], [folder], 0, None, NOW)[-2].plain
    assert "feat/side… 2 " in line
