import os
import shutil
import subprocess
import uuid

import pytest
from conftest import kill_tmux

from claude_sessions.term import launcher


@pytest.mark.skipif(shutil.which("tmux") is None, reason="tmux is not installed")
def test_creates_the_session_with_the_panel_in_tab_one_then_attaches(monkeypatch):
    socket = f"cs-test-launcher-{uuid.uuid4().hex}"
    attached = []
    monkeypatch.setattr(launcher, "SOCKET", socket)
    monkeypatch.setattr(launcher.os, "execvp", lambda file, args: attached.append(args))
    monkeypatch.delenv("TMUX", raising=False)
    try:
        launcher.main([])
        out = subprocess.run(
            [
                "tmux",
                "-L",
                socket,
                "list-panes",
                "-a",
                "-F",
                "#{window_index} #{window_name} #{pane_start_command}",
            ],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        assert out.split("\n")[0] == f'1 sessions "{launcher.PANEL}"'
        assert attached[0][-3:] == ["attach-session", "-t", "claude"]
        assert attached[0][2] == socket
    finally:
        kill_tmux(socket)


def test_ghostty_command_unbinds_the_alt_keys():
    cmd = launcher.ghostty_command("/x/cst")
    assert "--keybind=alt+1=unbind" in cmd and "--keybind=alt+arrow_right=unbind" in cmd
    assert cmd[-2:] == ["-e", "/x/cst"]


def test_missing_tmux_says_so_and_returns_1(monkeypatch, capsys):
    def run(*args, **kwargs):
        raise FileNotFoundError("tmux")

    monkeypatch.setattr(launcher.subprocess, "run", run)
    # main puts its own folder first on PATH; this restores it afterwards
    monkeypatch.setenv("PATH", os.environ["PATH"])
    assert launcher.main([]) == 1
    assert capsys.readouterr().err == "cst: tmux is not installed\n"


def test_failed_new_session_says_so_and_returns_1(monkeypatch, capsys):
    def run(cmd, **kwargs):
        if "new-session" in cmd:
            raise subprocess.CalledProcessError(1, cmd)
        return subprocess.CompletedProcess(cmd, 1)

    monkeypatch.setattr(launcher.subprocess, "run", run)
    # main puts its own folder first on PATH; this restores it afterwards
    monkeypatch.setenv("PATH", os.environ["PATH"])
    assert launcher.main([]) == 1
    assert capsys.readouterr().err == "cst: tmux could not create the claude session\n"
