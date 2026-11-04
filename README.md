# claude-sessions

Run several Claude Code sessions in tmux and see at a glance which one needs you. Each session
has its own tmux window. `cst` opens on a landing page, and once a session starts, every
session has a 30-column sidebar on its left with all the sessions, their status, your quota
and today's spend.

<p>
  <img src="docs/images/sidebar.svg" alt="A session with the sidebar on its left" width="49%">
  <img src="docs/images/landing.svg" alt="The landing page" width="49%">
</p>

- **Status at a glance:** permission, question, finished, working, shell or idle, with how
  long a session has waited for you or on a command.
- **One place for it all:** the sidebar looks the same in every session, so you never lose
  the list.
- **Keeps your setup:** `cs install` adds hooks and keeps your own status line; `cs uninstall`
  puts everything back.

## The landing page

`cst` opens on it, and you come back to it when you close the last session. Alt+↑ / Alt+↓
step over it.

- **Folders:** on the left, as wide as the terminal allows so session titles show whole: the
  folders you ran Claude in, last used first, each with its number of sessions and its spend
  over 7 days. → opens a folder's last 3 sessions and ← closes them.
- **Statistics:** on the right, as in the sidebar: today's spend, activity and waiting, your
  5-hour and 7-day quota, and the last 7 days' spend.

| Keys | Does |
|---|---|
| ↑ / ↓ | Move through the folders and their sessions |
| → / ← | Show or hide a folder's last sessions |
| Enter | Start a session in the folder, or resume the selected session |
| / | Search the folders |
| q | Quit `cst` |

## The sidebar

It sits on the left of every session, always 30 columns wide, and never takes the keyboard:
what you type goes to Claude.

- **Grouped by what you must do:** sessions sit under `NEEDS YOU`, `RUNNING`, `DONE` and
  `IDLE`, each with its own colour. Inside a group they keep the order they were opened in.
- **Two lines per session:** the name and waiting time, then what it waits on, the cost, the
  time Claude spent answering and the model. Both lines take the status's colour as a
  background, and a working session's breathes. A thin rule separates sessions of a group,
  and the current one is a little lighter.
- **Background runs:** while an automatic Claude run is going, such as a start hook's daily
  recap, it shows under the sessions with a magenta `↻`, its folder's name and how long it has
  been running. It leaves the list when it ends and never takes over a session's tab.
- **Statistics at the bottom:** TODAY (spend, activity by hour, lines changed, time waiting
  on Claude and sessions), USAGE (5-hour and 7-day quota with when each resets) and SPEND 7D
  (a column per day). TODAY steps aside when the sessions need its room.
- **Mouse:** click a session to go to it.

## Statuses

| Status | Colour | Meaning |
|---|---|---|
| Permission | red | Claude needs your approval to run something |
| Question | amber | Claude asked you something, or has a plan to approve |
| Finished | green | The turn ended and you have not looked yet |
| Working | blue, breathing | Running tools, or waiting on background agents |
| Shell | cyan | The turn ended and a shell command, such as a build, still runs in the background |
| Idle | none | Seen, or interrupted with Esc |

## Keys

Every shortcut is Alt plus one key, from any session.

| Keys | Does |
|---|---|
| Alt+↑ / Alt+↓ | Previous / next session, wrapping around, never the landing page |
| Alt+n | New session: folders by last use, each opening its last 3 sessions |
| Alt+c | Close the current session, after a confirmation |
| Alt+r | Rename the current session |

Alt+n opens a panel over the session with the landing page's folder list. → opens a folder's last 3 sessions and ← closes them; Enter on a folder starts a new
session there, Enter on a session resumes it. Typing searches, and a typed path that exists
is offered first. Closing a session moves to the next one in the list.

Alt+← and Alt+→ are left to Ghostty, to move to a split beside `cst`, so you can keep a plain
terminal next to your sessions.

## Get started

On Linux with tmux 3.3 or newer, uv and Claude Code, the
[install script](https://github.com/emanuel36/claude-sessions/blob/main/install.sh) sets
everything up and registers it with Claude Code. Then open it with `cst`.

- **What it changes:** a hook on 11 Claude Code events and the status line setting in
  `settings.json`. Your own hooks and status line keep working, and a backup of your settings
  is kept beside them.
- **Ghostty:** `cst` opens in the terminal you run it from. Ghostty keeps Alt+↑ / Alt+↓ for
  its splits unless its config unbinds them; `cst --ghostty` opens a window that leaves them
  to tmux.
- **Update:** run the install script again. A running tmux server keeps its old configuration
  until you quit it and open `cst` again.
- **Uninstall:** `cs uninstall` restores your settings, then remove the package with uv.

## Troubleshooting

| Symptom | Cause |
|---|---|
| A session never changes colour | The session is not in a `cst` window, or the hooks are missing: run `cs install` and check `hook-errors.log` in the runtime folder |
| Cost or time open is empty | The status line has not reported yet, or another tool replaced it: run `cs install` again |
| Quota is missing | Claude Code reports usage limits only to subscribers, after the first response |

## How it is organised

```
claude-sessions/
├── src/claude_sessions/
│   ├── cli.py       the cs command
│   ├── core/        session status, live records, history of costs
│   ├── claude/      Claude Code side: the hook, the status line, its files, cs install
│   ├── term/        tmux side: the cst launcher and the tmux config
│   └── ui/          the landing page, the sidebar and the new-session panel
├── install.sh       the one-command install
└── tests/
```

## License

[Apache License 2.0](https://github.com/emanuel36/claude-sessions/blob/main/LICENSE)
