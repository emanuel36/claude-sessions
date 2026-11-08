from claude_sessions.ui import new_panel
from claude_sessions.ui.new_panel import Folder, Session

NOW = 1_700_000_000.0
SESSIONS = [
    Session(f"s{i}", f"task {i}", NOW - 3600 * i, 1.5 if i == 0 else None) for i in range(7)
]
FOLDERS = [
    Folder("/w/api", "api", NOW - 60, 41, 12.5, SESSIONS),
    Folder("/w/web", "web", NOW - 7200, 1, 0.0, SESSIONS[:2]),
]


def plain(line):
    return line.plain.rstrip()


def test_rows_list_folders_and_open_a_folders_last_five_sessions():
    shown = new_panel.rows(FOLDERS, "", {"/w/api"}, lambda p: False)
    assert [(r.kind, r.path) for r in shown] == [
        ("folder", "/w/api"),
        *[("session", "/w/api")] * 5,
        ("folder", "/w/web"),
    ]
    assert [r.session.id for r in shown if r.kind == "session"] == ["s0", "s1", "s2", "s3", "s4"]
    assert [r.end for r in shown if r.kind == "session"] == [False] * 4 + [True]


def test_a_folders_sessions_scroll_past_the_last_and_back():
    p = new_panel.Picker(lambda path: False)
    p.folders = FOLDERS
    p.key("right")
    for _ in range(7):
        p.key("down")
    # Two steps past the fifth session scroll by two; the cursor stays on the last line
    assert p.cursor == 5 and p.scroll == {"/w/api": 2}
    ids = [r.session.id for r in p.shown() if r.kind == "session"]
    assert ids == [f"s{i}" for i in range(2, 7)]
    last = new_panel.session_line(p.selected(), NOW).plain
    assert last.startswith("     └ task 6")
    # With nothing older left, the cursor moves on to the next folder
    p.key("down")
    assert p.selected().path == "/w/web"
    # Back up to the first session on show, then one more scrolls back by one
    for _ in range(6):
        p.key("up")
    assert p.cursor == 1 and p.scroll == {"/w/api": 1}
    p.key("left")
    assert not p.scroll and not p.expanded


def test_a_folder_stays_closed_until_expanded():
    shown = new_panel.rows(FOLDERS, "", set(), lambda p: False)
    assert [r.kind for r in shown] == ["folder", "folder"]


def test_typing_filters_by_every_word_and_a_typed_folder_comes_first():
    assert [r.path for r in new_panel.rows(FOLDERS, "W E", set(), lambda p: False)] == ["/w/web"]
    shown = new_panel.rows(FOLDERS, "~/x", set(), lambda p: True)
    assert shown[0].kind == "path" and shown[0].path == "~/x"
    assert new_panel.rows(FOLDERS, "/nope", set(), lambda p: False) == []


def test_screen_draws_two_lines_per_folder_and_the_open_sessions():
    shown = new_panel.rows(FOLDERS, "", {"/w/api"}, lambda p: False)
    lines = new_panel.screen(shown, 0, "ap", {"/w/api"}, NOW, 16)
    assert len(lines) == 16 and all(len(plain(x)) <= new_panel.WIDTH for x in lines)
    text = [plain(x) for x in lines]
    assert text[0] == " ❯ ap▌"
    assert text[2].startswith(" ▾ api") and text[2].endswith("1m ago")
    assert text[3] == "   41 sessions · $12.50 7d"
    assert text[4].startswith("     ├ task 0") and text[4].endswith("$1.50")
    assert text[6].startswith("     ├ task 2") and "2h" in text[6]
    # More sessions scroll into view below: a mark, no closing branch yet
    assert text[8].startswith("     ▼ task 4")
    assert text[9].strip() == "─" * (new_panel.WIDTH - 1)
    # No spend this week: no spend part
    assert text[10].startswith(" ▸ web") and text[11] == "   1 session"
    assert text[-1].strip().startswith("↑↓ move")
    # Both lines of the selected folder have the highlight
    for line in lines[2:4]:
        assert any(s.style == new_panel.words.CURRENT for s in line.spans)
    assert not any(s.style == new_panel.words.CURRENT for s in lines[10].spans)


def test_the_list_scrolls_to_keep_the_selected_row_in_view():
    many = [Folder(f"/w/f{i}", f"f{i}", NOW - i, 1) for i in range(30)]
    shown = new_panel.rows(many, "", set(), lambda p: False)
    lines = [plain(x) for x in new_panel.list_lines(shown, 25, set(), NOW, 8)]
    assert len(lines) == 8 and any(" f25" in x for x in lines) and lines[-1] == "▼  1 session"
    assert not any(" f0" in x for x in lines)
    # At the bottom of the list nothing more lies below
    end = [plain(x) for x in new_panel.list_lines(shown, 29, set(), NOW, 8)]
    assert end[-1] == "   1 session"


def test_nothing_matching_says_so():
    assert plain(new_panel.list_lines([], 0, set(), NOW, 8)[0]) == "   nothing matches"


def test_the_picker_moves_opens_and_types():
    p = new_panel.Picker(lambda path: False)
    p.folders = FOLDERS
    assert p.key("right") is None and "/w/api" in p.expanded
    p.key("down")
    assert p.key("enter").session.id == "s0"
    p.key("left")
    assert p.cursor == 0 and not p.expanded
    p.key("x", "w")
    assert p.query == "w" and p.cursor == 0
    assert new_panel.command(p.selected()) == ["-c", "/w/api", "claude"]
