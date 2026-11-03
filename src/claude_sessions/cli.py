"""`cs`: the Claude Code hook, and the command tmux runs when you look at a session.

cs hook                                      read a hook event on stdin, update the session
cs statusline                                record usage, then print the status line
cs seen --tmux-pane %3 --tmux-socket PATH    mark the session in that pane as seen
cs panel                                     show the open and recent sessions (tab 1)
cs new --pick                                pick a folder and start a session there (Alt+c)
cs next                                      go to the next session that needs you
cs install                                   register cs in Claude Code's settings.json
cs uninstall                                 take it out again
"""

import argparse
import json
import os
import sys
import time

from claude_sessions.claude import files, hook, install, statusline
from claude_sessions.core import paths
from claude_sessions.term import tmux


def cmd_hook(_args):
    try:
        hook.handle_event(json.load(sys.stdin.buffer), dict(os.environ), time.time())
    except Exception:  # noqa: BLE001 - a hook must never break the session
        paths.log_error()
    return 0


def cmd_statusline(_args):
    # Claude Code cancels this script when the next update comes: record first, print after
    raw = sys.stdin.buffer.read()
    data = {}
    try:
        data = json.loads(raw)
        if isinstance(data, dict):
            statusline.record_usage(data, time.time())
    except Exception:  # noqa: BLE001 - the status line must print whatever happens
        paths.log_error()
    try:
        statusline.print_statusline(raw, data if isinstance(data, dict) else {})
    except Exception:  # noqa: BLE001
        paths.log_error()
    return 0


def cmd_seen(args):
    hook.seen(args.tmux_pane, args.tmux_socket, time.time())
    return 0


def cmd_panel(_args):
    # Imported here: the hook runs on every event and must not pay for loading Textual
    from claude_sessions.panel import app

    app.main()
    return 0


def cmd_new(args):
    if args.projects:
        home = os.path.expanduser("~")
        for path in files.recent_projects():
            print(path.replace(home, "~", 1) if path.startswith(home) else path)
    else:
        # Imported here: the hook runs on every event and needs none of it
        import importlib.resources

        script = importlib.resources.files("claude_sessions.term") / "data" / "new_session.sh"
        os.execvp("bash", ["bash", str(script)])
    return 0


def cmd_next(_args):
    windows = tmux.windows_of(hook.live_sessions(time.time()))
    target = tmux.next_pending(windows, tmux.run("display-message", "-p", "#{window_id}"))
    if target is None:
        tmux.run("display-message", "Nothing is waiting for you")
    else:
        tmux.select(target)
    return 0


def cmd_tig(args):
    tmux.toggle_tig(args.window, args.path)
    return 0


def cmd_install(_args):
    return install.install(os.path.abspath(sys.argv[0]))


def cmd_uninstall(_args):
    return install.uninstall()


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="cs", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("hook").set_defaults(func=cmd_hook)
    sub.add_parser("statusline").set_defaults(func=cmd_statusline)
    p_seen = sub.add_parser("seen")
    p_seen.add_argument("--tmux-pane", required=True)
    p_seen.add_argument("--tmux-socket", required=True)
    p_seen.set_defaults(func=cmd_seen)
    sub.add_parser("panel").set_defaults(func=cmd_panel)
    p_new = sub.add_parser("new")
    # Both flags serve the popup: --pick is what it runs, --projects feeds its list
    group = p_new.add_mutually_exclusive_group(required=True)
    group.add_argument("--pick", action="store_true")
    group.add_argument("--projects", action="store_true")
    p_new.set_defaults(func=cmd_new)
    sub.add_parser("next").set_defaults(func=cmd_next)
    p_tig = sub.add_parser("tig")
    p_tig.add_argument("window")
    p_tig.add_argument("path")
    p_tig.set_defaults(func=cmd_tig)
    sub.add_parser("install").set_defaults(func=cmd_install)
    sub.add_parser("uninstall").set_defaults(func=cmd_uninstall)
    args = parser.parse_args(argv)
    return args.func(args)
