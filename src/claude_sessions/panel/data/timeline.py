"""Today's bars without any drawing: each session's most urgent status in each cell of time."""

from claude_sessions.core import state


def timeline(by_id, start, end, cells):
    """Cuts the span from `start` to `end` into cells and gives each session's status in them.

    A cell holds the most urgent status (in `state.STATUSES` order) the session had at any
    moment inside it, the one carried in from before the cell included, so a short permission
    wait stays visible. A cell before the session's first event, after its `ended` event, or in
    a status that is not known has `None`.

    Args:
        by_id: Each session's events of the day, by session id.
        start: Start of the span as a Unix timestamp.
        end: End of the span as a Unix timestamp.
        cells: How many cells to cut the span into.

    Returns:
        The cells of each id, left to right.
    """
    if cells <= 0 or end <= start:
        return {sid: [None] * max(cells, 0) for sid in by_id}
    span = (end - start) / cells
    result = {}
    for sid, events in by_id.items():
        mine = sorted((e for e in events if "status" in e), key=lambda e: e["t"])
        # Each status holds from its event to the next one, the last until the end
        ends = [e["t"] for e in mine[1:]] + [end]
        stretches = [
            (e["t"], stop, _rank(e["status"])) for e, stop in zip(mine, ends, strict=False)
        ]
        row = []
        for i in range(cells):
            low, high = start + i * span, start + (i + 1) * span
            ranks = [r for t0, t1, r in stretches if r is not None and t0 < high and t1 > low]
            row.append(state.STATUSES[min(ranks)] if ranks else None)
        result[sid] = row
    return result


def _rank(status):
    """The urgency of a status, 0 the most; `None` for `ended` and for unknown ones."""
    return state.STATUSES.index(status) if status in state.STATUSES else None
