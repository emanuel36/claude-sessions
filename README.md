# claude-sessions

Run several Claude Code sessions in tmux and see at a glance which one needs you. Each session
has its own tmux window. `cst` opens on a landing page, and once a session starts, every
session has a 30-column sidebar on its left with all the sessions, their status, your quota
and today's spend.

- **Status at a glance:** permission, question, finished, working, shell or idle, with how
  long a session has waited for you or on a command.
- **One place for it all:** the sidebar looks the same in every session, so you never lose
  the list.
- **Keeps your setup:** `cs install` adds hooks and keeps your own status line; `cs uninstall`
  puts everything back.

## The landing page

`cst` shows it while no session is open. It closes when the first session starts and does not
come back.

- **Recent sessions:** the latest ones, with project, first prompt, cost and when they were
  used. Enter resumes the selected one and `/` searches the list.
- **New session:** your recent folders with their branch and git state, or another folder
  picked with fzf. `n` starts a session in the selected folder.
- **Usage and spend:** your 5-hour and 7-day quota, spend today and over 7 days, spend per
  project, sessions per day over two weeks, and lines changed this week.

| Keys | Does |
|---|---|
| ↑ / ↓ | Move through recent sessions and folders |
| Enter | Resume the selected session, or start one in the selected folder |
| n | New session |
| / | Search recent sessions |
| q | Quit `cst` |

## The sidebar

It sits on the left of every session, always 30 columns wide, and never takes the keyboard:
what you type goes to Claude.

- **Two lines per session:** the status mark, number, name and waiting time, then the cost
  and how long the session has been open. The current session is highlighted.
- **Stable numbers:** sessions keep the order they were opened in, so a number does not
  change when a status does.
- **Background runs:** while an automatic Claude run is going, such as a start hook's daily
  recap, it shows under the sessions with a magenta `↻`, its folder's name and how long it has
  been running. It leaves the list when it ends and never takes over a session's tab.
- **Quota and spend:** your 5-hour and 7-day usage with the time left until each resets,
  today's spend, and a chart of the last 7 days' spend with the weekday under each day.
- **Mouse:** click a session to go to it.

## Statuses

| Status | Mark | Meaning |
|---|---|---|
| Permission | red `!` | Claude needs your approval to run something |
| Question | amber `?` | Claude asked you something, or has a plan to approve |
| Finished | green `✓` | The turn ended and you have not looked yet |
| Working | blue `●`, blinking | Running tools, or waiting on background agents |
| Shell | cyan `⧗` | The turn ended and a shell command, such as a build, still runs in the background |
| Idle | grey | Seen, or interrupted with Esc |

## Keys

Every shortcut is Alt plus one key, from any session.

| Keys | Does |
|---|---|
| Alt+↑ / Alt+↓ | Previous / next session, wrapping around |
| Alt+1 … Alt+5 | The session with that number |
| Alt+n | New session: pick a folder, then start a new session or resume an earlier one |
| Alt+c | Close the current session, after a confirmation |
| Alt+r | Rename the current session |

Alt+← and Alt+→ are left to Ghostty, to move to a split beside `cst`, so you can keep a plain
terminal next to your sessions.

## Get started

On Linux with tmux 3.3 or newer, fzf, uv and Claude Code, the
[install script](https://github.com/emanuel36/claude-sessions/blob/main/install.sh) sets
everything up and registers it with Claude Code. Then open it with `cst`.

- **What it changes:** a hook on 11 Claude Code events and the status line setting in
  `settings.json`. Your own hooks and status line keep working, and a backup of your settings
  is kept beside them.
- **Ghostty:** `cst --ghostty` opens a window that leaves the Alt keys to tmux.
- **Update:** run the install script again. A running tmux server keeps its old configuration
  until you quit it and open `cst` again.
- **Uninstall:** `cs uninstall` restores your settings, then remove the package with uv.

## Troubleshooting

| Symptom | Cause |
|---|---|
| A session never changes colour | The session is not in a `cst` window, or the hooks are missing: run `cs install` and check `hook-errors.log` in the runtime folder |
| Cost or time open is empty | The status line has not reported yet, or another tool replaced it: run `cs install` again |
| Quota is missing | Claude Code reports usage limits only to subscribers, after the first response |
| The Alt+n popup closes at once | fzf is not on the tmux server's `PATH` |

## How it is organised

```
claude-sessions/
├── src/claude_sessions/
│   ├── cli.py       the cs command
│   ├── core/        session status, live records, history of costs
│   ├── claude/      Claude Code side: the hook, the status line, its files, cs install
│   ├── term/        tmux side: the cst launcher, tmux config, new-session picker
│   └── ui/          the landing page and the sidebar, and what they show
├── install.sh       the one-command install
└── tests/
```

## License

[Apache License 2.0](https://github.com/emanuel36/claude-sessions/blob/main/LICENSE)
