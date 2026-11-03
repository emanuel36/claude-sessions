import os
import stat
from pathlib import Path

import pytest

from claude_sessions.core import paths


@pytest.fixture
def no_runtime_dir(tmp_path, monkeypatch):
    """No XDG_RUNTIME_DIR: the records go to the temporary folder, here tmp_path."""
    monkeypatch.delenv("CLAUDE_SESSIONS_DIR")
    monkeypatch.delenv("XDG_RUNTIME_DIR", raising=False)
    monkeypatch.setattr(paths.tempfile, "tempdir", str(tmp_path))
    return tmp_path / f"claude-sessions-{os.getuid()}"


def test_without_a_runtime_dir_the_records_go_to_a_folder_of_this_user(no_runtime_dir):
    assert paths.state_dir() == str(no_runtime_dir)
    assert stat.S_IMODE(os.stat(no_runtime_dir).st_mode) == 0o700


def test_a_temporary_folder_others_can_write_is_refused(no_runtime_dir):
    no_runtime_dir.mkdir()
    no_runtime_dir.chmod(0o777)
    with pytest.raises(OSError):
        paths.state_dir()


def test_a_temporary_folder_of_another_user_is_refused(tmp_path, no_runtime_dir, monkeypatch):
    # Another user made this user's folder first; here the other user is the one running tests
    victim = os.getuid() + 1
    (tmp_path / f"claude-sessions-{victim}").mkdir(mode=0o700)
    monkeypatch.setattr(paths.os, "getuid", lambda: victim)
    with pytest.raises(OSError):
        paths.state_dir()


def test_a_folder_the_user_chose_is_taken_as_it_is(tmp_path, monkeypatch):
    # Made with a umask that lets the group write, as many desktops have
    chosen = tmp_path / "chosen"
    chosen.mkdir()
    chosen.chmod(0o775)
    monkeypatch.setenv("CLAUDE_SESSIONS_DIR", str(chosen))
    assert paths.state_dir() == str(chosen)


def test_log_error_never_raises(monkeypatch, tmp_path):
    # A file where the state directory should be
    (tmp_path / "blocked").write_text("")
    monkeypatch.setenv("CLAUDE_SESSIONS_DIR", str(tmp_path / "blocked"))
    try:
        raise ValueError("boom")
    except ValueError:
        paths.log_error()


def test_log_error_does_not_follow_a_symlink(tmp_path):
    target = tmp_path / "target"
    target.write_text("mine")
    os.symlink(target, Path(paths.state_dir(), paths.ERROR_LOG))
    try:
        raise ValueError("boom")
    except ValueError:
        paths.log_error()
    assert target.read_text() == "mine"


def test_a_symlink_in_place_of_the_temporary_folder_is_refused(tmp_path, no_runtime_dir):
    # Planted by someone else, pointing at a folder that passes the other checks
    target = tmp_path / "elsewhere"
    target.mkdir(mode=0o700)
    no_runtime_dir.symlink_to(target)
    with pytest.raises(OSError):
        paths.state_dir()
