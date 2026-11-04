import datetime
import io
import json
import os

from claude_sessions import cli
from claude_sessions.claude import statusline
from claude_sessions.core import journal, paths, state, store

# The status line input example from the Claude Code docs, trimmed to what usage reads
FULL = {
    "session_id": "abc123",
    "model": {"id": "claude-opus-4-1", "display_name": "Opus"},
    "cost": {
        "total_cost_usd": 0.01234,
        "total_duration_ms": 45000,
        "total_api_duration_ms": 2300,
        "total_lines_added": 156,
        "total_lines_removed": 23,
    },
    "context_window": {"used_percentage": 41.0},
    "rate_limits": {
        "five_hour": {"used_percentage": 23.5, "resets_at": 1738425600},
        "seven_day": {"used_percentage": 41.2, "resets_at": 1738857600},
    },
}


def test_parse_full_input():
    assert statusline.parse(FULL) == {
        "cost": 0.01234,
        "duration": 45.0,
        "model": "Opus",
        "added": 156,
        "removed": 23,
        "context": 41.0,
    }
    assert statusline.parse_limits(FULL) == {
        "five_hour": {"pct": 23.5, "resets_at": 1738425600},
        "seven_day": {"pct": 41.2, "resets_at": 1738857600},
    }


def test_parse_early_session_nulls():
    data = {"session_id": "abc123", "model": {"id": "claude-opus-4-1"}, "cost": None}
    assert statusline.parse(data) == {
        "cost": None,
        "duration": None,
        "model": "claude-opus-4-1",
        "added": 0,
        "removed": 0,
        "context": None,
    }
    assert statusline.parse_limits(data) == {}


def test_parse_ignores_values_of_the_wrong_type():
    data = {"model": "Opus", "cost": {"total_cost_usd": "1.2", "total_lines_added": True}}
    assert statusline.parse(data) == {
        "cost": None,
        "duration": None,
        "model": None,
        "added": 0,
        "removed": 0,
        "context": None,
    }
    assert statusline.parse_limits({"rate_limits": {"five_hour": None}}) == {}


def test_parse_limits_partial():
    data = {"rate_limits": {"seven_day": {"used_percentage": 12, "resets_at": 500}}}
    assert statusline.parse_limits(data) == {"seven_day": {"pct": 12, "resets_at": 500}}


def test_save_limits_replaces_only_the_windows_it_gets():
    statusline.save_limits({"five_hour": {"pct": 10, "resets_at": 200}}, now=100)
    statusline.save_limits({"seven_day": {"pct": 30, "resets_at": 900}}, now=110)
    statusline.save_limits({"five_hour": {"pct": 15, "resets_at": 200}}, now=120)
    assert statusline.load_limits(150) == {
        "five_hour": {"pct": 15, "resets_at": 200},
        "seven_day": {"pct": 30, "resets_at": 900},
    }
    assert os.path.isfile(os.path.join(paths.state_dir(), "limits.json"))


def test_load_limits_drops_expired():
    statusline.save_limits(
        {"five_hour": {"pct": 90, "resets_at": 200}, "seven_day": {"pct": 30, "resets_at": 900}},
        now=100,
    )
    assert statusline.load_limits(200) == {"seven_day": {"pct": 30, "resets_at": 900}}
    assert statusline.load_limits(900) == {}


def test_load_limits_with_a_broken_file_is_empty():
    assert statusline.load_limits(0) == {}
    with open(os.path.join(paths.state_dir(), "limits.json"), "w") as f:
        f.write("{")
    assert statusline.load_limits(0) == {}


def test_saved_statusline_path_is_in_the_state_home(tmp_path):
    assert statusline.saved_statusline_path() == str(tmp_path / "state" / "statusline.json")


def test_saved_command_reads_the_saved_statusline(tmp_path):
    path = tmp_path / "state" / "statusline.json"
    path.parent.mkdir(parents=True)
    path.write_text(json.dumps({"type": "command", "command": "echo hi", "padding": 0}))
    assert statusline.saved_command() == "echo hi"
    path.write_text("[")
    assert statusline.saved_command() is None


def test_save_limits_keeps_the_higher_use_of_the_same_window():
    statusline.save_limits({"five_hour": {"pct": 40, "resets_at": 200}}, now=100)
    # Another session's status line, not updated for a while
    statusline.save_limits({"five_hour": {"pct": 30, "resets_at": 200}}, now=110)
    assert statusline.load_limits(150) == {"five_hour": {"pct": 40, "resets_at": 200}}
    # A new window starts low
    statusline.save_limits({"five_hour": {"pct": 5, "resets_at": 500}}, now=210)
    assert statusline.load_limits(250) == {"five_hour": {"pct": 5, "resets_at": 500}}


def test_load_limits_drops_a_window_without_a_numeric_pct():
    windows = {
        "five_hour": {"pct": True, "resets_at": 900},
        "seven_day": {"pct": "40", "resets_at": 900},
    }
    with open(os.path.join(paths.state_dir(), "limits.json"), "w") as f:
        json.dump(windows, f)
    assert statusline.load_limits(0) == {}


def cs_statusline(monkeypatch, capsys, stdin):
    monkeypatch.setattr("sys.stdin", io.TextIOWrapper(io.BytesIO(stdin)))
    code = cli.main(["statusline"])
    return code, capsys.readouterr().out


def save_statusline(tmp_path, command):
    path = tmp_path / "state" / "statusline.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"type": "command", "command": command, "padding": 0}))


STATUS_INPUT = {
    "session_id": "s1",
    "model": {"id": "claude-opus-4-1", "display_name": "Opus"},
    "cost": {"total_cost_usd": 1.5, "total_duration_ms": 60000, "total_lines_added": 3},
    "rate_limits": {"five_hour": {"used_percentage": 40, "resets_at": 4_000_000_000}},
}


def test_statusline_unknown_session_still_chains(monkeypatch, capsys, tmp_path):
    save_statusline(tmp_path, "echo hi")
    code, out = cs_statusline(monkeypatch, capsys, json.dumps(STATUS_INPUT).encode())
    assert (code, out) == (0, "hi\n")
    assert store.read("s1") is None


def test_statusline_chained_command_gets_the_same_stdin(monkeypatch, capsys, tmp_path):
    save_statusline(tmp_path, "cat")
    stdin = json.dumps(STATUS_INPUT).encode()
    assert cs_statusline(monkeypatch, capsys, stdin) == (0, stdin.decode())


def test_statusline_non_json_stdin_prints_and_returns_0(monkeypatch, capsys, tmp_path):
    assert cs_statusline(monkeypatch, capsys, b"not json") == (0, "\n")
    save_statusline(tmp_path, "echo hi")
    assert cs_statusline(monkeypatch, capsys, b"not json") == (0, "hi\n")


def test_statusline_records_usage_without_touching_status(monkeypatch, capsys):
    record = {**state.new_record("s1"), "status": state.PERMISSION, "status_since": 7.0}
    store.write(record)
    code, out = cs_statusline(monkeypatch, capsys, json.dumps(STATUS_INPUT).encode())
    assert (code, out) == (0, "Opus  $1.50\n")
    assert store.read("s1") == {
        **record,
        "usage": {
            "cost": 1.5,
            "duration": 60.0,
            "model": "Opus",
            "added": 3,
            "removed": 0,
            "context": None,
        },
    }
    assert statusline.load_limits(0) == {"five_hour": {"pct": 40, "resets_at": 4_000_000_000}}


def test_statusline_default_line_is_empty_until_cost_is_known(monkeypatch, capsys):
    early = {**STATUS_INPUT, "cost": None}
    assert cs_statusline(monkeypatch, capsys, json.dumps(early).encode()) == (0, "\n")


def test_statusline_logs_the_cost_only_when_it_changes(monkeypatch, capsys):
    store.write({**state.new_record("s1"), "status_since": 7.0})
    stdin = json.dumps(STATUS_INPUT).encode()
    cs_statusline(monkeypatch, capsys, stdin)
    cs_statusline(monkeypatch, capsys, stdin)
    events = journal.read_day(datetime.date.today())
    assert [(e["id"], e["cost"]) for e in events if "cost" in e] == [("s1", 1.5)]


def test_cost_events_carry_lines(monkeypatch, capsys):
    store.write({**state.new_record("s1"), "status_since": 7.0})
    data = {**STATUS_INPUT, "cost": {"total_cost_usd": 1.0, "total_lines_added": 10}}
    data["cost"]["total_lines_removed"] = 2
    cs_statusline(monkeypatch, capsys, json.dumps(data).encode())
    events = [e for e in journal.read_day(datetime.date.today()) if "cost" in e]
    assert [(e["id"], e["cost"], e["added"], e["removed"]) for e in events] == [("s1", 1.0, 10, 2)]


def test_statusline_without_a_valid_session_still_saves_the_limits(monkeypatch, capsys):
    for session_id in (None, "../s1"):
        data = {**STATUS_INPUT, "session_id": session_id}
        assert cs_statusline(monkeypatch, capsys, json.dumps(data).encode())[0] == 0
    assert statusline.load_limits(0) == {"five_hour": {"pct": 40, "resets_at": 4_000_000_000}}
    assert not os.path.exists(os.path.join(paths.state_dir(), paths.ERROR_LOG))


def test_with_mode_right_aligns_the_mode_on_the_visible_width():
    out = statusline.with_mode("\x1b[32mOpus\x1b[0m  $1.50", "plan", 30)
    assert out == "\x1b[32mOpus\x1b[0m  $1.50" + " " * 15 + "plan"
    assert out.replace("\x1b[32m", "").replace("\x1b[0m", "").__len__() == 30


def test_with_mode_leaves_the_line_without_a_mode_or_room():
    assert statusline.with_mode("Opus", "", 30) == "Opus"
    assert statusline.with_mode("Opus", "accept edits on", 12) == "Opus"


def test_statusline_shows_the_session_mode_on_the_right(monkeypatch, capsys):
    store.write({**state.new_record("s1"), "mode": "acceptEdits"})
    monkeypatch.setattr(statusline, "pane_width", lambda: 50)
    _, out = cs_statusline(monkeypatch, capsys, json.dumps(STATUS_INPUT).encode())
    line = out.rstrip("\n")
    assert line.startswith("Opus  $1.50") and line.endswith("⏵⏵ accept edits on")
    assert len(line) == 48


def test_statusline_shows_no_mode_when_the_mode_is_default_or_the_width_unknown(
    monkeypatch, capsys
):
    store.write({**state.new_record("s1"), "mode": "default"})
    monkeypatch.setattr(statusline, "pane_width", lambda: 50)
    assert cs_statusline(monkeypatch, capsys, json.dumps(STATUS_INPUT).encode()) == (
        0,
        "Opus  $1.50\n",
    )
    store.write({**state.new_record("s1"), "mode": "plan"})
    monkeypatch.setattr(statusline, "pane_width", lambda: None)
    assert cs_statusline(monkeypatch, capsys, json.dumps(STATUS_INPUT).encode())[1] == (
        "Opus  $1.50\n"
    )


def test_statusline_puts_the_mode_on_the_last_line_of_a_chained_status_line(
    monkeypatch, capsys, tmp_path
):
    save_statusline(tmp_path, "printf 'first\\nsecond\\n'")
    store.write({**state.new_record("s1"), "mode": "plan"})
    monkeypatch.setattr(statusline, "pane_width", lambda: 30)
    _, out = cs_statusline(monkeypatch, capsys, json.dumps(STATUS_INPUT).encode())
    first, last = out.rstrip("\n").split("\n")
    assert first == "first" and last.startswith("second") and last.endswith("⏸ plan mode on")
