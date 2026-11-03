# Panel, tab bar and docs redesign

## Intent

The panel in tab 1 and the tmux chrome around it show little that helps the user decide what to
do next. The "today" timeline and the "claude sessions" block are not useful, the new-session
picker is a bare list of directories, the tab bar leaves most of its width empty with few tabs
open, the Claude mode sits on its own line, and tig opens over the tab instead of beside it.
The docs are split over three files for a tool that fits in one page.

## Goals

- **Panel:** show what each session is doing and what it costs, so the user can pick where to
  go; show quota pace; show where the money went.
- **New session:** pick a project quickly instead of scrolling a directory list.
- **Tab bar:** use the empty width for something useful.
- **Status line:** the Claude mode shares the status line row, on the right.
- **tig:** opens as a split on the right side of the tab, half the screen.
- **Docs:** one README, short, for users; `docs/install.md` and `docs/usage.md` dissolved into it.

## Non-goals

- No new data sources beyond what hooks and the status line already record, unless a column
  below needs one (flagged under "Left to the builder").
- No changes to session status semantics, colours or Alt key bindings.
- No browser or desktop UI; everything stays in the terminal.

## Decisions (the user's)

Mockups approved in `/tmp/previews/redesign.html` (variant B) and
`/tmp/previews/painel-empilhado.html` (variant 3).

- **Panel layout (stacked, variant 3):** a fixed strip at the top with quota (5h and 7d gauges,
  time to reset, estimated time to the limit); below it the **Sessions** section, then **Spend
  7d by project** (bars with amounts). Sections are stacked in one column, full width.
- **Sessions section:** one row per open session with a header row. Columns: status mark,
  project, state (with how long it has waited), model, branch, context bar and percentage,
  time open, lines added, cost.
- **Removed from the panel:** the "today" timeline and the "claude sessions" block.
- **Tab bar:** tabs packed on the left; on the right, a summary: counts of tabs waiting
  (red) and finished (green), 5h quota percentage, clock. The status colours and the
  `!`, `?`, `✓` marks keep working as documented today.
- **Status line:** the Claude mode (for example "accept edits on") is on the same row as the
  status line, right-aligned.
- **tig:** opens in a pane to the right of the tab's Claude pane; the split is 50/50.
- **README:** a summary, not the full reference. Statuses and keys are short tables; install is
  a link to the install script in the remote repository, with no code block; no local paths.

## Decisions (my calls, to confirm)

- **Sessions columns:** exactly the nine listed above. Removed lines, time in the current
  state, last action and full path were offered and not chosen.
- **New-session picker:** the user did not pick a layout. Default is a single fuzzy search over
  projects (open ones first, then recent ones), with a right-hand preview of branch, dirty
  files and last commits (variant A's picker, grouped like variant B's). The path-typing option
  stays.
- **Tig toggle:** Alt+g keeps its key; pressing it again closes the pane.
- **Docs:** `docs/screenshots/` stays; screenshots are regenerated for the new panel and
  referenced from the README.

## Constraints

- Follow the repository's existing layout: `panel/data` loads, `panel/view` draws, `term/`
  holds tmux code.
- 100-character lines, absolute imports, tests for the loaders and view text.
- The README rules in the user's global instructions apply: no code, tables or lists, remote
  links only.

## Panel layout (the approved mockup, kept here because previews are disposable)

```
 5h ▓▓▓▓▓▓░░░░ 62%   7d ▓▓▓░░░░░░░ 35%   hits the 5h limit in ~2h10
 Sessions
   project          state          model   branch       context      time  lines  cost
 ! claude-sessions  permission 3m  Opus    main         ▓▓▓▓▓▓▓ 72%  1h12  +142   $4.10
 ● api-gateway      working        Sonnet  feat/routes  ▓▓▓▓ 41%     48m   +86    $2.35
 ✓ docs-site        done 12m       Haiku   docs/readme  ▓▓ 18%       25m   +210   $0.90
 · dotfiles         idle           Sonnet  main         ▓▓▓▓▓ 55%    3h05  +12    $1.20
 Spend 7d by project
   claude-sessions  ▓▓▓▓▓▓▓▓ $31
   api-gateway      ▓▓▓▓▓ $22
 [tab bar] 1 panel │ 2 claude-sessions ! │ 3 api-gateway ● │ …        ● 1 ● 1 · 5h 62% · 14:32
```

## Answers from the project

Found in the code while checking the design; the builder can rely on them.

- **Quota data:** the status line input already gives `used_percentage` and `resets_at` for
  the 5h and 7d windows, stored per window. "Time to the limit" is my call: a linear burn rate
  since the window started.
- **Spend 7d:** the journal already keeps cost events per session for 7 days (`KEEP_DAYS`).
  Grouping by project uses the project folder name, as the tabs do (my call). The journal stays;
  only the timeline drawing and the "claude sessions" block are removed, with their tests.
- **Picker:** it is already an `fzf` popup (`cs new --pick`). `fzf` stays, and "recent" is
  whatever the current picker already lists; the preview is added to it.
- **Tab bar right side:** `status-right` already shows per-tab dots and the clock, refreshed
  every second from per-window tmux options; counts cover all tabs of the tool's tmux server.
- **tig:** today a 90% popup bound to Alt+g in `tmux.conf`; the new version splits the current
  tab's window to the right. In tab 1 (the panel) Alt+g does nothing.

## Open risk

- **Claude mode source:** it is not known whether Claude Code's status line input or the hook
  events carry the mode (such as "accept edits on"). The builder checks first; if neither
  does, the mode move is dropped and the user is told, instead of scraping the pane.

## Left to the builder

- Where the extra columns (model, branch, time open, lines added) come from; if the status line
  or hooks do not record one, add it to the live record or drop the column and say so.
- How the tab bar's right side is rendered in tmux and how it behaves in narrow windows.
- How the mode is moved onto the status line row, and what is shown when Claude Code reports
  none.
- How to size the tig pane and what happens with several panes in a tab.
- Narrow-window behaviour of the panel (which columns drop first).
