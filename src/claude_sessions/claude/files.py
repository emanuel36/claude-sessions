"""Claude Code's own files: the prompt history and the session transcripts.

They live under CLAUDE_CONFIG_DIR, ~/.claude by default (see `paths.config_dir`).
"""

import json
import os
import re
from dataclasses import dataclass

from claude_sessions.core import paths

HISTORY = "history.jsonl"
PROJECTS = "projects"
TITLE_TAIL_BYTES = 512 * 1024
INTERRUPT_MARKER = "[Request interrupted by user"
INTERRUPT_TAIL_BYTES = 8192
# A title the user set with /rename wins over the one Claude Code made up
TITLE_KEYS = (b"customTitle", b"aiTitle")
# The rest of a JSON string after its opening quote, up to the closing one
STRING_REST = re.compile(rb'((?:[^"\\]|\\.)*)"')
# Where the titles found in the transcripts are kept between runs
TITLES_FILE = "titles.json"


@dataclass
class HistorySession:
    """A session from Claude Code's prompt history."""

    id: str
    cwd: str
    last: float = 0.0
    first_prompt: str = ""


def history_path():
    """Returns the path of Claude Code's prompt history."""
    return os.path.join(paths.config_dir(), HISTORY)


def read_history(lines):
    """Groups Claude Code's prompt history by session, most recent first."""
    sessions = {}
    for line in lines:
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        if not isinstance(entry, dict) or not entry.get("sessionId") or not entry.get("project"):
            continue
        s = sessions.setdefault(
            entry["sessionId"], HistorySession(entry["sessionId"], entry["project"])
        )
        stamp = (entry.get("timestamp") or 0) / 1000
        s.last = max(s.last, stamp)
        s.cwd = entry["project"]
        prompt = str(entry.get("display") or "")
        # A command such as /resume says nothing about the session
        if not s.first_prompt and not prompt.startswith("/"):
            s.first_prompt = prompt
    return sorted(sessions.values(), key=lambda s: s.last, reverse=True)


_cache = {"key": None, "sessions": []}


def load_history():
    """Returns the parsed prompt history, read again only when the file changes."""
    path = history_path()
    try:
        st = os.stat(path)
        key = (path, st.st_mtime_ns, st.st_size)
        if key != _cache["key"]:
            with open(path, errors="replace") as f:
                _cache.update(key=key, sessions=read_history(f))
    except OSError:
        _cache.update(key=None, sessions=[])
    return _cache["sessions"]


def transcript_path(session_id, cwd):
    """Where Claude Code keeps a session's transcript: one folder per project path."""
    folder = re.sub(r"[^A-Za-z0-9]", "-", cwd)
    return os.path.join(paths.config_dir(), PROJECTS, folder, f"{session_id}.jsonl")


def tail(path, n):
    """Returns the last `n` bytes of a file, or all of it when it is shorter."""
    with open(path, "rb") as f:
        f.seek(0, os.SEEK_END)
        f.seek(max(0, f.tell() - n))
        return f.read()


def was_interrupted(transcript):
    """Tells whether the transcript's last user entry is an Esc interrupt marker."""
    try:
        chunk = tail(transcript, INTERRUPT_TAIL_BYTES).decode("utf-8", "replace")
    except (OSError, TypeError):
        return False
    for line in reversed(chunk.splitlines()):
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if entry.get("type") == "user":
            return _is_interrupt(entry.get("message"))
        if entry.get("type") == "assistant":
            return False
    return False


def _is_interrupt(message):
    """Tells whether a user message is the marker itself, not a tool result that quotes it."""
    content = message.get("content") if isinstance(message, dict) else None
    if isinstance(content, str):
        return content.startswith(INTERRUPT_MARKER)
    if isinstance(content, list):
        return any(
            isinstance(block, dict)
            and block.get("type") == "text"
            and str(block.get("text", "")).startswith(INTERRUPT_MARKER)
            for block in content
        )
    return False


def find_title(chunk):
    """Returns the latest title in a transcript's tail, preferring one the user set."""
    for key in TITLE_KEYS:
        title = _last_string(chunk, key)
        if title:
            return title
    return ""


def _last_string(chunk, key):
    """The last string value of `key` in a chunk of JSON lines that parses, or None.

    Searched from the end: a long transcript holds the key many times, and only the last counts.
    """
    marker = b'"' + key + b'":"'
    end = len(chunk)
    while (at := chunk.rfind(marker, 0, end)) >= 0:
        end = at
        m = STRING_REST.match(chunk, at + len(marker))
        if m is None:
            continue
        try:
            return json.loads(b'"' + m.group(1) + b'"')
        except ValueError:
            continue
    return None


def title_of(transcript):
    """Returns a session's title from its transcript, '' when it has none or cannot be read."""
    try:
        return find_title(tail(transcript, TITLE_TAIL_BYTES))
    except (OSError, TypeError):
        return ""


def titles_path():
    """Where the titles found in the transcripts are kept between runs."""
    return os.path.join(paths.state_home(), TITLES_FILE)


class Titles:
    """Session titles, read from the transcripts again only when one changes.

    With `stored`, they are also kept on disk, so a new process (the Alt+n panel) reads only
    the transcripts that changed since the last one.
    """

    def __init__(self, stored=False):
        self.stored = stored
        self.seen = None
        # The sessions asked for since the last save: the only ones it keeps
        self.used = set()
        self.changed = False

    def _load(self):
        self.seen = {}
        if not self.stored:
            return
        try:
            with open(titles_path(), encoding="utf-8") as f:
                saved = json.load(f)
        except (OSError, ValueError):
            return
        if isinstance(saved, dict):
            self.seen = {
                sid: (v[0], v[1])
                for sid, v in saved.items()
                if isinstance(v, list) and len(v) == 2 and isinstance(v[1], str)
            }

    def get(self, session_id, path):
        """Returns a session's title from its transcript, or '' if it has none yet."""
        if self.seen is None:
            self._load()
        if self.stored:
            self.used.add(session_id)
        try:
            mtime = os.stat(path).st_mtime_ns
        except (OSError, TypeError):
            mtime = None
        cached = self.seen.get(session_id)
        if cached and cached[0] == mtime:
            return cached[1]
        title = title_of(path) if mtime is not None else ""
        self.seen[session_id] = (mtime, title)
        self.changed = True
        return title

    def save(self):
        """Writes the titles asked for since the last save to disk, if any changed; never raises."""
        if not (self.stored and self.changed):
            return
        kept = {sid: list(v) for sid, v in self.seen.items() if sid in self.used}
        try:
            os.makedirs(paths.state_home(), exist_ok=True)
            paths.write_json(titles_path(), kept)
        except OSError:
            return
        self.used, self.changed = set(), False
