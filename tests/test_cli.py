import io
import os
import subprocess
import sys
from pathlib import Path

import pytest

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


def test_landing_command_runs_the_app(monkeypatch):
    from claude_sessions.ui import landing_app

    ran = []
    monkeypatch.setattr(landing_app, "main", lambda: ran.append(True))
    assert cli.main(["landing"]) == 0 and ran == [True]


def test_sidebar_command_runs_the_app(monkeypatch):
    from claude_sessions.ui import sidebar_app

    ran = []
    monkeypatch.setattr(sidebar_app, "main", lambda: ran.append(True))
    assert cli.main(["sidebar"]) == 0 and ran == [True]


def test_close_command_closes_the_window(monkeypatch):
    calls = []
    monkeypatch.setattr(cli.tmux, "close_window", calls.append)
    assert cli.main(["close", "@3"]) == 0
    assert calls == ["@3"]


def test_layout_adds_a_sidebar_to_a_window_or_fits_them_all(monkeypatch):
    calls = []
    monkeypatch.setattr(cli.tmux, "add_sidebar", lambda window: calls.append(("add", window)))
    monkeypatch.setattr(cli.tmux, "fit_sidebars", lambda: calls.append(("fit",)))
    assert cli.main(["layout", "@4"]) == 0
    assert cli.main(["layout"]) == 0
    assert calls == [("add", "@4"), ("fit",), ("fit",)]


def test_new_pick_runs_the_panel(monkeypatch):
    from claude_sessions.ui import new_app

    ran = []
    monkeypatch.setattr(new_app, "main", lambda: ran.append(True))
    assert cli.main(["new", "--pick"]) == 0 and ran == [True]


def test_new_without_pick_is_a_usage_error():
    with pytest.raises(SystemExit) as exit_info:
        cli.main(["new"])
    assert exit_info.value.code == 2
