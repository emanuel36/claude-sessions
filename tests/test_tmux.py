from claude_sessions.core import state
from claude_sessions.term import tmux

TMUX = {"TMUX": "/tmp/ttmux-1000/claude,123,0", "TMUX_PANE": "%3"}


def test_detect():
    assert tmux.detect({}) == {}
    assert tmux.detect(TMUX) == {"socket": "/tmp/ttmux-1000/claude", "pane": "%3"}


def test_window_name_is_the_project_folder():
    assert tmux.window_name({"cwd": "/w/vehicle.k265"}) == "vehicle.k265"
    assert tmux.window_name({"cwd": "/w/notes/"}) == "notes"
    long = tmux.window_name({"cwd": "/w/" + "x" * 40})
    assert len(long) == tmux.WINDOW_NAME_LEN and long.endswith("…")


def test_run_returns_empty_when_tmux_fails(monkeypatch):
    def fail(*args, **kwargs):
        raise OSError

    monkeypatch.setattr(tmux.subprocess, "run", fail)
    assert tmux.run("list-windows") == ""


def test_notify_with_a_dead_server_does_not_raise():
    tmux.notify({"status": state.DONE, "mux": {"socket": "/nonexistent", "pane": "%1"}})


def test_name_window_with_a_dead_server_does_not_raise():
    tmux.name_window({"cwd": "/w/api", "mux": {"socket": "/nonexistent", "pane": "%1"}})


def test_settle_if_viewed_with_a_dead_server_keeps_the_record():
    done = {"status": state.DONE, "mux": {"socket": "/nonexistent", "pane": "%1"}}
    assert tmux.settle_if_viewed(done, 5.0) == done
