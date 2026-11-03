from claude_sessions.core import journal, state
from claude_sessions.panel.data import timeline


def test_timeline_cell_takes_most_urgent():
    events = [
        {"t": 0, "id": "a", "status": state.WORKING},
        # Out of order, as concurrent hooks can log them
        {"t": 600, "id": "a", "status": state.WORKING},
        {"t": 540, "id": "a", "status": state.PERMISSION},
    ]
    row = timeline.timeline({"a": events}, 0, 1200, 2)["a"]
    assert row == [state.PERMISSION, state.WORKING]


def test_timeline_carries_the_status_into_later_cells():
    events = [{"t": 0, "id": "a", "status": state.DONE}]
    assert timeline.timeline({"a": events}, 0, 300, 3)["a"] == [state.DONE] * 3


def test_timeline_none_before_first_event():
    by_id = {
        "a": [{"t": 200, "id": "a", "status": state.IDLE}],
        "b": [{"t": 0, "id": "b", "cost": 1.0}],
    }
    out = timeline.timeline(by_id, 0, 400, 4)
    assert out["a"] == [None, None, state.IDLE, state.IDLE]
    assert out["b"] == [None] * 4


def test_timeline_ended_and_unknown_statuses_leave_cells_empty():
    by_id = {
        "a": [
            {"t": 0, "id": "a", "status": state.WORKING},
            {"t": 100, "id": "a", "status": journal.ENDED},
        ],
        "b": [{"t": 0, "id": "b", "status": "bogus"}],
    }
    out = timeline.timeline(by_id, 0, 300, 3)
    assert out["a"] == [state.WORKING, None, None]
    assert out["b"] == [None] * 3
