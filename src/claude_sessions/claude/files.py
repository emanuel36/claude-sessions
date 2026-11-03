"""Claude Code's own files: the session transcripts."""

import json
import os

INTERRUPT_MARKER = "[Request interrupted by user"
INTERRUPT_TAIL_BYTES = 8192


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
