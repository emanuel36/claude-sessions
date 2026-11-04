"""`cs`: the Claude Code hook, and the command tmux runs when you look at a session.

cs hook                                      read a hook event on stdin, update the session
cs statusline                                record usage, then print the status line
cs seen --tmux-pane %3 --tmux-socket PATH    mark the session in that pane as seen
cs landing                                   show the landing page (what cst opens with)
cs sidebar                                   show the sessions' column in a window's left pane
cs new --pick                                pick a folder and start a session there (Alt+n)
cs layout [WINDOW_ID]                        put the sidebar in a window, fit them all
cs close WINDOW_ID                           stop the window's background sessions, close it
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
from claude_sessions.term import picker, tmux


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


def cmd_landing(_args):
    # Imported here: the hook runs on every event and must not pay for loading Textual
    from claude_sessions.ui import landing_app

    landing_app.main()
    return 0


def cmd_sidebar(_args):
    from claude_sessions.ui import sidebar_app

    sidebar_app.main()
    return 0


def cmd_new(args):
    if args.preview:
        print(picker.preview(args.preview))
    elif args.sessions:
        for line in picker.session_lines(args.sessions):
            print(line)
    elif args.projects:
        home = os.path.expanduser("~")
        # The folders of open sessions first, then the recent ones; a background run's folder
        # (a start hook's work folder) is no project
        live = hook.live_sessions(time.time())
        open_now = [r["cwd"] for r in live if r.get("cwd") and not r.get("headless")]
        for path in dict.fromkeys([*open_now, *files.recent_projects()]):
            print(path.replace(home, "~", 1) if path.startswith(home) else path)
    else:
        # Imported here: the hook runs on every event and needs none of it
        import importlib.resources

        script = importlib.resources.files("claude_sessions.term") / "data" / "new_session.sh"
        os.execvp("bash", ["bash", str(script)])
    return 0


def cmd_layout(args):
    if args.window:
        tmux.add_sidebar(args.window)
    tmux.fit_sidebars()
    return 0


def cmd_close(args):
    tmux.close_window(args.window)
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
    sub.add_parser("landing").set_defaults(func=cmd_landing)
    sub.add_parser("sidebar").set_defaults(func=cmd_sidebar)
    p_new = sub.add_parser("new")
    # Both flags serve the popup: --pick is what it runs, --projects feeds its list
    group = p_new.add_mutually_exclusive_group(required=True)
    group.add_argument("--pick", action="store_true")
    group.add_argument("--projects", action="store_true")
    group.add_argument("--preview", metavar="PATH")
    group.add_argument("--sessions", metavar="PATH")
    p_new.set_defaults(func=cmd_new)
    p_layout = sub.add_parser("layout")
    p_layout.add_argument("window", nargs="?")
    p_layout.set_defaults(func=cmd_layout)
    p_close = sub.add_parser("close")
    p_close.add_argument("window")
    p_close.set_defaults(func=cmd_close)
    sub.add_parser("install").set_defaults(func=cmd_install)
    sub.add_parser("uninstall").set_defaults(func=cmd_uninstall)
    args = parser.parse_args(argv)
    return args.func(args)
