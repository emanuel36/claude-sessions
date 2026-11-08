# claude-sessions

**See at a glance which Claude Code session is waiting for you.**

claude-sessions is a small tool, built on open-source software, for running several Claude Code
sessions side by side. Its goal is to cut development latency: the time a session sits idle
waiting for a person to approve a tool, answer a question or read a finished turn.

<p align="center">
  <img src="docs/images/sidebar.svg" alt="A session with the sidebar on its left">
</p>

- **Status at a glance:** every session shows whether it needs approval, has a question, has
  finished, is working or is idle, and how long it has waited.
- **Most urgent first:** sessions that need you sit at the top of the sidebar; Alt+↑ / Alt+↓ go
  to them in that order.
- **Cost and quota in view:** today's spend, the 5-hour and 7-day quota and the week's spend sit
  under the sessions.
- **Keeps your setup:** your own hooks and status line keep working, and uninstalling puts
  everything back.

## Built on

| Piece | Role |
|---|---|
| [tmux](https://github.com/tmux/tmux) | One window per session, with the sidebar on its left |
| [Claude Code hooks](https://docs.claude.com/en/docs/claude-code/hooks) and status line | Each session's status, cost and model |
| [Textual](https://github.com/Textualize/textual) | The sidebar, the landing page and the new-session panel |

## The landing page

Where `cst` opens: the folders you ran Claude in, their recent sessions to resume, and the day's
and week's statistics.

<p align="center">
  <img src="docs/images/landing.svg" alt="The landing page">
</p>

## Statuses

| Status | Colour | Meaning |
|---|---|---|
| Permission | red | Claude needs your approval to run something |
| Question | amber | Claude asked something, or has a plan to approve |
| Finished | green | The turn ended and you have not looked yet |
| Working | orange spinner | Running tools or background agents |
| Shell | lavender | The turn ended; a shell command still runs |
| Idle | grey | Seen, or interrupted with Esc |

## Keys

| Keys | Does |
|---|---|
| Alt+↑ / Alt+↓ | Previous / next session, most urgent first |
| Alt+n | New session, or resume one, from the folders you used |
| Alt+r | Rename the current session |
| Alt+c | Close the current session, after a confirmation |

On the landing page: ↑ ↓ move, → / ← show or hide a folder's sessions, Enter starts or resumes,
/ searches, q quits.

## Get started

Needs Linux, tmux 3.3 or newer and Claude Code. Install with one command:

```sh
curl -fsSL https://raw.githubusercontent.com/emanuel36/claude-sessions/main/install.sh | sh
```

The [install script](https://github.com/emanuel36/claude-sessions/blob/main/install.sh):

1. Checks that the system is Linux and that tmux is 3.3 or newer.
2. Installs [uv](https://github.com/astral-sh/uv) if it is missing; its installer may add
   `~/.local/bin` to your shell's startup files.
3. Installs claude-sessions from this repository with uv, which gives the `cs` and `cst`
   commands.
4. Registers claude-sessions with Claude Code: a hook on 11 events and the status line in
   `settings.json`. Your own status line keeps running, and a backup of your settings is kept
   beside them.
5. Tells you if uv's command folder is not on your PATH yet.

Then open it with `cst`.

- **Update:** run the same command again, then quit `cst` and open it again.
- **Uninstall:** `cs uninstall` restores your settings; then remove the package with uv.

## Troubleshooting

| Symptom | Cause |
|---|---|
| A session never changes colour | It is not in a `cst` window, or the hooks are missing: run `cs install` |
| Cost is empty | The status line has not reported yet, or another tool replaced it |
| Quota is missing | Claude Code reports it only to subscribers, after the first response |

## How it is organised

```
src/claude_sessions/
├── cli.py     the cs command
├── core/      session status, live records, cost history
├── claude/    Claude Code side: hook, status line, install
├── term/      tmux side: the cst launcher and its config
└── ui/        landing page, sidebar, new-session panel
```

## License

[Apache License 2.0](https://github.com/emanuel36/claude-sessions/blob/main/LICENSE)
