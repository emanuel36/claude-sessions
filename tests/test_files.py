import json

from claude_sessions.claude import files


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
