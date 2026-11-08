import fcntl
import json
import os

from claude_sessions.claude import hook, install
from claude_sessions.core import paths, state, store

ENV = {"CLAUDE_PID": str(os.getpid())}


def live_record(session_id="s1", **fields):
    pid = os.getpid()
    return {
        **state.new_record(session_id),
        "pid": pid,
        "pid_start": store.process_start(pid),
        **fields,
    }


def write_transcript(path, *entries):
    path.write_text("".join(json.dumps(e) + "\n" for e in entries))
    return str(path)


def test_hook_outside_tmux_writes_record():
    hook.handle_event(
        {
            "session_id": "s1",
            "hook_event_name": "UserPromptSubmit",
            "cwd": "/nonexistent",
            "prompt": "hi",
        },
        ENV,
        5.0,
    )
    r = store.read("s1")
    assert (r["status"], r["pid"], r["mux"]) == (state.WORKING, os.getpid(), {})


def test_a_headless_run_is_marked_and_takes_no_tab(monkeypatch):
    named = []
    monkeypatch.setattr(hook.tmux, "name_window", lambda r, title="": named.append(r))
    env = {**ENV, "TMUX": "/tmp/t/claude,1,0", "TMUX_PANE": "%3"}
    event = {"session_id": "s1", "hook_event_name": "UserPromptSubmit", "cwd": "/tmp/daily-recap"}
    headless = {**env, "CLAUDE_CODE_ENTRYPOINT": "sdk-cli"}
    hook.handle_event(event, headless, 5.0)
    r = store.read("s1")
    assert (r["headless"], r["mux"], r["started_at"]) == (True, {}, 5.0)
    # Later events keep when it started
    hook.handle_event({**event, "hook_event_name": "PreToolUse"}, headless, 9.0)
    assert store.read("s1")["started_at"] == 5.0
    hook.handle_event({**event, "session_id": "s2"}, {**env, "CLAUDE_CODE_ENTRYPOINT": "cli"}, 5.0)
    assert store.read("s2")["headless"] is False


def test_hook_with_dead_tmux_socket_does_not_raise():
    env = {**ENV, "TMUX": "/nonexistent,1,0", "TMUX_PANE": "%1"}
    hook.handle_event(
        {"session_id": "s1", "hook_event_name": "UserPromptSubmit", "cwd": "/w/notes"}, env, 5.0
    )
    r = store.read("s1")
    assert (r["status"], r["mux"]) == (state.WORKING, {"socket": "/nonexistent", "pane": "%1"})


def test_a_clear_or_resume_drops_the_session_it_replaced():
    hook.handle_event({"session_id": "before", "hook_event_name": "SessionStart"}, ENV, 1.0)
    store.write({**state.new_record("other-process"), "pid": os.getpid(), "pid_start": -2})
    hook.handle_event(
        {"session_id": "after", "hook_event_name": "SessionStart", "source": "resume"}, ENV, 2.0
    )
    assert store.read("before") is None and store.read("after") is not None
    assert store.read("other-process") is not None


def test_hook_session_end_removes_the_record():
    hook.handle_event({"session_id": "s1", "hook_event_name": "SessionStart"}, ENV, 1.0)
    hook.handle_event({"session_id": "s1", "hook_event_name": "SessionEnd"}, ENV, 2.0)
    assert store.read("s1") is None


def test_session_end_of_a_clear_or_resume_keeps_the_record():
    # The new session may start first, and a resumed session has the same id
    hook.handle_event({"session_id": "s1", "hook_event_name": "SessionStart"}, ENV, 1.0)
    for reason in ("clear", "resume"):
        end = {"session_id": "s1", "hook_event_name": "SessionEnd", "reason": reason}
        hook.handle_event(end, ENV, 2.0)
        assert store.read("s1") is not None
    exit_ = {"session_id": "s1", "hook_event_name": "SessionEnd", "reason": "prompt_input_exit"}
    hook.handle_event(exit_, ENV, 3.0)
    assert store.read("s1") is None


def test_seen_ignores_the_same_pane_on_another_tmux_server():
    pid = os.getpid()

    def done(session_id, socket):
        return {
            **state.new_record(session_id),
            "pid": pid,
            "pid_start": store.process_start(pid),
            "status": state.DONE,
            "mux": {"socket": socket, "pane": "%3"},
        }

    store.write(done("mine", "/tmp/a"))
    store.write(done("other", "/tmp/b"))
    hook.seen("%3", "/tmp/a", 0)
    assert store.read("mine")["status"] == state.IDLE
    assert store.read("other")["status"] == state.DONE


def lock_is_held(session_id):
    """Tells whether someone holds the session's lock, by trying to take it."""
    with open(os.path.join(paths.state_dir(), f"{session_id}.json.lock"), "a") as f:
        try:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        fcntl.flock(f, fcntl.LOCK_UN)
        return False


def test_hook_names_the_window_while_it_holds_the_lock(monkeypatch):
    held = []
    monkeypatch.setattr(
        hook.tmux, "name_window", lambda r, title="": held.append(lock_is_held("s1"))
    )
    hook.handle_event({"session_id": "s1", "hook_event_name": "UserPromptSubmit"}, ENV, 1.0)
    hook.handle_event({"session_id": "s1", "hook_event_name": "SessionEnd"}, ENV, 2.0)
    assert held == [True]


def test_interrupted_working_session_is_shown_idle(tmp_path):
    transcript = write_transcript(
        tmp_path / "t.jsonl",
        {"type": "user", "message": {"content": "do it"}},
        {"type": "assistant", "message": {"content": "ok"}},
        {"type": "user", "message": {"content": "[Request interrupted by user]"}},
    )
    store.write(
        live_record(status=state.WORKING, transcript=transcript, updated_at=0, background=0)
    )
    [r] = hook.live_sessions(100)
    assert r["status"] == state.IDLE
    assert store.read("s1")["status"] == state.IDLE


def test_a_prompt_rejected_with_esc_is_settled(tmp_path):
    transcript = write_transcript(
        tmp_path / "t.jsonl",
        {"type": "user", "message": {"content": "[Request interrupted by user for tool use]"}},
    )
    store.write(
        live_record(
            status=state.PERMISSION,
            pending=[["main", state.PERMISSION]],
            transcript=transcript,
            updated_at=0,
        )
    )
    [r] = hook.live_sessions(100)
    assert (r["status"], r["pending"]) == (state.IDLE, [])


def test_recent_working_session_is_not_checked_for_interrupts(tmp_path):
    transcript = write_transcript(
        tmp_path / "t.jsonl",
        {"type": "user", "message": {"content": "[Request interrupted by user]"}},
    )
    store.write(live_record(status=state.WORKING, transcript=transcript, updated_at=99))
    assert hook.live_sessions(100)[0]["status"] == state.WORKING


def test_the_tmux_calls_of_a_hook_fit_in_its_timeout():
    # A prompt: name_window makes 3 tmux calls and settle_if_viewed 1
    assert 4 * hook.tmux.TIMEOUT < install.HOOK_TIMEOUT


def test_hook_names_the_window_after_the_sessions_title(monkeypatch, tmp_path):
    transcript = tmp_path / "s1.jsonl"
    transcript.write_text('{"type":"ai-title","aiTitle":"Fix the login bug"}\n')
    named = []
    monkeypatch.setattr(hook.tmux, "name_window", lambda r, title="": named.append(title))
    event = {"session_id": "s1", "hook_event_name": "Stop", "transcript_path": str(transcript)}
    hook.handle_event(event, ENV, 1.0)
    assert named == ["Fix the login bug"]


def test_hook_keeps_a_title_that_scrolled_out_of_the_transcripts_tail(monkeypatch, tmp_path):
    transcript = tmp_path / "s1.jsonl"
    transcript.write_text('{"type":"ai-title","aiTitle":"Fix the login bug"}\n')
    named = []
    monkeypatch.setattr(hook.tmux, "name_window", lambda r, title="": named.append(title))
    event = {"session_id": "s1", "hook_event_name": "Stop", "transcript_path": str(transcript)}
    hook.handle_event(event, ENV, 1.0)
    transcript.write_text('{"type":"user"}\n')
    hook.handle_event({**event, "hook_event_name": "UserPromptSubmit"}, ENV, 2.0)
    assert named == ["Fix the login bug", "Fix the login bug"]
