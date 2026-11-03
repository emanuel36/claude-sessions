"""Registers `cs` in Claude Code's settings.json, and takes it out again."""

import copy
import json
import os
import shlex
import sys

from claude_sessions.claude import statusline
from claude_sessions.core import paths

EVENTS = (
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Notification",
    "Stop",
    "StopFailure",
    "SubagentStop",
)
BACKUP = "settings.json.before-claude-sessions"
HOOK_TIMEOUT = 10


def hook_command(cs):
    return f"{shlex.quote(cs)} hook"


def statusline_command(cs):
    return f"{shlex.quote(cs)} statusline"


def is_ours(command):
    """Tells whether a command is `cs hook` or `cs statusline`, whichever path `cs` has."""
    try:
        argv = shlex.split(command)
    except (ValueError, AttributeError):
        return False
    return (
        len(argv) == 2 and os.path.basename(argv[0]) == "cs" and argv[1] in ("hook", "statusline")
    )


def _command_of(entry):
    return entry.get("command") if isinstance(entry, dict) else None


def _group_has_ours(group):
    hooks = group.get("hooks") if isinstance(group, dict) else None
    return isinstance(hooks, list) and any(is_ours(_command_of(h) or "") for h in hooks)


def shape_error(settings):
    """Returns what is malformed in settings for our purposes, or None."""
    hooks = settings.get("hooks", {})
    if not isinstance(hooks, dict):
        return "hooks is not an object"
    for event, groups in hooks.items():
        if not isinstance(groups, list):
            return f"hooks.{event} is not a list"
        if not all(isinstance(g, dict) and isinstance(g.get("hooks"), list) for g in groups):
            return f"hooks.{event} has a group without a hooks list"
    if "statusLine" in settings and not isinstance(settings["statusLine"], dict):
        return "statusLine is not an object"
    return None


def _repoint(groups, command):
    """Points our entries in groups at command. Returns whether any of them was ours."""
    found = False
    for group in groups:
        for hook in group["hooks"]:
            if is_ours(_command_of(hook) or ""):
                found = True
                hook["command"] = command
    return found


def merge(settings, cs, saved):
    """Adds our hooks and status line to settings, or points ours at cs if it moved.

    Args:
        settings: The parsed settings.json. It is not modified.
        cs: The path of the `cs` executable.
        saved: The user's own status line saved by an earlier install, if any.

    Returns:
        The new settings, and the status line to save (the user's own, or None).
    """
    new = copy.deepcopy(settings)
    hooks = new.setdefault("hooks", {})
    for event in EVENTS:
        groups = hooks.setdefault(event, [])
        if not _repoint(groups, hook_command(cs)):
            entry = {"type": "command", "command": hook_command(cs), "timeout": HOOK_TIMEOUT}
            groups.append({"hooks": [entry]})
    current = new.get("statusLine")
    if isinstance(current, dict) and is_ours(_command_of(current) or ""):
        current["command"] = statusline_command(cs)
        return new, saved
    if isinstance(current, dict):
        new["statusLine"] = {**current, "command": statusline_command(cs)}
        return new, current
    new["statusLine"] = {"type": "command", "command": statusline_command(cs)}
    return new, None


def unmerge(settings, saved):
    """Takes our hooks out and puts the user's own status line back.

    An event list, or the hooks object, that only our entries filled goes away with them; an
    empty one the user had stays.

    Args:
        settings: The parsed settings.json. It is not modified.
        saved: The user's own status line saved by install, if any.
    """
    new = copy.deepcopy(settings)
    hooks = new.get("hooks")
    if isinstance(hooks, dict):
        emptied = False
        for event in list(hooks):
            groups = hooks[event]
            if not isinstance(groups, list) or not any(_group_has_ours(g) for g in groups):
                continue
            kept_groups = []
            for group in groups:
                if _group_has_ours(group):
                    group = {
                        **group,
                        "hooks": [h for h in group["hooks"] if not is_ours(_command_of(h) or "")],
                    }
                    if not group["hooks"]:
                        continue
                kept_groups.append(group)
            if kept_groups:
                hooks[event] = kept_groups
            else:
                del hooks[event]
                emptied = True
        if emptied and not hooks:
            del new["hooks"]
    current = new.get("statusLine")
    if isinstance(current, dict) and is_ours(_command_of(current) or ""):
        if saved is not None:
            new["statusLine"] = saved
        else:
            del new["statusLine"]
    return new


def settings_path():
    return os.path.join(paths.config_dir(), "settings.json")


def _dump(settings):
    return json.dumps(settings, indent=2, ensure_ascii=False) + "\n"


def _read_json(path):
    """Returns the parsed JSON object, {} if the file is missing, or None if it is not one."""
    try:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
    except FileNotFoundError:
        return {}
    except ValueError:
        return None
    return data if isinstance(data, dict) else None


def _saved():
    """Returns the user's own status line saved by install, or None if there is none."""
    path = statusline.saved_statusline_path()
    return _read_json(path) if os.path.exists(path) else None


def _fail(path, why):
    print(f"error: {path}: {why}; nothing was changed", file=sys.stderr)
    return 1


def _load_settings(path):
    """Returns (settings, None), or (None, exit code) after printing why it cannot be used."""
    try:
        settings = _read_json(path)
    except OSError as e:
        return None, _fail(path, e.strerror or "cannot be read")
    if settings is None:
        return None, _fail(path, "not a valid JSON object")
    problem = shape_error(settings)
    if problem:
        return None, _fail(path, problem)
    return settings, None


def install(cs):
    """Registers `cs` in settings.json. Returns the exit code."""
    try:
        return _install(cs)
    except OSError as e:
        return _fail(settings_path(), e.strerror or "cannot be written")


def _install(cs):
    path = settings_path()
    settings, code = _load_settings(path)
    if settings is None:
        return code
    new, to_save = merge(settings, cs, _saved())
    old_hooks = settings.get("hooks", {})
    new_hooks = new["hooks"]
    added = [e for e in EVENTS if not any(map(_group_has_ours, old_hooks.get(e, [])))]
    updated = [e for e in EVENTS if e not in added and new_hooks[e] != old_hooks[e]]
    changes = []
    if added:
        changes.append(f"hooks: added to {len(added)} events")
    if updated:
        changes.append(f"hooks: updated to {cs}")
    old_line = settings.get("statusLine")
    if new["statusLine"] != old_line:
        if to_save is not None:
            changes.append("status line: now cs statusline (yours is kept and still shown)")
        elif old_line is not None:
            changes.append(f"status line: updated to {cs}")
        else:
            changes.append("status line: now cs statusline")
    if not changes:
        print("already installed")
        return 0
    real = os.path.realpath(path)
    os.makedirs(os.path.dirname(real), exist_ok=True)
    os.makedirs(paths.state_home(), exist_ok=True)
    if os.path.exists(real):
        backup = os.path.join(os.path.dirname(real), BACKUP)
        if not os.path.exists(backup):
            with open(real, encoding="utf-8") as src:
                paths.write_text(backup, src.read(), os.stat(real).st_mode)
            changes.append(f"backup: {backup}")
    if to_save is not None:
        paths.write_json(statusline.saved_statusline_path(), to_save)
    paths.write_text(real, _dump(new))
    print("\n".join(changes))
    return 0


def uninstall():
    """Takes `cs` out of settings.json. Returns the exit code."""
    try:
        return _uninstall()
    except OSError as e:
        return _fail(settings_path(), e.strerror or "cannot be written")


def _uninstall():
    path = settings_path()
    settings, code = _load_settings(path)
    if settings is None:
        return code
    saved = _saved()
    if saved is None and os.path.exists(statusline.saved_statusline_path()):
        return _fail(statusline.saved_statusline_path(), "not a valid JSON object")
    new = unmerge(settings, saved)
    changes = []
    if new.get("hooks") != settings.get("hooks"):
        changes.append("hooks: removed")
    if new.get("statusLine") != settings.get("statusLine"):
        changes.append(
            "status line: yours restored" if saved is not None else "status line: removed"
        )
    if new != settings:
        paths.write_text(os.path.realpath(path), _dump(new))
    try:
        os.remove(statusline.saved_statusline_path())
    except FileNotFoundError:
        pass
    print("\n".join(changes) if changes else "nothing to uninstall")
    return 0
