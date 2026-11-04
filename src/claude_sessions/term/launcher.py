"""`cst`: open the tmux server of Claude Code sessions, creating it if needed.

cst            attach to the `claude` tmux session in this terminal, creating it if needed
cst --ghostty  open it in a new Ghostty window that leaves Alt+↑↓ to tmux
"""

import importlib.resources
import os
import subprocess
import sys

SOCKET = "claude"
SESSION = "claude"
# The landing page stays as the first window: closing the last session returns to it
LANDING = "cs landing"


def ghostty_command(cst):
    """Returns the Ghostty command line that runs `cst` with the Alt keys unbound.

    Ghostty moves between splits with Alt+↑↓; a separate instance lets these flags apply.
    """
    keybinds = []
    # Alt+←→ stay with Ghostty: tmux does not use them
    for way in ("up", "down"):
        keybinds += [f"--keybind=alt+{way}=unbind", f"--keybind=alt+arrow_{way}=unbind"]
    return ["ghostty", "--gtk-single-instance=false", *keybinds, "-e", cst]


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    here = os.path.dirname(os.path.abspath(sys.argv[0]))
    if argv[:1] == ["--ghostty"]:
        cmd = ghostty_command(os.path.join(here, "cst"))
        os.execvp(cmd[0], cmd)
    # tmux runs the hook's `cs`, which lives beside this script
    os.environ["PATH"] = here + os.pathsep + os.environ.get("PATH", "")
    conf = importlib.resources.files("claude_sessions.term") / "data" / "tmux.conf"
    tmux = ["tmux", "-L", SOCKET, "-f", str(conf)]
    try:
        start_session(tmux)
        # The landing page is the first window; marked so Alt+Up/Down step over it. Set on
        # every start, so a server from before the mark gets it too
        subprocess.run([*tmux, "set-option", "-w", "-t", f"{SESSION}:^", "@cs-landing", "1"])
    except FileNotFoundError:
        print("cst: tmux is not installed", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError:
        print("cst: tmux could not create the claude session", file=sys.stderr)
        return 1
    cmd = [*tmux, "attach-session", "-t", SESSION]
    os.execvp(cmd[0], cmd)
    return 0


def start_session(tmux):
    """Creates the `claude` tmux session, unless the server already has it."""
    if subprocess.run([*tmux, "has-session", "-t", SESSION], capture_output=True).returncode:
        # Window 1 is the landing page
        subprocess.run(
            [
                *tmux,
                "new-session",
                "-d",
                "-s",
                SESSION,
                "-n",
                "sessions",
                "-c",
                os.path.expanduser("~"),
                LANDING,
            ],
            check=True,
        )
