"""`cs`: the Claude Code hook, and the command tmux runs when you look at a session.

cs hook                                      read a hook event on stdin, update the session
cs seen --tmux-pane %3 --tmux-socket PATH    mark the session in that pane as seen
cs panel                                     show the open and recent sessions (tab 1)
cs next                                      go to the next session that needs you
"""

import argparse
import json
import os
import sys
import time

from claude_sessions.claude import hook
from claude_sessions.core import paths
from claude_sessions.term import tmux


def cmd_hook(_args):
    try:
        hook.handle_event(json.load(sys.stdin.buffer), dict(os.environ), time.time())
    except Exception:  # noqa: BLE001 - a hook must never break the session
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


def cmd_next(_args):
    windows = tmux.windows_of(hook.live_sessions(time.time()))
    target = tmux.next_pending(windows, tmux.run("display-message", "-p", "#{window_id}"))
    if target is None:
        tmux.run("display-message", "Nothing is waiting for you")
    else:
        tmux.select(target)
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="cs", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("hook").set_defaults(func=cmd_hook)
    p_seen = sub.add_parser("seen")
    p_seen.add_argument("--tmux-pane", required=True)
    p_seen.add_argument("--tmux-socket", required=True)
    p_seen.set_defaults(func=cmd_seen)
    sub.add_parser("panel").set_defaults(func=cmd_panel)
    sub.add_parser("next").set_defaults(func=cmd_next)
    args = parser.parse_args(argv)
    return args.func(args)
