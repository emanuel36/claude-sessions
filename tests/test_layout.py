from claude_sessions.panel.view import layout


def names(lay):
    return [name for name, _, _ in lay.columns]


def test_layout_shrinks_the_title_then_drops_columns_from_the_right():
    assert names(layout.layout(120, [], True)) == ["cost", "time", "model", "lines"]
    narrow = layout.layout(80, [], True)
    assert names(narrow) == ["cost", "time", "model"] and narrow.title >= layout.MIN_TITLE
    assert names(layout.layout(40, [], True)) == ["cost", "time"]


def test_layout_gives_recent_rows_a_folder_only_when_wide():
    assert layout.layout(100, [], False).project == layout.PROJECT_WIDTH
    assert layout.layout(60, [], False).project == 0
    assert layout.layout(100, ["x" * 30], False).right == 30


def test_draw_aligns_cells_and_fills_the_width():
    cells = [layout.Cell("ab", 4), layout.Cell("cd", 4, align="right"), layout.Cell("toolong", 3)]
    assert layout.draw(cells, 14).plain == "ab    cdtoo   "
    assert layout.draw(cells, 5).plain == "ab   "
