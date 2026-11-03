"""`cs`: the Claude Code hook, and the command tmux runs when you look at a session.

cs hook                                      read a hook event on stdin, update the session
cs seen --tmux-pane %3 --tmux-socket PATH    mark the session in that pane as seen
cs panel                                     show the open and recent sessions (tab 1)
"""

import argparse
import json
import os
import sys
import time

from claude_sessions.claude import hook
from claude_sessions.core import paths


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
    args = parser.parse_args(argv)
    return args.func(args)
