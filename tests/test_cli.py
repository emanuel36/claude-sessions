import io
import os
import subprocess
import sys
from pathlib import Path

from claude_sessions import cli
from claude_sessions.core import paths, state, store


def test_hook_bad_stdin_logs_and_returns_0(monkeypatch):
    monkeypatch.setattr("sys.stdin", io.TextIOWrapper(io.BytesIO(b"{")))
    assert cli.main(["hook"]) == 0
    log = Path(paths.state_dir(), paths.ERROR_LOG)
    assert "JSONDecodeError" in log.read_text()


def test_cli_does_not_import_textual_or_rich():
    code = "import claude_sessions.cli, sys; assert not {'textual', 'rich'} & set(sys.modules)"
    subprocess.run([sys.executable, "-c", code], check=True)


def test_seen_marks_done_session_idle():
    pid = os.getpid()
    store.write(
        {
            **state.new_record("s1"),
            "pid": pid,
            "pid_start": store.process_start(pid),
            "status": state.DONE,
            "mux": {"socket": "S", "pane": "%3"},
        }
    )
    assert cli.main(["seen", "--tmux-pane", "%3", "--tmux-socket", "S"]) == 0
    assert store.read("s1")["status"] == state.IDLE


def test_panel_runs_the_textual_app(monkeypatch):
    from claude_sessions.panel import app

    ran = []
    monkeypatch.setattr(app, "main", lambda: ran.append(True))
    assert cli.main(["panel"]) == 0 and ran == [True]
