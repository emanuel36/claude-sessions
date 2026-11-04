import fcntl
import importlib.resources
import os
import pty
import select
import shutil
import signal
import struct
import sys
import termios
import time

import pytest
from conftest import throwaway_tmux

from claude_sessions.term.tmux import SIDEBAR_WIDTH

pytestmark = pytest.mark.skipif(shutil.which("tmux") is None, reason="tmux is not installed")

CONF = importlib.resources.files("claude_sessions.term") / "data" / "tmux.conf"


@pytest.fixture
def run():
    """Runs tmux commands on a throwaway server, killed afterwards."""
    tmux, kill = throwaway_tmux(CONF)
    yield tmux
    kill()


def test_windows_are_numbered_from_one(run):
    assert run("list-windows", "-F", "#{window_index}").split() == ["1", "2"]


def test_scheme_a_keys(run):
    keys = " ".join(run("list-keys", "-T", "root").split())
    assert "M-Up previous-window" in keys and "M-Down next-window" in keys
    assert "M-n display-popup" in keys and "cs new --pick" in keys
    assert "M-c confirm-before" in keys and "close #{window_name}? (y/n)" in keys
    assert "cs close #{window_id}" in keys
    for gone in ("M-Left", "M-Right", "M-0", "M-,", "M-.", "M-g", "cs next"):
        assert gone not in keys


def test_status_bar_is_off(run):
    assert run("show", "-gv", "status").strip() == "off"


def test_conf_hooks_and_mouse(run):
    hooks = run("show-hooks", "-g")
    assert "after-new-window" in hooks and "cs layout" in hooks
    assert "after-split-window" in hooks
    # A window hook, so it is listed among the window options
    assert "cs layout" in run("show-hooks", "-gw", "window-resized")
    assert "@cs-sidebar" in run("list-keys", "-T", "root", "MouseDown1Pane")


def test_new_window_hook_passes_its_window_id_to_cs_layout(tmp_path, monkeypatch):
    log = tmp_path / "log"
    fake = tmp_path / "cs"
    fake.write_text(f'#!/bin/sh\necho "$@" >> {log}\n')
    fake.chmod(0o755)
    monkeypatch.setenv("PATH", f"{tmp_path}:{os.environ['PATH']}")
    tmux, kill = throwaway_tmux(CONF)
    try:
        window = tmux("new-window", "-P", "-F", "#{window_id}").strip()
        deadline = time.time() + 3
        while window not in (log.read_text() if log.exists() else "") and time.time() < deadline:
            time.sleep(0.05)
        assert f"layout {window}" in log.read_text().splitlines()
    finally:
        kill()


def _resize(fd, pid, cols, rows):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    os.kill(pid, signal.SIGWINCH)


def _drain(fd, seconds):
    end = time.time() + seconds
    while time.time() < end:
        if select.select([fd], [], [], 0.05)[0]:
            try:
                os.read(fd, 65536)
            except OSError:
                return


def test_sidebars_keep_their_width_in_windows_resized_while_hidden(tmp_path, monkeypatch):
    # tmux resizes a window only once it becomes the current one, so a window that was hidden
    # during a terminal resize must be fitted when it is resized, not only on client-resized
    fake = tmp_path / "cs"
    fake.write_text(
        '#!/bin/sh\n[ "$1" = sidebar ] && exec sleep 1000\n'
        f'exec {sys.executable} -c "import sys; from claude_sessions.cli import main; '
        'sys.exit(main(sys.argv[1:]))" "$@"\n'
    )
    fake.chmod(0o755)
    monkeypatch.setenv("PATH", f"{tmp_path}:{os.environ['PATH']}")
    tmux, kill = throwaway_tmux(CONF)
    pid, fd = pty.fork()
    try:
        socket = tmux("display", "-p", "#{socket_path}").strip()
        if pid == 0:
            env = {k: v for k, v in os.environ.items() if k != "TMUX"}
            env["TERM"] = "xterm-256color"
            os.execvpe("tmux", ["tmux", "-S", socket, "attach", "-t", "t"], env)
        _resize(fd, pid, 200, 50)
        _drain(fd, 0.5)
        for _ in range(2):
            tmux("new-window", "sleep 1000")
            _drain(fd, 1.0)

        def widths():
            out = tmux("list-panes", "-a", "-F", "#{window_index}:#{@cs-sidebar}:#{pane_width}")
            return [int(w) for _, flag, w in (r.split(":") for r in out.split()) if flag == "1"]

        assert widths() and set(widths()) == {SIDEBAR_WIDTH}
        _resize(fd, pid, 120, 40)
        _drain(fd, 1.5)
        for window in (1, 2, 3, 4):
            tmux("select-window", "-t", f":{window}")
            _drain(fd, 0.8)
        assert widths() and set(widths()) == {SIDEBAR_WIDTH}
        _resize(fd, pid, 220, 50)
        _drain(fd, 1.5)
        for window in (1, 2, 3, 4):
            tmux("select-window", "-t", f":{window}")
            _drain(fd, 0.8)
        assert widths() and set(widths()) == {SIDEBAR_WIDTH}
    finally:
        kill()
        try:
            os.kill(pid, signal.SIGTERM)
            os.waitpid(pid, 0)
        except (ProcessLookupError, ChildProcessError):
            pass
        os.close(fd)
