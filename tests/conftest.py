"""Shared fixtures."""

import contextlib
import os
import subprocess
import uuid

import pytest


@pytest.fixture(autouse=True)
def _isolated_env(tmp_path, monkeypatch):
    """Point every state directory at tmp_path so tests never touch real data."""
    monkeypatch.setenv("CLAUDE_SESSIONS_DIR", str(tmp_path / "sessions"))
    monkeypatch.setenv("CLAUDE_SESSIONS_STATE", str(tmp_path / "state"))
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "claude"))


def throwaway_tmux(conf):
    """Starts a tmux server of the tests' own, never the user's, with two windows.

    Args:
        conf: The configuration file the server starts with.

    Returns:
        A function that runs a tmux command on the server and returns its output, and one that
        kills the server.
    """
    # Unique per server: a pid alone repeats across the servers of one run, and across runs
    name = f"cs-test-{uuid.uuid4().hex}"
    env = {k: v for k, v in os.environ.items() if k != "TMUX"}

    def tmux(*args):
        out = subprocess.run(
            ["tmux", "-L", name, "-f", str(conf), *args],
            env=env,
            capture_output=True,
            text=True,
            timeout=5,
            check=True,
        )
        return out.stdout

    def kill():
        kill_tmux(name, env)

    tmux("new-session", "-d", "-s", "t", "-x", "120", "-y", "20")
    tmux("new-window", "-t", "t:")
    return tmux, kill


def kill_tmux(name, env=None):
    """Kills a test tmux server and deletes the socket file tmux leaves behind."""
    subprocess.run(["tmux", "-L", name, "kill-server"], env=env, capture_output=True)
    # Where tmux puts a -L socket; derived, so it goes even when the server already died
    tmpdir = (env or os.environ).get("TMUX_TMPDIR") or "/tmp"
    with contextlib.suppress(FileNotFoundError):
        os.unlink(os.path.join(tmpdir, f"tmux-{os.getuid()}", name))
