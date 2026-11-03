"""`cs`: the Claude Code hook.

cs hook                                      read a hook event on stdin, update the session
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


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="cs", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("hook").set_defaults(func=cmd_hook)
    args = parser.parse_args(argv)
    return args.func(args)
