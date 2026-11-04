# Sidebar, landing page and shortcuts: design

## Intent

Today tab 1 holds a panel that is always available, and a tab bar at the bottom shows every
session's status. The owner wants a different shape:

- **Landing page:** a page shown only when `cst` opens, with the session history, some
  statistics and a way to open a new session. It goes away once the first session opens and
  never comes back.
- **Left sidebar:** a vertical bar on the left of every session that replaces the panel and
  the bottom bar. It shows each open session's status and summary, plus the statistics. The
  owner rarely has more than 5 sessions open, so a vertical list has room to spare. A Claude
  mascot fills the empty space.
- **Vertical navigation:** the list is vertical, so up and down move between sessions.
- **One place for status:** the bottom bar goes away so the status is not shown twice.
- **Shortcuts that never collide:** the owner works with claude-sessions side by side with a
  plain terminal, both in Ghostty splits, to run and watch other things. Today Alt+←/→ means
  both "next tab" here and "go to the split beside" in Ghostty. The new shortcuts must not
  collide with Ghostty, Claude Code or GNOME.
- **No tig:** tig moves to the terminal beside, so its support is removed.

The owner chose every visual detail below through interactive previews. The result must look
identical to them: same layout, same text, same marks, same colours, same order.

## Goals

- **Landing page** at start, as specified below.
- **Left sidebar** in every session, as specified below.
- **Shortcut scheme A**, as specified below.
- **No tmux status bar**, no tab 1 panel, and no tig.
- **Closing the last session closes everything.**

## Non-goals

- **No built-in terminal.** The terminal beside is a Ghostty split the owner opens. This
  project builds nothing for it and only leaves it the Alt+←/→ keys.
- **No way back to the landing page.** Once a session is open, the history and the landing
  statistics are not reachable again.
- **No "next session that needs you" shortcut.** It is removed (today's Alt+n, `cs next`).
- **No status logic changes.** Statuses, their rules and the hook stay as they are, Shell
  included.
- **Not more than about 5 sessions.** The sidebar does not need scrolling. With more
  sessions it may cut the list, but it must not break.

## Anti-goals

- **A shortcut that steals a key from Claude Code or Ghostty.** Claude Code's keys (plain
  arrows, Ctrl+letters, Alt+o/p/t/w, Shift+↑/↓, Ctrl+↑/↓, Alt+b/f/d) must reach Claude.
  Ghostty's split keys (Alt+←/→, Alt+Shift+arrows, Ctrl+Shift+arrows, Alt+z/=/q) must reach
  Ghostty.
- **A sidebar that takes the keyboard.** Typing always goes to Claude. The sidebar never
  takes focus from the keyboard.
- **A slower hook.** The hook's import path must stay light, as today.
- **A sidebar that differs from one session to the next.** It must look the same, in the
  same place and at the same width, in every session.
- **Drift from the previews.** Text, marks, colours and order below are the spec, not
  examples.

## Constraints

- **Platform:** Linux, tmux ≥ 3.3, Ghostty, GNOME, as today.
- **Stack:** Python package as today. Textual is already used by the panel and may be used for
  the sidebar and the landing page.
- **Hook:** it stays quick and never breaks Claude Code.
- **Code conventions:** 100-character lines, absolute imports, Google-style docstrings on
  non-trivial public APIs.
- **Language of the screens:** English, exactly as written in the layouts below
  (`Recent sessions`, `today`, `yesterday`, weekday abbreviations such as `Wed` and `Fri`,
  dates as `25 Oct`, thousands with a comma). Weekday and month names are fixed English
  abbreviations, not the locale's. The README and the code stay in English.

## The landing page

### When it shows

- **On start:** `cst` shows the landing page full screen, with no sidebar, when the tmux
  server has no session open.
- **Gone after the first session:** once a session opens, from a recent session or from a
  folder, the landing page closes and is never shown again in that server's life.
- **Reopening:** if `cst` is run again while sessions are alive, it goes straight to them,
  with no landing page. (Designer's call; the owner did not object.)

### Layout: two columns

The left column is for getting started, the right column for statistics, with the footer
below both. Exact text, at the preview's sample data:

```
 ▐▛███▜▌
▝▜█████▛▘
  ▘▘ ▝▝    claude-sessions
Recent sessions  / search
› api              refatora o cliente HTTP para … $5.42  2h
  vehicle.k265     comunicação com tablet (VP-56… $8.10  5h
  web              corrige o fluxo de login com … $3.10  yesterday
  claude-sessions  estado para comando shell em … $4.77  yesterday
  docs             atualiza o guia de instalação  $0.42  Wed
New session in…  n
  ~/workspace/api              main       3 changed
  ~/workspace/vehicle.k265     VP-562     clean
  ~/workspace/web              feat/login 1 changed
  ~/Documents/claude-sessions  main       clean
  other folder…
```

Right column, four columns of gap to the right of the left one:

```
Usage
5h   █████████░░░░░░░ 58%  resets 16:40
7d   █████░░░░░░░░░░░ 31%  resets Fri 09:00
today $9.59   7 days $84.30
Spend 7d by project
api              ██████████████ $31.20
vehicle.k265     ██████████░░░░ $22.75
web              █████░░░░░░░░░ $12.04
claude-sessions  █████░░░░░░░░░ $10.90
docs             █░░░░░░░░░░░░░ $2.10
Sessions per day (14 days)
▄ ▆ ▅     ▇ █ ▆ ▅ ▇   ▃ ▆ ▅
20 Oct                today
Week
+4,210 -1,388 lines in 23 sessions
```

Footer, below both columns, after a blank line:

```
↑↓ choose · Enter resume · n new session · / search · q quit
```

Section headings (`Recent sessions`, `New session in…`, `Usage`, `Spend 7d by project`,
`Sessions per day (14 days)`, `Week`) have a blank line above them.

### Landing rules

- **Mascote:** the three-line pixel mascot, top left, with the title `claude-sessions` on its
  third line, two columns after it.
- **Recent sessions:** newest first. Each row: a selection mark (`›` on the selected row,
  space otherwise), the project folder's name padded to 16, the first prompt cut to 30 with
  `…`, the session's cost padded to 6, and when it was last used (`2h`, `yesterday`, weekday
  `Wed`). Enter resumes the selected session. `/` searches the list, as the panel does today.
- **New session:** recent folders, with the branch padded to 10 and the git state (`clean`,
  `N changed`), then `other folder…`, which opens today's fzf folder picker. `n` starts a
  new session. Builder's call: `n` starts it in the selected folder and `other folder…` asks
  for another.
- **Keys:** ↑/↓ move the selection through both lists, Enter acts on it, `q` quits `cst`.
- **Usage:** the 5h and 7d bars have 16 cells, with the percentage and the reset time. Then
  today's spend and the last 7 days' spend.
- **Spend per project:** the last 7 days, largest first, bars of 14 cells scaled to the
  largest project, then the amount.
- **Sessions per day:** the last 14 days, oldest on the left, one block character per day
  (`▁`…`█`, blank for none) separated by spaces, with the first day's date and `today` under
  the ends.
- **Week:** lines added and removed in the last 7 days and the number of sessions.
- **Data:** the journal keeps 7 days today. The 14-day activity needs either a longer
  journal or another source, such as Claude Code's history. Builder's call.

## The sidebar

### Where it shows

- **Every session:** a sidebar on the left of every session window, 30 columns wide,
  including a one-column border on its right. The Claude session fills the rest. (Owner's
  choice, 2026-11-04; it was 38. A smaller font only for the sidebar is not possible: tmux
  draws it and Claude in one terminal with one font.)
- **Always 30 columns:** the width never changes, even when `cst` sits in a narrow Ghostty
  split; Claude gets whatever is left.
- **One entry per window:** a window with teammates (a leader and other Claude panes) is one
  entry, as one tab is today. It shows the most urgent status among its panes and the
  leader's cost and time open. Alt+c closes the whole window. (Designer's call.)
- **Same everywhere:** it shows the same content in every window, with the current session
  highlighted.

### Layout

Exact text, at the sample data of `tests/test_sidebar.py`, with `api` as the current session
(the week cost $0, $2, $4, $6, $8, $1 and $3, today last):

```
          ▐▛███▜▌
         ▝▜█████▛▘
           ▘▘ ▝▝

! 1 web                  3m
    $3.10 · 40m
● 2 vehicle.k265
    $0.90 · 1h12
⧗ 3 api                  6m
    $5.42 · 2h05
✓ 4 docs
    $0.12 · 15m
  5 notes
    $0.05 · 3h00

(empty space up to the bottom)

───────────────────────────
5h  ██████████░░░░░░░░ 58%
    resets in 2h33
7d  ██████░░░░░░░░░░░░ 31%
    resets in 2d 18h
today $9.59

spend 7d             $24.00
                ██
            ██  ██
        ██  ██  ██      ▄▄
    ██  ██  ██  ██  ▄▄  ██
We  Th  Fr  Sa  Su  Mo  Tu
```

### Sidebar rules

- **Padding:** one column on each side of the content, which is 27 columns wide.
- **Mascote:** the three-line pixel mascot, centred, at the top.
- **Each session, two lines:**
  - **Line 1:** status mark, its number, the name, and the waiting time right-aligned at
    column 27. The name takes all the room the rest leaves and is cut with `…` only past
    it. The waiting time shows only for the timed statuses (permission, question, shell)
    and only from one minute on, as today (`3m`, `2h`).
  - **Line 2:** four spaces, then the cost and how long the session has been open (`40m`,
    `1h12`), joined by ` · `. An unknown part is left out. (Owner's choice, 2026-11-04: model,
    branch and context are no longer shown.)
  - **Current session:** both lines on a highlighted background.
- **Order and numbers:** sessions keep the order they were opened in, numbered 1, 2, 3… The
  number is the one Alt+1…5 uses, so it must not change when a status changes. (Builder's
  call: today's panel orders by urgency, which would move numbers around.)
- **Status marks:** as today: `!` permission, `?` question, `✓` finished, `●`/`○` working
  (blinking once a second), `⧗` shell, blank for idle.
- **Statistics, at the bottom:** a separator line across the content; the 5h and 7d bars with
  18 cells, each with the time left until it resets on the next line (`resets in 2h33` under
  a day, `resets in 2d 18h` from a day on); today's spend; a blank line; then the week's
  spend chart: `spend 7d` with the week's total right-aligned, four rows of columns, one
  2-cell column per day of the last 7 with 2 spaces between them, scaled to the most
  expensive day (full blocks, and eighths of a block for a partial top), today's column
  green and the others blue; and the two-letter weekday under each column (`Mo` … `Su`),
  today's bright white and bold, the others faint. No shortcut reminder. (Owner's choice,
  2026-11-04.)
- **Mouse:** a click on a session goes to it. (Builder's call.)
- **Background runs:** a Claude run without a terminal (`claude -p` or the Agent SDK, as a
  start hook's daily recap or trending-repos run, or a script) is not a session. While it runs
  (any status but finished or idle) it is listed under the sessions: a blank line, the heading
  `Background` in grey, then one line per run, oldest first: a magenta `↻`, the run's folder
  name, and how long it has been running right-aligned at column 27 in magenta, from one
  minute on. No number, no click, no Alt keys. With no run, the section is not drawn. In a
  short terminal, runs are dropped before sessions. Such a run never takes the tab of the
  session that started it, even though it inherits its tmux environment. (Owner's choice,
  added 2026-11-04.)

### Colours

Colours follow the terminal palette as the tab bar and panel do today.

| Element | Colour |
|---|---|
| Mascot | orange, close to `#d97757` (xterm colour 173) |
| `!` mark and waiting time | red, bold |
| `?` mark and waiting time | yellow, bold |
| `✓` mark | green |
| `●`/`○` mark | blue, bold, blinking |
| `⧗` mark and waiting time | cyan |
| Idle name and mark | grey (colour 244) |
| Session number | faint grey |
| Current session's name | bright white, bold |
| Other names | default text colour |
| Detail line (line 2) | grey (colour 244) |
| Current session background | dark grey, close to `#262a33` |
| Separator and sidebar border | dark grey (colour 238) |
| Quota bar, filled cells | blue below 70 %, yellow from 70 %, red from 85 % |
| Quota bar, empty cells | faint grey |
| Reset times | faint grey |
| Spend chart: today's column / other days | green / blue |
| Spend chart: today's weekday / other weekdays | bright white, bold / faint grey |
| Background runs: `↻` mark and running time | magenta |
| Spend amounts | bright white, bold |

The landing page uses the same colours. In addition: the selected recent session's mark and
name are bright white and bold, the first prompts are grey, the times are faint grey, the
branches are cyan, the git states are faint grey, the project spend bars are blue, the
sessions-per-day blocks are green, lines added are green and lines removed red, and the
headings, the `/ search` and `n` hints and the footer are grey or faint grey as above.

## Shortcuts: scheme A

| Keys | Does |
|---|---|
| Alt+↑ / Alt+↓ | previous / next session in the sidebar's order, wrapping around |
| Alt+1 … Alt+5 | the session with that number (up to 9 if there are more) |
| Alt+n | new session: today's fzf folder picker in a popup |
| Alt+c | close the current session |
| Alt+r | rename the current session |

- **Alt+←/→ belong to Ghostty**, to go to the split beside. claude-sessions no longer binds
  them.
- **Removed:** Alt+0 (last tab), Alt+←/→ and Alt+,/. (tabs), Alt+g (tig), today's Alt+n
  (next session that needs you) and today's Alt+c (picker, now Alt+n).
- **Closing:** Alt+c first asks `close api? (y/n)`, with the session's name, in any status.
  `y` ends the session's Claude and removes it from the sidebar; any other key cancels. The
  answer must never reach Claude. Where the question shows (in the sidebar or in tmux's
  prompt line) is the builder's call. When it was the last session, `cst` closes entirely.
- **Closing a background session:** a session that runs in Claude Code's daemon and shows in
  a `claude attach` window is ended in the daemon too, not only detached. Background sessions
  count as open sessions for "the last one".
- **Ghostty:** `cst --ghostty` keeps unbinding Alt+1…9 in its own Ghostty instance, stops
  unbinding Alt+←/→, and unbinds Alt+↑/↓, which the owner's Ghostty config maps to "go to the
  split above/below". The owner works with splits side by side, so these are not missed.
  (Builder's call: done in the launcher, so the owner's Ghostty config does not change.)
- **GNOME:** Ctrl+Alt+arrows stay with GNOME's workspace switching; nothing here uses them.

## Removals

- **tmux status bar:** off. What it showed moves to the sidebar: statuses, waiting times,
  the 5h quota and each session's cost. The current mode, the clock and the current
  session's model, branch and context are dropped; the owner did not pick them.
- **Tab 1 panel:** gone, with its restart loop. The landing page takes its place only at
  start.
- **tig:** `cs tig`, the Alt+g binding and its README and install mentions.
- **`cs next`:** no shortcut uses it any more.
- **README:** rewritten for the new shape: landing page, sidebar, shortcuts, statuses.

## Left to the builder

- **How the sidebar runs:** for example a tmux pane on the left of each window running a
  small app, or another approach, as long as it looks and behaves as specified, never takes
  keyboard focus and stays the same in every window.
- **How the landing page closes** when the first session opens, and how `cst` exits after
  the last one closes.
- **Where the 14-day activity and the weekly lines come from.**
- **What the panel code becomes:** reused for the landing page and the sidebar, or replaced.
