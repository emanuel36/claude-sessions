import json
import os

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


def test_the_last_title_that_parses_wins():
    tail = b'{"aiTitle":"Good"}\n{"aiTitle":"bad \\q"}\n{"customTitle":""}\n'
    assert files.find_title(tail) == "Good"


def test_stored_titles_spare_a_new_process_the_transcripts(tmp_path, monkeypatch):
    path = tmp_path / "t.jsonl"
    path.write_bytes(b'{"aiTitle":"First"}\n')
    titles = files.Titles(stored=True)
    assert titles.get("a", str(path)) == "First"
    titles.save()
    read = []
    monkeypatch.setattr(files, "title_of", lambda p: read.append(p) or "")
    again = files.Titles(stored=True)
    assert again.get("a", str(path)) == "First" and read == []
    # A transcript that changed is read again
    os.utime(path, ns=(1, 2))
    assert again.get("a", str(path)) == "" and read == [str(path)]


def test_stored_titles_keep_only_the_sessions_asked_for(tmp_path):
    titles = files.Titles(stored=True)
    titles.get("a", None)
    titles.save()
    titles.get("b", None)
    titles.save()
    with open(files.titles_path()) as f:
        assert json.load(f) == {"b": [None, ""]}


def test_history_is_read_on_from_where_the_last_process_stopped(monkeypatch):
    write_history(entry("a", "/w/api", 10, "first"))
    assert [s.id for s in files.load_history(stored=True)] == ["a"]
    # A new process: nothing in memory, the reading on disk
    monkeypatch.setattr(files, "_cache", {"key": None, "sessions": [], "reading": None})
    with open(files.history_path(), "a") as f:
        f.write(entry("b", "/w/web", 20) + "\n" + '{"sessionId": "c", "proj')
    parsed = []
    real = files._add_history
    monkeypatch.setattr(
        files, "_add_history", lambda s, lines: parsed.extend(lines) or real(s, lines)
    )
    sessions = files.load_history()
    assert [(s.id, s.first_prompt) for s in sessions] == [("b", "hi"), ("a", "first")]
    # Only the new whole line was parsed; the one still being written waits
    assert parsed == [entry("b", "/w/web", 20)]


def test_history_replaced_by_a_shorter_file_is_read_again():
    write_history(entry("a", "/w/api", 10), entry("b", "/w/api", 20))
    assert len(files.load_history()) == 2
    write_history(entry("c", "/w/api", 30))
    assert [s.id for s in files.load_history()] == ["c"]


def test_a_long_first_prompt_is_cut():
    sessions = files.read_history([entry("a", "/w/api", 10, "x" * 1000)])
    assert len(sessions[0].first_prompt) == files.PROMPT_CHARS


def test_history_rewritten_in_place_is_read_again():
    write_history(entry("a", "/w/api", 10))
    assert [s.id for s in files.load_history()] == ["a"]
    # Same file, longer, but what was read is gone
    with open(files.history_path(), "r+") as f:
        f.write(entry("b", "/w/api", 20) + "\n" + entry("c", "/w/api", 30) + "\n")
    assert [s.id for s in files.load_history()] == ["c", "b"]


def test_a_prompt_with_a_line_separator_stays_one_entry():
    # Written raw, as Claude Code writes it: JSON leaves U+2028 unescaped
    raw = {"sessionId": "a", "project": "/w/api", "timestamp": 1, "display": "one\u2028two"}
    write_history(json.dumps(raw, ensure_ascii=False))
    assert [s.first_prompt for s in files.load_history()] == ["one\u2028two"]


def test_history_is_kept_on_disk_only_when_asked():
    write_history(entry("a", "/w/api", 10))
    files.load_history()
    assert not os.path.exists(files.stored_history_path())
    write_history(entry("a", "/w/api", 10), entry("b", "/w/api", 20))
    files.load_history(stored=True)
    assert os.path.exists(files.stored_history_path())
