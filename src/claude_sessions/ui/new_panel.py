"""The folder list that starts a session: folders by last use, each opening its last sessions.

The Alt+n panel shows it under a search line; the landing page shows it under its header.
"""

import os
import shlex
from dataclasses import dataclass, field

from rich.text import Text

from claude_sessions.ui import words
from claude_sessions.ui.words import STYLES

# How many of a folder's sessions open under it at once; the rest scroll into view
SHOWN = 5
# The list's width in the Alt+n popup; the landing page gives it all the room it has
WIDTH = 48
AGE_WIDTH = 4
SPEND_WIDTH = 8
FOOTER = (("↑↓", "move"), ("→", "sessions"), ("Enter", "new / resume"), ("Esc", ""))
# Marks the last line on show when the list goes on below it
MORE = "▼"


@dataclass
class Session:
    """An earlier session of a folder, to resume."""

    id: str
    title: str
    last: float
    cost: float | None = None


@dataclass
class Folder:
    """A folder Claude ran in: when it was last used, its sessions and its week's spend."""

    path: str
    name: str
    last: float
    count: int = 0
    spend: float = 0.0
    sessions: list[Session] = field(default_factory=list)


@dataclass
class Row:
    """A line of the list: a typed path, a folder, or one of a folder's sessions."""

    kind: str
    path: str
    folder: Folder | None = None
    session: Session | None = None
    # Whether a session is the last of its folder on show, drawn with the closing branch
    end: bool = False


def matches(folder, query):
    """Tells whether a folder's path holds every word of the query."""
    text = folder.path.lower()
    return all(word in text for word in query.lower().split())


def rows(folders, query, expanded, is_dir, scroll=None):
    """The list's rows: a typed path first, then the folders that match, with sessions below.

    Args:
        folders: The folders, last used first.
        query: What the user typed.
        expanded: The paths of the folders showing their sessions.
        is_dir: Tells whether a typed path (with `~` expanded) is a folder.
        scroll: By folder path, how many of its sessions are scrolled past; none by default.
    """
    scroll = scroll or {}
    out = []
    typed = query.strip()
    if typed.startswith(("/", "~")) and is_dir(typed):
        out.append(Row("path", typed))
    for folder in folders:
        if not matches(folder, query):
            continue
        out.append(Row("folder", folder.path, folder))
        if folder.path in expanded:
            start = scroll.get(folder.path, 0)
            window = folder.sessions[start : start + SHOWN]
            out += [Row("session", folder.path, folder, s, end=s is window[-1]) for s in window]
    return out


def _line(*parts):
    out = Text()
    for text, style in parts:
        out.append(text, style=STYLES[style])
    return out


def _rule(width):
    return _line((" " + "─" * (width - 1), "rule"))


def folder_lines(folder, expanded, now, width=WIDTH):
    """A folder's two lines: name and last use, then its sessions and the week's spend."""
    arrow = " ▾ " if folder.path in expanded else " ▸ "
    age = f"{words.age(folder.last, now)} ago"
    first = _line((arrow, "soft"))
    first.append(words.fit(folder.name, width - 4 - len(age)), style=STYLES["strong"])
    first.append(" " * (width - first.cell_len - len(age)))
    first.append(age, style=STYLES["soft"])
    sessions = f"{folder.count} session{'s' * (folder.count != 1)}"
    spend = f" · {words.money(folder.spend)} 7d" if folder.spend else ""
    return [first, _line(("   " + sessions + spend, "soft"))]


def session_line(row, now, width=WIDTH):
    """One of a folder's last sessions: its title, when it was used and what it cost."""
    s = row.session
    # The closing branch only under the folder's very last session: more may scroll into view
    last = s is row.folder.sessions[-1]
    branch = "└ " if last else f"{MORE} " if row.end else "├ "
    line = _line(("     ", "ink"), (branch, "faint"))
    tail = words.age(s.last, now).ljust(AGE_WIDTH) + words.money(s.cost).rjust(SPEND_WIDTH)
    title = width - line.cell_len - len(tail) - 1
    line.append(words.fit(s.title, title).ljust(title + 1), style=STYLES["ink"])
    line.append(tail, style=STYLES["soft"])
    return line


def row_lines(shown, expanded, now, width=WIDTH):
    """Every row's lines, with a rule between folders, and the row each line belongs to."""
    lines, owners = [], []
    for i, row in enumerate(shown):
        if row.kind == "path":
            drawn = [_line(("   + ", "green"), (words.fit(f"open {row.path}", width - 5), "ink"))]
        elif row.kind == "session":
            drawn = [session_line(row, now, width)]
        else:
            if lines:
                lines.append(_rule(width))
                owners.append(None)
            drawn = folder_lines(row.folder, expanded, now, width)
        lines += drawn
        owners += [i] * len(drawn)
    return lines, owners


def list_lines(shown, cursor, expanded, now, room, width=WIDTH):
    """The rows around the selected one, in `room` lines at most, the selection highlighted.

    Args:
        shown: The rows from `rows`.
        cursor: The index of the selected row.
        expanded: The paths of the folders showing their sessions.
        now: Current time as a Unix timestamp.
        room: Lines available.
        width: Columns available.
    """
    if not shown:
        return [_line(("   nothing matches", "faint"))]
    lines, owners = row_lines(shown, expanded, now, width)
    picked = [n for n, owner in enumerate(owners) if owner == cursor]
    # Scrolls just enough to keep every line of the selected row in view
    top = max(0, min(picked[-1] - room + 1, len(lines) - room))
    top = min(top, picked[0])
    out = []
    end = min(len(lines), top + room)
    for n in range(top, end):
        line = lines[n]
        # A session line's branch already marks its folder's own more below
        if n == end - 1 < len(lines) - 1 and line.plain[5:6] != MORE:
            # Its first column is always blank: room for the mark that more lies below
            line = _line((MORE, "faint")) + line[1:]
        if n in picked:
            line.pad_right(width - line.cell_len)
            line.stylize(words.CURRENT)
        out.append(line)
    return out


def keys_line(keys, width):
    """The keys the screen takes, each in Claude's orange before what it does, if anything.

    A line too long for `width` is cut with an ellipsis.
    """
    line = Text(" ")
    for i, (key, does) in enumerate(keys):
        line.append(" · " * bool(i), style=STYLES["faint"])
        line.append(key, style=STYLES["claude"])
        line.append(f" {does}" * bool(does), style=STYLES["ink"])
    if line.cell_len > width:
        line.truncate(width - 1)
        line.append("…", style=STYLES["faint"])
    return line


def screen(shown, cursor, query, expanded, now, height):
    """The Alt+n panel as `height` lines: the search line, the list and the keys."""
    prompt = _line((" ❯ ", "blue"), (query, "ink"), ("▌", "blue"))
    lines = [prompt, Text(), *list_lines(shown, cursor, expanded, now, height - 4)]
    while len(lines) < height - 1:
        lines.append(Text())
    lines.append(keys_line(FOOTER, WIDTH))
    return [Text(" ") if not line.plain else line for line in lines][:height]


class Picker:
    """What the user did in the list: the search, the selected row and the open folders."""

    def __init__(self, is_dir=None):
        self.is_dir = is_dir or (lambda path: os.path.isdir(os.path.expanduser(path)))
        self.folders = []
        self.query = ""
        self.cursor = 0
        self.expanded = set()
        # By folder path, how many of its sessions are scrolled past
        self.scroll = {}

    def shown(self):
        """The rows on show, with the cursor kept on one of them."""
        out = rows(self.folders, self.query, self.expanded, self.is_dir, self.scroll)
        self.cursor = max(0, min(self.cursor, len(out) - 1))
        return out

    def selected(self):
        """The selected row, or None when nothing is listed."""
        shown = self.shown()
        return shown[self.cursor] if shown else None

    def key(self, key, character=None):
        """Applies a key; returns the row to open on Enter, else None.

        Args:
            key: Textual's name for the key, such as `up` or `enter`.
            character: The printable character it types, if any, which goes to the search.
        """
        row = self.selected()
        if key == "enter":
            return row
        if key in ("up", "down") and not self.scrolls(row, key):
            self.cursor += 1 if key == "down" else -1
        elif key == "right" and row and row.kind == "folder":
            self.expanded.add(row.path)
        elif key == "left" and row and row.kind != "path":
            # From a session, back to its folder, which then closes
            self.expanded.discard(row.path)
            self.scroll.pop(row.path, None)
            shown = self.shown()
            self.cursor = next(
                i for i, r in enumerate(shown) if r.kind == "folder" and r.path == row.path
            )
        elif key == "backspace":
            self.query, self.cursor = self.query[:-1], 0
        elif character:
            self.query, self.cursor = self.query + character, 0
        self.shown()
        return None

    def scrolls(self, row, key):
        """Scrolls a folder's sessions when the cursor would leave them with more that way.

        Returns:
            Whether it scrolled, in which case the cursor stays on its line.
        """
        if not row or row.kind != "session":
            return False
        start = self.scroll.get(row.path, 0)
        if key == "down" and row.end and start + SHOWN < len(row.folder.sessions):
            self.scroll[row.path] = start + 1
            return True
        if key == "up" and start and row.session is row.folder.sessions[start]:
            self.scroll[row.path] = start - 1
            return True
        return False


def command(row):
    """The tmux `new-window` arguments that open a row: resume its session, or start one."""
    if row.kind == "session":
        return ["-c", row.path, f"claude --resume {shlex.quote(row.session.id)}"]
    return ["-c", os.path.expanduser(row.path), "claude"]
