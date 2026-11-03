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


def test_panel_runs_the_textual_app(monkeypatch):
    from claude_sessions.panel import app

    ran = []
    monkeypatch.setattr(app, "main", lambda: ran.append(True))
    assert cli.main(["panel"]) == 0 and ran == [True]


def test_next_selects_the_pending_window_or_says_nothing_waits(monkeypatch):
    shown, selected = [], []
    monkeypatch.setattr(cli.hook, "live_sessions", lambda now: [])
    monkeypatch.setattr(cli.tmux, "windows_of", lambda records: windows)
    monkeypatch.setattr(cli.tmux, "run", lambda *args: shown.append(args) or "@1")
    monkeypatch.setattr(cli.tmux, "select", selected.append)
    windows = [cli.tmux.Window("@2", {"status": state.INPUT, "mux": {"pane": "%2"}})]
    assert cli.main(["next"]) == 0
    assert [w.window_id for w in selected] == ["@2"]
    windows.clear()
    assert cli.main(["next"]) == 0
    assert shown[-1] == ("display-message", "Nothing is waiting for you")


def test_new_projects_prints_tilde_paths(monkeypatch, capsys, tmp_path):
    home = tmp_path / "home"
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setattr(
        cli.files, "recent_projects", lambda: [str(home / "work" / "api"), "/srv/other"]
    )
    assert cli.main(["new", "--projects"]) == 0
    assert capsys.readouterr().out.splitlines() == ["~/work/api", "/srv/other"]


def test_new_needs_pick_or_projects(monkeypatch):
    calls = []
    monkeypatch.setattr(cli.tmux, "run", lambda *args: calls.append(args) or "")
    with pytest.raises(SystemExit) as exit_info:
        cli.main(["new"])
    assert exit_info.value.code == 2 and calls == []


def test_new_pick_runs_the_packaged_script(monkeypatch):
    execs = []
    monkeypatch.setattr(cli.os, "execvp", lambda *args: execs.append(args))
    cli.main(["new", "--pick"])
    ((file, argv),) = execs
    assert file == "bash" and argv[0] == "bash" and argv[1].endswith("data/new_session.sh")
    assert os.path.isfile(argv[1])


def test_tig_command_toggles_tig_for_the_window_and_folder(monkeypatch):
    calls = []
    monkeypatch.setattr(cli.tmux, "toggle_tig", lambda *args: calls.append(args))
    assert cli.main(["tig", "@3", "/w/api"]) == 0
    assert calls == [("@3", "/w/api")]


def test_new_projects_lists_open_folders_first_without_repeats(monkeypatch, capsys, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(cli.hook, "live_sessions", lambda now: [{"cwd": "/w/b"}, {"cwd": "/w/a"}])
    monkeypatch.setattr(cli.files, "recent_projects", lambda: ["/w/a", "/w/c"])
    assert cli.main(["new", "--projects"]) == 0
    assert capsys.readouterr().out.splitlines() == ["/w/b", "/w/a", "/w/c"]


def test_new_preview_of_a_repo_shows_branch_changes_and_commits(capsys, tmp_path):
    def git(*args):
        subprocess.run(
            ["git", "-C", str(tmp_path), "-c", "user.name=t", "-c", "user.email=t@t", *args],
            check=True,
            capture_output=True,
        )

    git("init", "-q", "-b", "trunk")
    git("commit", "-q", "--allow-empty", "-m", "feat: first")
    (tmp_path / "new.txt").write_text("x")
    assert cli.main(["new", "--preview", str(tmp_path)]) == 0
    out = capsys.readouterr().out
    assert "trunk" in out and "1 changed" in out and "feat: first" in out


def test_new_preview_of_a_plain_folder_says_so(capsys, tmp_path):
    assert cli.main(["new", "--preview", str(tmp_path)]) == 0
    assert "not a git repository" in capsys.readouterr().out
