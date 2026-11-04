import datetime
import json
import os
import time

from claude_sessions.claude import files
from claude_sessions.core import paths


def entry(session_id, project, t, display="hi"):
    return json.dumps(
        {"sessionId": session_id, "project": project, "timestamp": t * 1000, "display": display}
    )


def write_history(*lines):
    os.makedirs(paths.config_dir(), exist_ok=True)
    with open(files.history_path(), "w") as f:
        f.write("\n".join(lines) + "\n")


def test_history_is_grouped_by_session_newest_first():
    lines = [
        entry("a", "/w/api", 5, "/resume"),
        entry("a", "/w/api", 10, "first"),
        entry("b", "/w/notes", 20),
        "not json",
        json.dumps({"display": "no session id", "project": "/w", "timestamp": 1}),
        entry("a", "/w/api", 30, "second"),
    ]
    sessions = files.read_history(lines)
    assert [(s.id, s.last, s.first_prompt) for s in sessions] == [
        ("a", 30, "first"),
        ("b", 20, "hi"),
    ]


def test_history_lives_under_the_claude_config_dir(tmp_path):
    assert files.history_path() == str(tmp_path / "claude" / "history.jsonl")


def test_load_history_reads_again_only_when_the_file_changes():
    assert files.load_history() == []
    write_history(entry("a", "/w/api", 10))
    assert [s.id for s in files.load_history()] == ["a"]
    write_history(entry("a", "/w/api", 10), entry("b", "/w/api", 20))
    assert [s.id for s in files.load_history()] == ["b", "a"]


def test_transcript_path_follows_claude_code_folder_names(tmp_path):
    path = files.transcript_path("abc", "/home/me/projects/app")
    assert path == str(tmp_path / "claude" / "projects" / "-home-me-projects-app" / "abc.jsonl")


def test_a_title_the_user_set_wins():
    tail = (
        b'{"type":"ai-title","aiTitle":"Old"}\n{"type":"ai-title","aiTitle":"Fix \\"x\\""}\n'
        b'{"type":"user"}\n'
    )
    assert files.find_title(tail) == 'Fix "x"'
    assert files.find_title(tail + b'{"type":"custom-title","customTitle":"Mine"}') == "Mine"
    assert files.find_title(b'{"aiTitle":"cut') == ""


def test_titles_read_again_when_the_transcript_changes(tmp_path):
    path = tmp_path / "t.jsonl"
    titles = files.Titles()
    assert titles.get("a", str(path)) == ""
    path.write_bytes(b'{"aiTitle":"First"}\n')
    assert titles.get("a", str(path)) == "First"
    path.write_bytes(b'{"aiTitle":"First"}\n{"customTitle":"Mine"}\n')
    os.utime(path, ns=(1, 2))
    assert titles.get("a", str(path)) == "Mine"
    assert titles.get("b", None) == ""


def test_tail_reads_the_last_bytes_of_a_file(tmp_path):
    path = tmp_path / "f"
    path.write_bytes(b"0123456789")
    assert files.tail(path, 4) == b"6789"
    assert files.tail(path, 100) == b"0123456789"


def write_transcript(path, *entries):
    path.write_text("".join(json.dumps(e) + "\n" for e in entries))
    return str(path)


def test_the_marker_inside_a_tool_result_is_not_an_interrupt(tmp_path):
    result = {"type": "tool_result", "content": "grep: [Request interrupted by user]"}
    transcript = write_transcript(
        tmp_path / "t.jsonl", {"type": "user", "message": {"content": [result]}}
    )
    assert not files.was_interrupted(transcript)


def test_an_interrupt_in_a_text_block_counts(tmp_path):
    block = {"type": "text", "text": "[Request interrupted by user for tool use]"}
    transcript = write_transcript(
        tmp_path / "t.jsonl", {"type": "user", "message": {"content": [block]}}
    )
    assert files.was_interrupted(transcript)


def test_history_keeps_the_days_a_session_was_active():
    day1, day3 = (
        time.mktime((2026, 11, 1, 12, 0, 0, 0, 0, -1)),
        time.mktime((2026, 11, 3, 12, 0, 0, 0, 0, -1)),
    )
    sessions = files.read_history([entry("a", "/w/api", day1), entry("a", "/w/api", day3)])
    assert sessions[0].days == {datetime.date(2026, 11, 1), datetime.date(2026, 11, 3)}
