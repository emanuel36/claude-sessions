# claude-sessions v1: design

## Intent

claude-sessions is a tmux setup for running several Claude Code sessions at once. Each tmux
tab is one session and shows its status in the tab bar. Tab 1 holds a panel with every
session and a summary of the day.

A working prototype exists. v1 is the first public version and is built from the prototype's
final behaviour. It brings a clean structure and a panel redone in Textual, with per-session
usage, a daily summary and a timeline. The prototype is in the owner's dotfiles, outside this
repository. Its problems are the reason for the structure below:

- **Two artificial halves.** The code is split into two halves that each patch `sys.path`.
- **Duplicated helpers.** There are two tmux helpers, and Claude Code's history is parsed
  twice.
- **One large panel file.** The panel is a single curses file that mixes data, drawing and
  keys.
- **Hooks registered by hand.** There is no install step.

After v1, most new features will go into the panel: more information, new layouts, charts.
The structure must make those changes touch only the panel.

## Goals

- **Own repository:** `github.com/emanuel36/claude-sessions`, Apache-2.0, branch `main`.
- **Installable by other people:** with `uv tool install` from git, then one `cs install`.
- **One package, clear modules:** each concern lives in one place.
- **Panel in Textual:**
  - layout A, below;
  - per-session columns: cost, duration, model and lines +/−;
  - a summary with usage limits, cost today, counts by status and time spent waiting for the
    user;
  - a timeline of today.
- **Same behaviour elsewhere:** the tab bar, keys, status rules and hook logic work exactly as
  the prototype does.
- **Clean history:** a short series of Conventional Commits, each one green, that reads as the
  project being built in a sensible order.
- **User documentation:** a README, a usage guide, an installation manual and real
  screenshots.

## Non-goals

- **Linux only.** macOS is not supported in v1. Process liveness uses `/proc`, and
  `cst --ghostty` uses a Ghostty option that exists only on Linux. The install manual says
  so.
- **No tab bar changes.** No new tab bar features or restyle.
- **No status logic changes.** No new statuses or changes to the status rules.
- **No session data the owner did not pick.** Context %, effort, current activity, todos,
  agents and git stay out of the panel.
- **No cost-per-hour chart.**
- **The Alt+c picker stays in fzf.**
- **Not packaged for PyPI.**

## Anti-goals

- **A slow hook.** The hook runs on every tool call. Its import path must not load Textual or
  anything heavy.
- **A broken session.** A failing hook, status line or tmux call must never break or block
  Claude Code.
- **Clobbering the user's settings.** `cs install` must not lose any existing hooks or the
  user's own status line, and `cs uninstall` must restore what was there.
  `~/.claude/settings.json` may be a symlink; it is edited in place and the link is kept.
- **Features that come and go in the history.** Do not add commits that introduce something
  and later remove it. The prototype's dead ends (a separate animation process, earlier
  record formats) never appear.
- **Private details in the repository.** No personal paths, employer project names or ticket
  keys in code, docs, tests or screenshots.

## Constraints

- **Languages:**
  - Python ≥ 3.11;
  - Textual is the only runtime dependency;
  - bash only where tmux or fzf needs a shell.
- **Code rules:**
  - 100-character lines;
  - absolute imports;
  - Google-style docstrings on non-trivial public APIs;
  - ruff (`E F W I B`).
- **Commits:**
  - Conventional Commits in the imperative mood;
  - subject ≤ 50 characters, body lines ≤ 72;
  - the body is one-line bullets of what changed and why;
  - a `Co-Authored-By: Claude` trailer.
- **Runtime requirements:**
  - Linux;
  - tmux (the minimum version is the builder's to find and document);
  - fzf for Alt+c;
  - tig, optional, for Alt+g;
  - Ghostty, optional, for `cst --ghostty`.
- **Documentation:** in English, written for users. The README says what the tool is, how it
  is organised and how to use it, using a tree, tables or lists. It has no code; commands
  live in the install manual. It uses remote links only.

## How it works (decided)

### Pieces

| Piece | Responsibility |
|---|---|
| `state` | Pure state machine, from a hook event to the session record. Carried over unchanged from the prototype |
| `store` | Live session records in `$XDG_RUNTIME_DIR/claude-sessions`. Handles locking, atomic writes, the dead-process sweep and Esc-interrupt settling |
| `usage` | Reads the status line input Claude Code sends: per-session cost, duration, model and lines +/−, plus the account's 5-hour and 7-day usage limits with their reset times |
| `journal` | Day history in `~/.local/state/claude-sessions/`: status changes and cost snapshots. It lets the summary and timeline survive sessions closing and the panel restarting |
| `claude_files` | Everything read from Claude Code's own files: `history.jsonl` (recent sessions, recent folders) and transcript titles. There is one parser, used by both the panel and the picker |
| `tmux` | The only module that runs tmux. It pushes status to a pane, names windows, tells whether a pane is on screen, lists and selects windows, and finds the next pending one |
| `cli` | The `cs` command |
| `panel` | The Textual app for tab 1. It is split into a view model, built from the modules above with no Textual import and unit-tested, and the widgets that draw it |
| tmux config, picker script | Shipped inside the package as data files |

Module and file names are the builder's call; the split is decided.

### Commands

`uv tool install` installs `cs` and `cst`.

| Command | Does |
|---|---|
| `cs hook` | Called by Claude Code on each hook event |
| `cs statusline` | Called by Claude Code as its status line |
| `cs seen` | Called by tmux when you look at a session |
| `cs next` | Alt+n |
| `cs new` | Alt+c |
| `cs panel` | Tab 1 |
| `cs install` | Registers the hooks and the status line in `~/.claude/settings.json` |
| `cs uninstall` | Removes them and restores what was there |
| `cst` | Opens or attaches to the tmux session |
| `cst --ghostty` | Does the same in a new Ghostty window |

The hooks cover 11 events: SessionStart, SessionEnd, UserPromptSubmit, PreToolUse,
PostToolUse, PostToolUseFailure, PermissionRequest, Notification, Stop, StopFailure and
SubagentStop. The tmux config calls `cs …` rather than script paths. `cs install` writes the
absolute path of `cs` into the settings, because Claude Code hooks may not have
`~/.local/bin` on `PATH`.

### Status line chaining

Cost, duration, model, lines and usage limits reach only the status line command. `cs
install` sets the status line to `cs statusline`.

If the user already had a status line command, it is kept. `cs statusline` runs it with the
same input and prints its output, so the user's status line looks the same. Without one, `cs
statusline` prints a short default line.

Before printing, `cs statusline` records the usage numbers on the session's record. Recording
never delays or breaks the output.

### Panel (layout A)

```
  Claude sessions   ! 1 permission   ? 1 question   ● 1 working   ✓ 1 finished   1 idle

  5h limit  ▰▰▰▰▰▰▰▰▰▰▰▰▰▱▱▱▱▱▱▱  64%  resets in 1h52m      today  $11.40  ($8.67 in open sessions)
  7d limit  ▰▰▰▰▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱  31%  resets Fri 09:00      waited for you today  47m

    Open                                      cost   time  model        lines
  ! Restructure the config loader            $4.12  1h40m  Opus 5.5     +820 −310   tab 2  permission 3m
  ? Fix request timeout in the API client    $1.30    35m  Sonnet 5.5    +44 −12    tab 3  question 12m
  ● Migrate usage dashboards to v2           $2.75    52m  Opus 5.5     +210 −95    tab 4  working
  ✓ Write the release notes                  $0.08     4m  Haiku 5.5      +0 −0     tab 5  finished
    Explore tmux popup styles                $0.42    18m  Opus 5.5       +6 −2     tab 6

  today                   08:00       10:00       12:00       14:00       16:00
  Restructure the conf…   ▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇
  Fix request timeout…    ▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇
  …                       (each cell coloured by the session's status at that time)
                          ■ working  ■ permission  ■ question  ■ finished, not seen  ■ idle

  Recent
    Review the retry PR                                  sync-service                 14:05
    Weekly notes                                         notes                yesterday 18:12

  enter open    n new session    / search
```

- **Header:** the session count per status, in the status colours.
- **Usage:**
  - the 5h and 7d limits as bars that turn amber at 70% and red at 85%, with the time to
    reset;
  - cost today, total and in open sessions;
  - "waited for you today", the total time sessions spent in the permission and question
    statuses.
- **Open:** one row per open session in this tmux server, ordered by tab. The columns are:
  - status mark: `!` red, `?` amber, `●` blue blinking in step with the tab bar, `✓` green,
    none for idle;
  - title;
  - cost;
  - duration;
  - model;
  - lines (+ green, − red);
  - on the right, `tab N` plus the status word and waiting time, in the status colour.

  The selected row has a grey background. When the panel is narrow, the title shrinks first,
  then data columns drop from the right. A session with no status line input yet shows empty
  cells.
- **Today:** one bar per open session, from the earliest event of the day to now, on a shared
  hour axis, coloured by status, with a legend.
- **Recent:** sessions from Claude Code's history that are not open, with title, folder and
  when.
- **Keys line:** Enter goes to the tab or resumes a closed session in a new tab; n starts a
  new session; / searches titles and folders and Esc clears; ↑↓ and j/k move; a mouse click
  only selects.

The panel uses the terminal background, the tab bar's status colours and grey for secondary
text, with no boxes around sections. It refreshes once a second and Ctrl+C does not kill it.
tmux restarts the panel if it exits.

### Cost today and waiting time

**Cost today** is the sum of each session's latest known cost, over the sessions that had a
hook event today. A session that crosses midnight counts in full on both days; this is
accepted. **Waiting time** comes from the journal's status changes.

## Commits

The repository lives at `~/Documents/claude-sessions`. Each commit passes tests and ruff and
leaves the tool working. The builder may merge or split neighbours, but must keep that rule.

1. `chore: scaffold the project`: pyproject, ruff, license, gitignore, the package skeleton,
   this design and the plan.
2. `feat(state): track session status from hooks`
3. `feat(store): keep live session records`
4. `feat(cli): add the cs hook command`
5. `feat(tmux): show each session's status on its tab`: tmux config, `cst`, pushing status,
   naming windows, `cs seen`. Tab 1 is a plain shell until the next commit.
6. `feat(panel): list open and recent sessions`: the Textual app with the header, open
   sessions (status, title, tab), recent sessions, keys and search. Tab 1 runs it.
7. `feat(tmux): jump to the next session needing you`: Alt+n.
8. `feat(tmux): start sessions from recent folders`: Alt+c and the panel's n key.
9. `feat(tmux): show commit history in a popup`: Alt+g.
10. `feat(usage): record usage from the status line`: `cs statusline`, chaining, and the
    panel's cost, duration, model and lines columns and limit bars.
11. `feat(journal): keep a history of the day`: the journal, plus the panel's cost today and
    waiting time.
12. `feat(panel): draw today's timeline`
13. `feat(cli): register hooks with cs install`: install and uninstall.
14. `docs: add usage guide and install manual`: README, guides and screenshots.

These docs do not mention the owner's dotfiles. Migrating them is tracked outside this
repository.

## Documentation

| File | Contents |
|---|---|
| `README.md` | What it is, a screenshot of the tab bar and the panel, statuses, how it is organised, links to the guides |
| `docs/install.md` | Requirements and versions (Linux); `uv tool install`; `cs install` and exactly what it changes in `~/.claude/settings.json`; Ghostty notes; updating; `cs uninstall`; troubleshooting (hook error log, empty usage columns) |
| `docs/usage.md` | Opening it; statuses; every key; the panel section by section; where each number comes from |
| `docs/screenshots/` | The tab bar showing every status, the panel, the Alt+c picker and the Alt+g popup |

Screenshots show the real tool rendering, not mockups. The sessions in them have neutral
names. The capture method is the builder's call.

## Left to the builder

- **Code layout:** module and file names, and the internal APIs between modules.
- **Panel implementation:** the shape of the panel model, and the Textual widgets and CSS,
  within the layout above.
- **Journal:** its file format and retention. Today must be kept; pruning older days is fine.
- **Status line JSON:** the exact field names, checked against Claude Code's documentation.
- **Minimum tmux version and screenshot capture.**
- **Tests:**
  - the prototype's tests carry over, adapted to the new modules;
  - new logic (usage parsing, journal sums, the panel model, install merging) gets tests;
  - Textual rendering is covered with Textual's test pilot where that is cheap.

## Calls made without the owner

- **Status line chaining:** the user's status line keeps working, called by `cs statusline`.
- **Hook path:** `cs install` writes the absolute path of `cs` into the hooks.
- **Commit order:** the panel commit comes right after the tab bar commit, so tab 1 is only
  briefly a plain shell.
- **Public docs:** the design and plan in the repository omit the owner's dotfiles and local
  paths.
- **Cost today:** the cross-midnight double count is accepted.
- **Mockup:** the layout reference is the sketch above, not the preview file.
