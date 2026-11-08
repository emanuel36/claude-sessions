"""Renders the Python screens for each golden fixture to the neutral form the Rust tests compare.

Run from the repository root with `uv run python tools/golden.py`. It reads
`tests/golden/fixtures.json` and writes `tests/golden/expected/<name>.json`: an array of lines,
each an array of runs `{"text", "fg", "bg", "bold"}`. Adjacent runs of equal style are merged and
empty runs dropped. Colours are `default`, `idx:N` (the 16 standard colours and `color(N)`) or
`rgb:#rrggbb`.
"""

import os
import time

# Before anything reads local time: the landing header's date and the day boundaries
os.environ["TZ"] = "UTC"
time.tzset()

import datetime  # noqa: E402
import json  # noqa: E402
import pathlib  # noqa: E402

from rich.color import ColorType  # noqa: E402
from rich.console import Console  # noqa: E402
from rich.text import Text  # noqa: E402

from claude_sessions.ui import landing, new_panel, sidebar, usage  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
GOLDEN = ROOT / "tests" / "golden"
CONSOLE = Console(width=1000, color_system="truecolor", force_terminal=True)
# Style attributes the neutral form cannot carry: a screen that sets one fails generation
UNSUPPORTED = ("dim", "italic", "underline", "blink", "blink2", "reverse", "conceal", "strike")


def colour(color):
    """A rich colour in the neutral form.

    Raises:
        ValueError: For a colour type the neutral form has no name for.
    """
    if color is None or color.type == ColorType.DEFAULT:
        return "default"
    if color.type in (ColorType.STANDARD, ColorType.EIGHT_BIT):
        return f"idx:{color.number}"
    if color.type == ColorType.TRUECOLOR:
        return f"rgb:{color.triplet.hex}"
    raise ValueError(f"no neutral name for {color!r}")


def runs(line):
    """A rich line as runs, each segment's style as the terminal sees it.

    The line goes through `Text.join`, as the apps draw their lines, so its own style (a
    background across the line) is laid under every span.
    """
    out = []
    for segment in Text("\n").join([line]).render(CONSOLE):
        if not segment.text:
            continue
        style = segment.style
        if style is not None:
            for name in UNSUPPORTED:
                if getattr(style, name):
                    raise ValueError(f"unsupported style {name} in {line.plain!r}")
            if style.link:
                raise ValueError(f"unsupported link in {line.plain!r}")
        run = {
            "text": segment.text,
            "fg": colour(style.color if style else None),
            "bg": colour(style.bgcolor if style else None),
            "bold": bool(style and style.bold),
        }
        last = out[-1] if out else None
        if last and all(last[k] == run[k] for k in ("fg", "bg", "bold")):
            last["text"] += run["text"]
        else:
            out.append(run)
    return out


def stats(raw):
    """A `sidebar.Stats` from a fixture's `stats`."""
    day = raw["day"]
    totals = raw["totals"]
    return sidebar.Stats(
        limits=raw["limits"],
        today=raw["today"],
        week=[(datetime.date.fromisoformat(d), v) for d, v in raw["week"]],
        day=None if day is None else usage.Day(**day),
        totals=None
        if totals is None
        else usage.Totals(
            by_project=[tuple(p) for p in totals["by_project"]],
            by_model=[tuple(p) for p in totals["by_model"]],
            added=totals["added"],
            removed=totals["removed"],
            sessions=totals["sessions"],
        ),
        hours=raw["hours"],
    )


def picker(fixture):
    """A `new_panel.Picker` holding a fixture's folders and what the user did in the list."""
    dirs = set(fixture["dirs"])
    p = new_panel.Picker(lambda path: path in dirs)
    p.folders = [
        new_panel.Folder(
            **{k: v for k, v in f.items() if k != "sessions"},
            sessions=[new_panel.Session(**s) for s in f["sessions"]],
        )
        for f in fixture["folders"]
    ]
    p.query = fixture["query"]
    p.cursor = fixture["cursor"]
    p.expanded = set(fixture["expanded"])
    p.scroll = dict(fixture["scroll"])
    return p


def sidebar_screen(f):
    lines = sidebar.column(
        [sidebar.Entry(**e) for e in f["entries"]],
        f["current"],
        stats(f["stats"]),
        f["now"],
        f["height"],
        [sidebar.Run(**r) for r in f["runs"]],
    )
    assert all(line.cell_len == sidebar.WIDTH for line in lines), f["name"]
    return lines


def landing_screen(f):
    p, st = picker(f), stats(f["stats"])
    return landing.screen(p, f["searching"], f["opened"], st, f["now"], f["width"], f["height"])


def panel_screen(f):
    p = picker(f)
    shown = p.shown()
    return new_panel.screen(shown, p.cursor, p.query, p.expanded, f["now"], f["height"])


SCREENS = {"sidebar": sidebar_screen, "landing": landing_screen, "panel": panel_screen}


def main():
    fixtures = json.loads((GOLDEN / "fixtures.json").read_text(encoding="utf-8"))
    out_dir = GOLDEN / "expected"
    out_dir.mkdir(exist_ok=True)
    for f in fixtures:
        lines = SCREENS[f["kind"]](f)
        assert len(lines) == f["height"], (f["name"], len(lines))
        body = ",\n".join(json.dumps(runs(line), ensure_ascii=False) for line in lines)
        (out_dir / f"{f['name']}.json").write_text(f"[\n{body}\n]\n", encoding="utf-8")
        print(f"{f['name']}: {len(lines)} lines")


if __name__ == "__main__":
    main()
