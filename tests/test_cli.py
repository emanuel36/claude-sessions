import io
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

from claude_sessions import cli
from claude_sessions.claude import files
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


def test_close_command_closes_the_window(monkeypatch):
    calls = []
    monkeypatch.setattr(cli.tmux, "close_window", calls.append)
    assert cli.main(["close", "@3"]) == 0
    assert calls == ["@3"]


def test_new_projects_lists_open_folders_first_without_repeats(monkeypatch, capsys, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(cli.hook, "live_sessions", lambda now: [{"cwd": "/w/b"}, {"cwd": "/w/a"}])
    monkeypatch.setattr(cli.files, "recent_projects", lambda: ["/w/a", "/w/c"])
    assert cli.main(["new", "--projects"]) == 0
    assert capsys.readouterr().out.splitlines() == ["/w/b", "/w/a", "/w/c"]


def test_new_projects_leaves_out_the_folders_of_background_runs(monkeypatch, capsys):
    runs = [{"cwd": "/tmp/daily-recap", "headless": True}, {"cwd": "/w/a"}]
    monkeypatch.setattr(cli.hook, "live_sessions", lambda now: runs)
    monkeypatch.setattr(cli.files, "recent_projects", lambda: [])
    assert cli.main(["new", "--projects"]) == 0
    assert capsys.readouterr().out.splitlines() == ["/w/a"]


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


def write_history(*entries):
    os.makedirs(paths.config_dir(), exist_ok=True)
    with open(files.history_path(), "w") as f:
        for session_id, project, seconds_ago, display in entries:
            stamp = int((time.time() - seconds_ago) * 1000)
            line = {"sessionId": session_id, "project": project, "timestamp": stamp}
            f.write(json.dumps({**line, "display": display}) + "\n")


def test_new_sessions_lists_new_first_then_the_projects_sessions_newest_first(capsys):
    write_history(
        ("old", "/w/api", 3 * 86400, "fix timeout"),
        ("other", "/w/docs", 60, "write guide"),
        ("recent", "/w/api", 2 * 3600, "adjust routes"),
        ("cmd", "/w/api", 30, "/resume"),
    )
    assert cli.main(["new", "--sessions", "/w/api"]) == 0
    assert capsys.readouterr().out.splitlines() == [
        "+\t+ new session",
        "recent\t↺ adjust routes  2h ago",
        "old\t↺ fix timeout  3d ago",
    ]


def test_new_sessions_of_an_unknown_folder_is_empty(capsys):
    assert cli.main(["new", "--sessions", "/w/none"]) == 0
    assert capsys.readouterr().out == ""


def test_new_preview_shows_the_latest_sessions_of_the_folder(capsys, tmp_path):
    write_history(("a", str(tmp_path), 7200, "adjust routes"))
    assert cli.main(["new", "--preview", str(tmp_path)]) == 0
    assert "adjust routes" in capsys.readouterr().out


def test_layout_adds_a_sidebar_to_a_window_or_fits_them_all(monkeypatch):
    calls = []
    monkeypatch.setattr(cli.tmux, "add_sidebar", lambda window: calls.append(("add", window)))
    monkeypatch.setattr(cli.tmux, "fit_sidebars", lambda: calls.append(("fit",)))
    assert cli.main(["layout", "@4"]) == 0
    assert cli.main(["layout"]) == 0
    assert calls == [("add", "@4"), ("fit",), ("fit",)]
