import time

from claude_sessions.panel.view import widgets


def test_hour_axis_thins_out_when_dense():
    start = time.mktime((2026, 11, 2, 0, 0, 0, 0, 0, -1))
    labels = widgets.hour_axis(start, start + 12 * 3600, 36)
    assert [t for _, t in labels] == ["00:00", "02:00", "04:00", "06:00", "08:00", "10:00"]
    # Three cells an hour: a label every two hours; 12:00 would run past the edge
    assert [c for c, _ in labels] == [0, 6, 12, 18, 24, 30]
