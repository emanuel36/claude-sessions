from claude_sessions.core import state

NOW = 1000.0


def event(name, **fields):
    return {"session_id": "s1", "hook_event_name": name, "cwd": "/w/k265", **fields}


def run(*events, record=None):
    for e in events:
        record = state.apply_event(record, e, NOW)
    return record


def test_new_session_is_idle():
    r = run(event("SessionStart", source="startup"))
    assert (r["id"], r["status"], r["cwd"]) == ("s1", state.IDLE, "/w/k265")


def test_a_prompt_or_a_tool_means_working():
    assert run(event("UserPromptSubmit", prompt="x"))["status"] == state.WORKING
    assert run(event("PreToolUse", tool_name="Edit"))["status"] == state.WORKING


def test_permission_request_needs_permission_until_the_tool_runs():
    r = run(event("UserPromptSubmit", prompt="x"), event("PermissionRequest", tool_name="Bash"))
    assert r["status"] == state.PERMISSION
    assert run(event("PostToolUse", tool_name="Bash"), record=r)["status"] == state.WORKING


def test_questions_and_plans_need_input():
    for tool in ("AskUserQuestion", "ExitPlanMode"):
        asked = run(event("PreToolUse", tool_name=tool))
        assert asked["status"] == state.INPUT
        # Their permission request neither counts twice nor turns them into a permission
        again = run(event("PermissionRequest", tool_name=tool), record=asked)
        assert (again["status"], again["pending"]) == (state.INPUT, [["main", state.INPUT]])


def test_a_permission_outranks_a_question():
    asked = run(event("PreToolUse", tool_name="AskUserQuestion"))
    both = run(event("PermissionRequest", tool_name="Bash", agent_id="a1"), record=asked)
    assert both["status"] == state.PERMISSION
    assert (
        run(event("PostToolUse", tool_name="Bash", agent_id="a1"), record=both)["status"]
        == state.INPUT
    )


def test_notifications():
    working = run(event("UserPromptSubmit", prompt="x"))
    asked = run(event("Notification", notification_type="permission_prompt"), record=working)
    assert asked["status"] == state.PERMISSION
    elicited = run(event("Notification", notification_type="elicitation_dialog"), record=working)
    assert elicited["status"] == state.INPUT
    idle = run(event("Notification", notification_type="idle_prompt"), record=working)
    assert idle["status"] == state.WORKING


def test_another_agents_tool_use_keeps_a_pending_permission():
    asked = run(event("PermissionRequest", tool_name="Bash", agent_id="a1"))
    for other in (
        event("PreToolUse", tool_name="Read"),
        event("PostToolUse", tool_name="Grep", agent_id="a2"),
    ):
        assert run(other, record=asked)["status"] == state.PERMISSION
    assert (
        run(event("PostToolUse", tool_name="Bash", agent_id="a1"), record=asked)["status"]
        == state.WORKING
    )
    assert run(event("SubagentStop", agent_id="a1"), record=asked)["status"] == state.WORKING


def test_a_notification_already_explained_by_a_prompt_changes_nothing():
    asked = run(event("PermissionRequest", tool_name="Bash"))
    r = run(event("Notification", notification_type="permission_prompt"), record=asked)
    assert r["pending"] == asked["pending"]


def test_compaction_keeps_the_turn_going():
    working = run(event("UserPromptSubmit", prompt="x"))
    assert run(event("SessionStart", source="compact"), record=working)["status"] == state.WORKING
    assert run(event("SessionStart", source="clear"), record=working)["status"] == state.IDLE


def test_stop_finishes_the_turn():
    r = run(event("UserPromptSubmit", prompt="x"), event("Stop", background_tasks=[]))
    assert r["status"] == state.DONE
    assert run(event("StopFailure", error="rate_limit"))["status"] == state.DONE


def test_stop_with_background_tasks_keeps_working():
    tasks = [{"status": "running"}, {"status": "completed"}, {"status": "pending"}]
    r = run(event("Stop", background_tasks=tasks))
    assert (r["status"], r["background"]) == (state.WORKING, 2)


def test_stop_with_only_shell_commands_running_waits_on_the_shell():
    tasks = [{"type": "shell", "status": "running"}, {"type": "subagent", "status": "completed"}]
    r = run(event("UserPromptSubmit", prompt="x"), event("Stop", background_tasks=tasks))
    assert (r["status"], r["background"]) == (state.SHELL, 1)
    # The command finishes and wakes Claude, which works again
    assert run(event("PreToolUse", tool_name="Read"), record=r)["status"] == state.WORKING
    # A prompt while the build runs starts a turn that Esc can interrupt
    again = run(event("UserPromptSubmit", prompt="y"), record=r)
    assert (again["status"], again["background"]) == (state.WORKING, 0)


def test_stop_with_a_subagent_running_keeps_working():
    tasks = [{"type": "shell", "status": "running"}, {"type": "subagent", "status": "running"}]
    assert run(event("Stop", background_tasks=tasks))["status"] == state.WORKING


def test_seen_settles_a_finished_session_as_idle():
    seen = state.mark_seen(run(event("Stop")), NOW + 5)
    assert (seen["status"], seen["status_since"]) == (state.IDLE, NOW + 5)


def test_apply_event_does_not_modify_its_input():
    r = run(event("SessionStart"))
    run(event("UserPromptSubmit", prompt="x"), record=r)
    assert r["status"] == state.IDLE


def test_sort_puts_what_needs_you_first_and_oldest_first():
    a = {"status": state.WORKING, "updated_at": 1}
    b = {"status": state.PERMISSION, "updated_at": 9}
    c = {"status": state.PERMISSION, "updated_at": 2}
    d = {"status": state.INPUT, "updated_at": 0}
    assert sorted([a, b, c, d], key=state.sort_key) == [c, b, d, a]
    # An unknown status sorts last instead of failing
    old = {"status": "error", "updated_at": 0}
    assert sorted([old, a], key=state.sort_key) == [a, old]


def test_status_since_survives_hooks_that_keep_the_status():
    asked = state.apply_event(None, event("PermissionRequest", tool_name="Bash"), NOW)
    later = state.apply_event(
        asked, event("Notification", notification_type="permission_prompt"), NOW + 30
    )
    assert (later["status_since"], later["updated_at"]) == (NOW, NOW + 30)
    working = state.apply_event(later, event("PostToolUse", tool_name="Bash"), NOW + 40)
    assert working["status_since"] == NOW + 40


def test_project_is_the_folder_name():
    assert state.project({"cwd": "/w/notes/"}) == "notes"
    assert state.project({}) == "?"
