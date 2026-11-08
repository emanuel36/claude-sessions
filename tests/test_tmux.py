import os
import pty
import shutil
import signal
import time

import pytest
from conftest import throwaway_tmux

from claude_sessions.core import state
from claude_sessions.term import tmux

TMUX = {"TMUX": "/tmp/ttmux-1000/claude,123,0", "TMUX_PANE": "%3"}


def test_detect():
    assert tmux.detect({}) == {}
    assert tmux.detect(TMUX) == {"socket": "/tmp/ttmux-1000/claude", "pane": "%3"}


def test_detect_ignores_the_tab_a_headless_run_inherited(monkeypatch):
    # A `claude -p` started from a session (a start hook, a script) carries that tab's TMUX
    assert tmux.detect({**TMUX, "CLAUDE_CODE_ENTRYPOINT": "sdk-cli"}, "s1") == {}
    assert tmux.detect({**TMUX, "CLAUDE_CODE_ENTRYPOINT": "cli"}, "s1")["pane"] == "%3"
    assert tmux.is_headless({"CLAUDE_CODE_ENTRYPOINT": "sdk-py"})
    # A daemon session is looked up by its attach tab whatever its entrypoint says
    monkeypatch.setattr(tmux, "attached_pane", lambda sid: {"socket": "/s", "pane": "%9"})
    job = {"CLAUDE_JOB_DIR": "/j", "CLAUDE_CODE_ENTRYPOINT": "sdk-cli"}
    assert tmux.detect(job, "s1")["pane"] == "%9"
    assert not tmux.is_headless({})


def test_detect_finds_the_tab_attached_to_a_background_session(monkeypatch):
    sid = "685e0c8a-d242-44fd-ab34-55ac744fd5ab"
    panes = "/s/claude\t%1\t100\n/s/claude\t%2\t200"
    commands = {"100": ["zsh"], "101": ["/bin/claude", "attach", "685e0c8a"], "200": ["claude"]}
    monkeypatch.setattr(tmux, "run", lambda *args: panes)
    monkeypatch.setattr(tmux, "_cmdline", lambda pid: commands.get(pid, []))
    monkeypatch.setattr(tmux, "_children", lambda pid: ["101"] if pid == "100" else [])
    bg = {"CLAUDE_CODE_SESSION_KIND": "bg"}
    assert tmux.detect(bg, sid) == {"socket": "/s/claude", "pane": "%1"}
    # What a hook sees: the kind is not passed on, the job folder is
    job = {"CLAUDE_JOB_DIR": "/home/u/.claude/jobs/685e0c8a"}
    assert tmux.detect(job, sid) == {"socket": "/s/claude", "pane": "%1"}
    # Not attached anywhere, or not a background session: no pane
    assert tmux.detect(bg, "ffffffff-0000") == {}
    assert tmux.detect({}, sid) == {}
    # The tmux pane a background session left behind does not count
    assert tmux.detect({**bg, **TMUX}, sid) == {"socket": "/s/claude", "pane": "%1"}
    # A line tmux did not format as asked is skipped
    monkeypatch.setattr(tmux, "run", lambda *args: "warning\n" + panes)
    assert tmux.detect(bg, sid) == {"socket": "/s/claude", "pane": "%1"}


def test_attaches_matches_the_session_id_or_its_prefix():
    sid = "685e0c8a-d242-44fd-ab34-55ac744fd5ab"
    assert tmux.attaches(["/bin/claude", "attach", "685e0c8a"], sid)
    assert tmux.attaches(["claude", "attach", sid], sid)
    assert tmux.attaches(["node", "/lib/claude/cli.js", "attach", "685e0c8a"], sid)
    assert not tmux.attaches(["claude", "attach", "685e"], sid)
    assert not tmux.attaches(["claude", "--resume", "685e0c8a"], sid)
    assert not tmux.attaches([], sid)


def test_window_name_is_the_title_or_else_the_project_folder():
    assert tmux.window_name({"cwd": "/w/vehicle.k265"}) == "vehicle.k265"
    assert tmux.window_name({"cwd": "/w/notes/"}) == "notes"
    assert tmux.window_name({"cwd": "/w/api"}, "Fix the\nlogin  bug") == "Fix the login bug"
    long = tmux.window_name({"cwd": "/w/" + "x" * 80})
    assert len(long) == tmux.WINDOW_NAME_LEN and long.endswith("…")


def test_run_returns_empty_when_tmux_fails(monkeypatch):
    def fail(*args, **kwargs):
        raise OSError

    monkeypatch.setattr(tmux.subprocess, "run", fail)
    assert tmux.run("list-windows") == ""


def test_windows_of_keeps_this_server_and_the_most_urgent_per_window(monkeypatch):
    monkeypatch.setenv("TMUX", "/s/claude,1,0")
    panes = "%1\t@1\t1\n%2\t@1\t2\n%3\t@2\t1\n%4\t@3\t1"
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


def test_windows_of_picks_the_leader_by_its_pane_not_by_urgency(monkeypatch):
    monkeypatch.setenv("TMUX", "/s/claude,1,0")
    # The sidebar is pane 1; the first Claude pane is 2, a teammate's is 3
    panes = "%9\t@1\t1\n%2\t@1\t2\n%3\t@1\t3\n%7\t@2\t4\n%6\t@2\t3"

    def record(session_id, status, pane):
        return {"id": session_id, "status": status, "mux": {"socket": "/s/claude", "pane": pane}}

    records = [
        record("mate", state.PERMISSION, "%3"),
        record("lead", state.IDLE, "%2"),
        record("late", state.WORKING, "%7"),
        record("early", state.WORKING, "%6"),
    ]
    monkeypatch.setattr(tmux, "run", lambda *args: panes)
    by_window = {w.window_id: w for w in tmux.windows_of(records)}
    assert (by_window["@1"].lead["id"], by_window["@1"].leader["id"]) == ("mate", "lead")
    assert by_window["@1"].lead["status"] == state.PERMISSION
    assert by_window["@2"].leader["id"] == "early"


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


def test_close_window_stops_attached_background_sessions(monkeypatch):
    calls = []
    stops = []

    def run(*args):
        calls.append(args)
        return "100\n200" if args[0] == "list-panes" else ""

    commands = {"101": ["/bin/claude", "attach", "685e0c8a"], "200": ["claude"]}
    monkeypatch.setattr(tmux, "run", run)
    monkeypatch.setattr(tmux, "_cmdline", lambda pid: commands.get(pid, []))
    monkeypatch.setattr(tmux, "_children", lambda pid: ["101"] if pid == "100" else [])
    monkeypatch.setattr(tmux.subprocess, "run", lambda cmd, **kw: stops.append((cmd, kw)) or None)
    tmux.close_window("@2")
    assert calls[0] == ("list-panes", "-t", "@2", "-F", "#{pane_pid}")
    assert calls[-1] == ("kill-window", "-t", "@2")
    assert stops == [(["claude", "stop", "685e0c8a"], {"capture_output": True, "timeout": 10})]


def test_close_window_closes_even_when_claude_stop_fails(monkeypatch):
    calls = []
    monkeypatch.setattr(
        tmux, "run", lambda *a: calls.append(a) or ("100" if a[0] == "list-panes" else "")
    )
    monkeypatch.setattr(tmux, "_cmdline", lambda pid: ["claude", "attach", "685e0c8a"])
    monkeypatch.setattr(tmux, "_children", lambda pid: [])

    def boom(*args, **kwargs):
        raise tmux.subprocess.TimeoutExpired("claude", 10)

    monkeypatch.setattr(tmux.subprocess, "run", boom)
    tmux.close_window("@2")
    assert calls[-1] == ("kill-window", "-t", "@2")


def fake_windows(monkeypatch, active):
    """Windows @1 (the landing page, no sidebar), @2 and @3 (sessions), and what tmux was told."""
    calls = []

    def run(*args):
        calls.append(args)
        if args[:2] == ("list-panes", "-a"):
            return "@1 \n@2 1\n@2 \n@3 1\n@3 "
        if args[0] == "list-windows":
            return "@1\t1\tsessions\n@2\t2\tapi\n@3\t3\tweb"
        if args[0] == "display-message":
            return active
        return ""

    monkeypatch.setattr(tmux, "run", run)
    return calls


def test_closing_the_window_on_screen_moves_to_the_next_session_not_the_landing(monkeypatch):
    calls = fake_windows(monkeypatch, active="1")
    tmux.close_window("@2")
    assert calls[-2:] == [("select-window", "-t", "@3"), ("kill-window", "-t", "@2")]
    assert tmux.next_session_window("@3") == "@2"


def test_closing_the_last_session_or_a_hidden_one_selects_nothing(monkeypatch):
    calls = fake_windows(monkeypatch, active="0")
    tmux.close_window("@2")
    assert not any(c[0] == "select-window" for c in calls)
    monkeypatch.setattr(
        tmux, "run", lambda *a: "@1\t1\tsessions\n@2\t2\tapi" if a[0] == "list-windows" else "@2 1"
    )
    assert tmux.next_session_window("@2") is None


@pytest.fixture
def in_server(server, monkeypatch):
    """Points the module's `run`, which has no -S, at the throwaway server, as $TMUX does."""
    socket = server("display", "-p", "#{socket_path}").strip()
    monkeypatch.setenv("TMUX", f"{socket},1,0")
    return server


def window_of(server, window):
    return server("display", "-p", "-t", f":{window}", "#{window_id}").strip()


def list_panes(server, window, fmt):
    return server("list-panes", "-t", f":{window}", "-F", fmt).splitlines()


def sidebar_width(server, window):
    fmt = "#{?@cs-sidebar,#{pane_width},}"
    return int(next(w for w in list_panes(server, window, fmt) if w))


def test_add_sidebar_puts_a_fixed_width_pane_on_the_left(in_server):
    tmux.add_sidebar(window_of(in_server, 1), command="sleep 600")
    fmt = "#{pane_left}\t#{pane_width}\t#{@cs-sidebar}\t#{pane_active}"
    panes = list_panes(in_server, 1, fmt)
    assert panes[0] == f"0\t{tmux.SIDEBAR_WIDTH}\t1\t0" and panes[1].endswith("\t\t1")


def test_add_sidebar_twice_adds_one(in_server):
    window = window_of(in_server, 1)
    tmux.add_sidebar(window, command="sleep 600")
    tmux.add_sidebar(window, command="sleep 600")
    assert len(list_panes(in_server, 1, "#{pane_id}")) == 2


def test_teammate_split_keeps_the_sidebar_width(in_server):
    window = window_of(in_server, 1)
    tmux.add_sidebar(window, command="sleep 600")
    in_server("split-window", "-h", "-t", window)
    in_server("resize-pane", "-t", f"{window}.0", "-x", "50")
    assert sidebar_width(in_server, 1) == 50
    tmux.fit_sidebars()
    assert sidebar_width(in_server, 1) == tmux.SIDEBAR_WIDTH
