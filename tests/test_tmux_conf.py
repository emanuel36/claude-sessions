import importlib.resources
import re
import shutil

import pytest
from conftest import throwaway_tmux

pytestmark = pytest.mark.skipif(shutil.which("tmux") is None, reason="tmux is not installed")

CONF = importlib.resources.files("claude_sessions.term") / "data" / "tmux.conf"
MARKS = ("! ", "? ", "✓ ", "●", "○")


@pytest.fixture
def run():
    """Runs tmux commands on a throwaway server, killed afterwards."""
    tmux, kill = throwaway_tmux(CONF)
    yield tmux
    kill()


def tab(run, index):
    """The window's tab text with tmux styles stripped."""
    out = run("display", "-p", "-t", f":{index}", "#{T:window-status-format}")
    return re.sub(r"#\[[^\]]*\]", "", out)


def test_windows_are_numbered_from_one(run):
    assert run("list-windows", "-F", "#{window_index}").split() == ["1", "2"]


@pytest.mark.parametrize(
    "status, present",
    [
        ("permission", "! "),
        ("input", "? "),
        ("done", "✓ "),
        ("working", None),
        ("idle", ""),
    ],
)
def test_tab_shows_the_status_mark(run, status, present):
    run("set-option", "-p", "-t", ":2", "@claude_status", status)
    text = tab(run, 2)
    if status == "working":
        assert "●" in text or "○" in text
    elif status == "idle":
        assert not any(m in text for m in MARKS)
    else:
        assert present in text


def test_tab_one_is_the_session_list_mark(run):
    assert "≡" in tab(run, 1)


def test_alt_g_bound(run):
    assert "tig" in run("list-keys", "-T", "root", "M-g")
