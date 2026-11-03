# Installation

## Requirements

| Needs | Version | What for |
|---|---|---|
| Linux | any | Session liveness is read from `/proc`. macOS is not supported |
| Python | 3.11 or newer | `uv` installs it if it is missing |
| [uv](https://docs.astral.sh/uv/) | recent | Installs the `cs` and `cst` commands |
| [tmux](https://github.com/tmux/tmux) | 3.3 or newer | The tabs and the popups |
| [fzf](https://github.com/junegunn/fzf) | any | The folder picker of Alt+c and of the panel's n key |
| [Claude Code](https://github.com/anthropics/claude-code) | recent | The sessions themselves (`claude` on your `PATH`) |
| [tig](https://jonas.github.io/tig/) | optional | The commit history popup of Alt+g |
| [Ghostty](https://ghostty.org) | optional | `cst --ghostty`, a terminal window set up for the Alt keys |

## Install

One command installs everything and registers it with Claude Code:

```sh
curl -fsSL https://raw.githubusercontent.com/emanuel36/claude-sessions/main/install.sh | sh
```

The script:

1. checks that this is Linux, that tmux is 3.3 or newer and that fzf is there, and says when
   tig is missing;
2. installs [uv](https://docs.astral.sh/uv/) with its official installer, if it is missing
   (that installer may add `~/.local/bin` to your shell's startup files);
3. installs the `cs` and `cst` commands with `uv tool install`, in uv's tool folder
   (`~/.local/bin` unless you changed it);
4. runs `cs install`, described below.

If that folder is not on your `PATH`, the script says so: run `uv tool update-shell` and open
a new terminal. A tmux built from git (`tmux -V` says `master`) is accepted as new enough.

To do the same by hand:

```sh
uv tool install git+https://github.com/emanuel36/claude-sessions
cs install
```

`cs install` prints what it changed, for example:

```text
hooks: added to 11 events
status line: now cs statusline (yours is kept and still shown)
```

If `settings.json` already existed, the first install ends with a line starting with
`backup:`, the path of the copy it made of your settings.

Claude Code sessions started from now on are tracked. Open the tabs with `cst`; the
[usage guide](usage.md) takes it from there.

## What `cs install` changes

`cs install` edits Claude Code's `settings.json`, in `~/.claude`, or in `$CLAUDE_CONFIG_DIR`
when that is set. If `settings.json` is a symbolic link, the file it points to is edited and
the link stays. Nothing else in the file changes, apart from its layout: it is written back
with two-space indentation.

| Where | Change |
|---|---|
| `hooks` in `settings.json` | A hook that runs `cs hook` (with a 10-second timeout) is added to 11 events: SessionStart, SessionEnd, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest, Notification, Stop, StopFailure and SubagentStop. Hooks you already had stay, and run as before |
| `statusLine` in `settings.json` | Set to `cs statusline`, which records the session's cost, time, model, lines and usage limits. If you had a status line of your own, `cs statusline` runs it with the same input and prints its output, so it looks the same; its other settings, such as `padding`, are kept. Without one, it prints the model and the cost |
| `settings.json.before-claude-sessions` | A copy of `settings.json` as it was, next to the real file. It is made only if there is none yet, and never removed |
| `statusline.json` in `~/.local/state/claude-sessions` | Your own status line setting, kept for `cs statusline` to run and for `cs uninstall` to put back. Only written if you had one |

The settings point at `cs` by its absolute path, because Claude Code hooks may run without
`~/.local/bin` on their `PATH`. Running `cs install` again is safe: it says
`already installed`, or, if `cs` has moved (after reinstalling it somewhere else, say), it
points the existing entries at the new path instead of adding new ones.

If `settings.json` is not valid JSON, or its `hooks` or `statusLine` do not have the shape
Claude Code expects, `cs install` says what is wrong and changes nothing.

`~/.local/state` above is `$XDG_STATE_HOME` when that is set.

## Ghostty

Ghostty uses Alt+1…9 to switch its own tabs and Alt+← → to move between its splits, so in an
ordinary Ghostty window those keys never reach tmux. `cst --ghostty` opens a separate Ghostty
window with those keys unbound, so they work as described in the [usage guide](usage.md):

```sh
cst --ghostty
```

It relies on a Ghostty option that exists only on Linux. Your other Ghostty windows keep their
keys. In other terminals, check that Alt sends an Escape prefix (most Linux terminals do).

## Update

Run the install command again, or by hand:

```sh
uv tool upgrade claude-sessions
cs install
```

Running `cs install` again is only needed if `cs` moved, and does no harm otherwise. A tmux
server that is already running keeps its old configuration until it stops: once you have
closed your sessions, quit it with `tmux -L claude kill-server` and open `cst` again.

## Uninstall

```sh
cs uninstall
uv tool uninstall claude-sessions
```

`cs uninstall` takes the `cs` hooks out of `settings.json` and puts your own status line
back, or removes the `statusLine` setting if you had none. A hook list that held only `cs`
goes with it. Your other hooks and settings are not touched. It also deletes
`statusline.json`.

What stays, for you to delete if you want: the backup `settings.json.before-claude-sessions`,
the day history in `~/.local/state/claude-sessions/journal`, and an `install.json` in
`~/.local/state/claude-sessions` that older versions wrote.

## Files

| File | Where | Contents |
|---|---|---|
| Session records and `limits.json` | `$XDG_RUNTIME_DIR/claude-sessions` | One record per running session, and your latest usage limits. Gone after a reboot |
| `hook-errors.log` | `$XDG_RUNTIME_DIR/claude-sessions` | Errors from `cs hook` and `cs statusline` |
| Day history | `~/.local/state/claude-sessions/journal` | Status changes and costs, one file per day, kept for a week |
| `statusline.json` | `~/.local/state/claude-sessions` | Written by `cs install`, see above |

`$XDG_RUNTIME_DIR` is usually `/run/user/<your user id>`; without it, the records go to
`claude-sessions-<your user id>` in the system's temporary folder, and `cs` refuses that folder
if another user owns it or can write to it. `CLAUDE_SESSIONS_DIR` and `CLAUDE_SESSIONS_STATE`
override the two folders.

## Troubleshooting

**A tab never changes colour.** `cs hook` never stops Claude Code: when it fails, it writes
the error to `hook-errors.log` in `$XDG_RUNTIME_DIR/claude-sessions` and lets the session go
on. Look there first. Check also that the session runs in a tab of the `cst` tmux server, and
that `cs install` ran (it says `already installed`).

**The cost, time, model and lines columns are empty.** These numbers reach `cs` only through
Claude Code's status line, so they appear on the session's first status line update, after
its first response. If something changed `statusLine` in `settings.json` after
`cs install`, they stay empty: run `cs install` again.

**The 5h and 7d limit bars are missing.** Claude Code reports usage limits only to Claude
Pro, Max and Ultra subscribers, and only after the session's first response. With an API key,
or before that first response, the bars stay hidden.

**Your own status line went blank.** `cs statusline` waits up to 5 seconds for your status
line command, as Claude Code would cancel it at its next update anyway; a slower one shows
nothing for that update.

**The Alt+c popup closes at once.** It needs `fzf` on the `PATH` of the tmux server. Alt+g
needs `tig` in the same way, and in a folder that is not a git repository it says so instead
of opening.
