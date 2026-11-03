import os
import pty
import shutil
import signal
import subprocess
import time

import pytest
from conftest import throwaway_tmux

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


def test_windows_of_keeps_this_server_and_the_most_urgent_per_window(monkeypatch):
    monkeypatch.setenv("TMUX", "/s/claude,1,0")
    panes = "%1\t@1\n%2\t@1\n%3\t@2\n%4\t@3"
    monkeypatch.setattr(tmux, "run", lambda *args: panes)

    def record(session_id, status, pane, socket="/s/claude"):
        return {"id": session_id, "status": status, "mux": {"socket": socket, "pane": pane}}

    records = [
        record("a", state.PERMISSION, "%2"),
        record("b", state.WORKING, "%1"),
        record("c", state.IDLE, "%3"),
        record("d", state.PERMISSION, "%4", socket="/s/other"),
    ]
    windows = tmux.windows_of(records)
    assert [(w.window_id, w.lead["id"]) for w in windows] == [("@1", "a"), ("@2", "c")]


def _window(window_id, status):
    return tmux.Window(window_id, {"status": status})


WINDOWS = [
    _window("@2", state.PERMISSION),
    _window("@4", state.INPUT),
    _window("@5", state.DONE),
    _window("@6", state.WORKING),
]


def test_next_pending_starts_with_the_first_that_needs_you():
    assert tmux.next_pending(WINDOWS).window_id == "@2"
    assert tmux.next_pending(WINDOWS, "@6").window_id == "@2"


def test_next_pending_walks_through_all_pending_and_wraps():
    seen = []
    current = None
    for _ in range(4):
        current = tmux.next_pending(WINDOWS, current).window_id
        seen.append(current)
    assert seen == ["@2", "@4", "@5", "@2"]


def test_next_pending_without_pending():
    assert tmux.next_pending([_window("@6", state.WORKING)]) is None


@pytest.fixture
def server():
    """A throwaway tmux server with no configuration (no hooks), killed afterwards.

    Its windows count from 0, and the second one, window 1, is the active one.
    """
    if shutil.which("tmux") is None:
        pytest.skip("tmux is not installed")
    run, kill = throwaway_tmux(os.devnull)
    yield run
    kill()


def pane_of(server, window):
    """Where a session in the active pane of a window runs, as a record's mux says it."""
    out = server("display", "-p", "-t", f":{window}", "#{socket_path}\t#{pane_id}")
    socket, pane = out.strip().split("\t")
    return {"socket": socket, "pane": pane}


def pane_option(server, mux, option):
    return server("show-options", "-pqv", "-t", mux["pane"], option).strip()


@pytest.fixture
def attached(server):
    """Attaches a client to the server through a pseudo-terminal, as a user looking at it."""
    socket = server("display", "-p", "#{socket_path}").strip()
    pid, fd = pty.fork()
    if pid == 0:
        try:
            env = {k: v for k, v in os.environ.items() if k != "TMUX"}
            env["TERM"] = "xterm-256color"
            os.execvpe("tmux", ["tmux", "-S", socket, "attach", "-t", "t"], env)
        finally:
            os._exit(127)
    deadline = time.time() + 5
    while server("display", "-p", "-t", "t", "#{session_attached}").strip() != "1":
        if time.time() > deadline:
            pytest.fail("the client never attached")
        time.sleep(0.05)
    yield server
    os.kill(pid, signal.SIGTERM)
    os.waitpid(pid, 0)
    os.close(fd)


def test_notify_sets_then_unsets_the_status_and_since(server):
    mux = pane_of(server, 1)
    record = {"status": state.PERMISSION, "status_since": 123.9, "mux": mux}
    tmux.notify(record)
    assert pane_option(server, mux, "@claude_status") == state.PERMISSION
    assert pane_option(server, mux, "@cs-since") == "123"
    tmux.notify({**record, "status": state.WORKING})
    assert pane_option(server, mux, "@claude_status") == state.WORKING
    assert pane_option(server, mux, "@cs-since") == ""
    # A shell wait counts its time too
    tmux.notify({**record, "status": state.SHELL})
    assert pane_option(server, mux, "@cs-since") == "123"
    tmux.notify(record, ended=True)
    assert pane_option(server, mux, "@claude_status") == ""
    assert pane_option(server, mux, "@cs-since") == ""


def test_notify_with_a_dead_server_does_not_raise():
    tmux.notify({"status": state.DONE, "mux": {"socket": "/nonexistent", "pane": "%1"}})


def window_name_of(server, window):
    return server("display", "-p", "-t", f":{window}", "#{window_name}").strip()


def test_name_window_renames_an_automatic_name_and_its_own(server):
    mux = pane_of(server, 1)
    tmux.name_window({"cwd": "/w/api", "mux": mux})
    assert window_name_of(server, 1) == "api"
    # Renaming turned automatic-rename off; the name it set is still its own to change
    tmux.name_window({"cwd": "/w/notes", "mux": mux})
    assert window_name_of(server, 1) == "notes"


def test_name_window_leaves_a_name_the_user_gave_alone(server):
    server("rename-window", "-t", ":1", "mine")
    tmux.name_window({"cwd": "/w/api", "mux": pane_of(server, 1)})
    assert window_name_of(server, 1) == "mine"


def test_name_window_with_a_dead_server_does_not_raise():
    tmux.name_window({"cwd": "/w/api", "mux": {"socket": "/nonexistent", "pane": "%1"}})


def test_a_pane_is_not_in_view_without_a_client(server):
    assert not tmux.tmux_pane_in_view(pane_of(server, 1))


def test_only_the_active_pane_of_an_attached_session_is_in_view(attached):
    assert tmux.tmux_pane_in_view(pane_of(attached, 1))
    assert not tmux.tmux_pane_in_view(pane_of(attached, 0))
    attached("split-window", "-t", ":1")
    hidden = attached("display", "-p", "-t", ":1.{top}", "#{pane_id}").strip()
    assert not tmux.tmux_pane_in_view({**pane_of(attached, 1), "pane": hidden})


def test_settle_if_viewed_marks_a_finished_session_in_view_seen(attached):
    done = {"status": state.DONE, "status_since": 1.0, "mux": pane_of(attached, 1)}
    assert tmux.settle_if_viewed(done, 5.0)["status"] == state.IDLE
    behind = {**done, "mux": pane_of(attached, 0)}
    assert tmux.settle_if_viewed(behind, 5.0) == behind
    working = {**done, "status": state.WORKING}
    assert tmux.settle_if_viewed(working, 5.0) == working


def test_settle_if_viewed_with_a_dead_server_keeps_the_record():
    done = {"status": state.DONE, "mux": {"socket": "/nonexistent", "pane": "%1"}}
    assert tmux.settle_if_viewed(done, 5.0) == done


def test_throwaway_servers_have_their_own_names_and_leave_no_socket():
    if shutil.which("tmux") is None:
        pytest.skip("tmux is not installed")
    servers = [throwaway_tmux(os.devnull) for _ in range(2)]
    sockets = [run("display", "-p", "#{socket_path}").strip() for run, _ in servers]
    assert sockets[0] != sockets[1]
    for _, kill in servers:
        kill()
    assert not any(os.path.exists(socket) for socket in sockets)


def test_summary_text_counts_the_tabs_that_need_you_and_the_5h_quota():
    counts = {state.PERMISSION: 1, state.INPUT: 2, state.DONE: 1, state.WORKING: 3}
    limits = {"five_hour": {"pct": 62.4, "resets_at": 1}}
    assert tmux.summary_text(counts, limits) == (
        "#[fg=red bold]! 1 #[fg=yellow bold]? 2 #[fg=green]✓ 1 #[default]· 5h 62%"
    )


def test_summary_text_leaves_out_what_is_unknown():
    assert tmux.summary_text({state.WORKING: 2}, {}) == ""
    assert tmux.summary_text({}, {"five_hour": {"pct": 8, "resets_at": 1}}) == "5h 8%"
    assert tmux.summary_text({state.DONE: 1}, {}) == "#[fg=green]✓ 1"


def git_repo(path):
    subprocess.run(["git", "init", "-q", str(path)], check=True)
    return str(path)


def test_toggle_tig_splits_to_the_right_then_closes(server, monkeypatch, tmp_path):
    monkeypatch.setattr(tmux, "run", lambda *args: server(*args).rstrip("\n"))
    window = server("display", "-p", "#{window_id}").strip()
    tmux.toggle_tig(window, git_repo(tmp_path), command="sleep 60")
    panes = server("list-panes", "-t", window, "-F", "#{pane_left}\t#{pane_width}\t#{@cs-tig}")
    (left_pane, tig_pane) = (line.split("\t") for line in panes.splitlines())
    assert left_pane[0] == "0" and left_pane[2] == "" and tig_pane[2] == "1"
    assert abs(int(left_pane[1]) - int(tig_pane[1])) <= 2
    tmux.toggle_tig(window, str(tmp_path), command="sleep 60")
    assert len(server("list-panes", "-t", window).splitlines()) == 1


def test_toggle_tig_outside_a_repo_says_so(monkeypatch, tmp_path):
    calls = []
    monkeypatch.setattr(tmux, "run", lambda *args: calls.append(args) or "")
    tmux.toggle_tig("@1", str(tmp_path))
    assert [c[0] for c in calls] == ["list-panes", "display-message"]
    assert "Not a git repository" in calls[1][-1]
