# claude-sessions

Run several Claude Code sessions in tmux and see at a glance which one needs you. Each tab is
one session, coloured by its status; tab 1 is a panel with cost, usage limits and a timeline
of the day.

<p align="center">
  <img src="docs/screenshots/panel.svg" width="640"
       alt="The panel in tab 1, with the tab bar below it">
</p>

- **Status on every tab:** permission, question, finished, working or idle, with how long it
  has waited for you.
- **One key to what needs you:** Alt+n jumps to the next waiting session.
- **A panel in tab 1:** cost, time, model and lines per session; 5-hour and 7-day limits; cost
  today; a timeline of the day; recent sessions to resume.
- **Keeps your setup:** `cs install` adds hooks and keeps your own status line; `cs uninstall`
  puts everything back.

## Statuses

| Status | Tab | Meaning |
|---|---|---|
| Permission | red `!` | Claude needs your approval to run something |
| Question | amber `?` | Claude asked you something, or has a plan to approve |
| Finished | green `✓` | The turn ended and you have not looked yet |
| Working | blue `●`, blinking | Running tools or background tasks |
| Idle | grey | Seen, or interrupted with Esc |

## Get started

On Linux with tmux 3.3+ and fzf, one command installs it and registers it with Claude Code:

```sh
curl -fsSL https://raw.githubusercontent.com/emanuel36/claude-sessions/main/install.sh | sh
```

Then open it with `cst`. Details in the [install guide](docs/install.md); keys and the panel
in the [usage guide](docs/usage.md).

## How it is organised

```
claude-sessions/
├── src/claude_sessions/
│   ├── cli.py       the cs command
│   ├── core/        session status, live records, history of the day
│   ├── claude/      Claude Code side: the hook, the status line, its files, cs install
│   ├── term/        tmux side: tab colours, the cst launcher, tmux config, new-session picker
│   └── panel/       tab 1: data/ loads what it shows, view/ draws it, app.py runs it
├── install.sh       the one-command install
├── docs/            install and usage guides, screenshots
├── scripts/         screenshots.py, which redraws the screenshots
└── tests/
```

## License

[Apache License 2.0](https://github.com/emanuel36/claude-sessions/blob/main/LICENSE)
