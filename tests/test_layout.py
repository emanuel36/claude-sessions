from claude_sessions.panel.view import layout


def test_layout_gives_recent_rows_a_folder_only_when_wide():
    assert layout.layout(100, [], True).project == layout.PROJECT_WIDTH
    assert layout.layout(60, [], True).project == 0
    assert layout.layout(100, ["x" * 30], True).right == 30


def test_draw_aligns_cells_and_fills_the_width():
    cells = [layout.Cell("ab", 4), layout.Cell("cd", 4, align="right"), layout.Cell("toolong", 3)]
    assert layout.draw(cells, 14).plain == "ab    cdtoo   "
    assert layout.draw(cells, 5).plain == "ab   "
