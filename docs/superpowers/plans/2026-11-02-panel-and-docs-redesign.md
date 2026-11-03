# Panel, tab bar and docs redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the panel's header, usage lines and timeline with a quota strip, a richer
Sessions table and a 7-day spend section; fill the tab bar's right side; split tig to the
right; improve the new-session picker; fold the docs into one short README.

**Architecture:** The panel keeps its loader/view split: `PanelData.load` builds a `Snapshot`,
`PanelApp.redraw` draws widgets from it. New numbers come from data already recorded (status
line usage and limits, the journal's cost events, git for the branch). Tmux changes live in
`tmux.conf` and `term/data/new_session.sh`; the panel also publishes a one-line summary in a
tmux option that the tab bar prints.

**Tech Stack:** Python 3.11+, Textual 8, Rich, pytest, ruff, tmux 3.3+, fzf, tig.

**Spec:** `docs/superpowers/specs/2026-11-02-panel-and-docs-redesign-design.md`

## Global Constraints

- 100-character lines (`ruff` line-length 100; rules E, F, W, I, B); absolute imports only.
- Google-style docstrings on non-trivial public functions.
- Status colours, the `!` `?` `✓` marks and every Alt key binding keep working unchanged.
- No new data source unless Task 1 proves a field exists; a missing field drops its column or
  feature and is reported to the user, never scraped from a pane.
- README: for users, no code blocks, tables or lists, links to the remote repository only, no
  local paths; `docs/install.md` and `docs/usage.md` are deleted.
- Commits are Conventional Commits (subject ≤ 50 chars, body lines ≤ 72, imperative) and are
  made only when the user has said to commit; never push.
- Verification for every task: `uv run pytest -q` and `uv run ruff check .` pass.

## Review Focus

- **No status line input yet:** a session that has not reported usage shows empty cells, not
  errors (covered in Task 3).
- **Not a git repository / folder gone:** the branch cell is empty (Task 2).
- **No limits known:** the quota strip is hidden, the tab bar summary omits quota (Tasks 3, 4).
- **Narrow panel (< 80 columns):** columns drop from the right, the project name never
  disappears, no line wraps (Task 3).
- **Session spanning days or not in history:** spend counts only the cost added inside the
  7 days, and a session with no known folder is left out (Task 2).

---

### Task 1: Confirm the fields Claude Code sends

**Files:**
- Modify: `src/claude_sessions/claude/statusline.py` (`parse`)
- Modify: `src/claude_sessions/claude/hook.py` (`handle_event`) only if the hook input carries
  the mode
- Test: `tests/test_statusline.py`, `tests/test_hook.py`

**Interfaces:**
- Produces: `usage["context"]` (float percent 0-100 or None) on the session record, and, if a
  source exists, `record["mode"]` (str or None). Later tasks read these keys.

- [ ] **Step 1: Capture real input.** Temporarily have `cs statusline` and `cs hook` write
  their raw stdin to a file in the scratchpad; run one real Claude Code turn; read the files.
  Remove the capture code. Record in the commit body which fields exist: the context window
  usage percentage, and the permission/mode field.
- [ ] **Step 2: Write the failing test** `test_parse_reads_context_percentage` in
  `tests/test_statusline.py`: input with the context field found in Step 1 gives
  `parse(data)["context"] == 41.0`; input without it gives `None`.
- [ ] **Step 3: Implement** the `context` key in `statusline.parse`, using `_get` and
  `_number` as the other keys do.
- [ ] **Step 4: If Step 1 found a mode field**, test and store it as `record["mode"]` (a text
  such as `acceptEdits`), None when absent. If not found, write `no mode field` in the plan's
  Task 5 and skip to Step 5.
- [ ] **Step 5: Run** `uv run pytest tests/test_statusline.py tests/test_hook.py -q` → PASS.
- [ ] **Step 6: Commit** `feat(usage): record context use and mode`.

### Task 2: Loader data — branch, spend, pace

**Files:**
- Modify: `src/claude_sessions/core/journal.py` (add `spend_by_session`; remove `cost_today`
  and `waited`)
- Modify: `src/claude_sessions/panel/data/loader.py`
- Delete: `src/claude_sessions/panel/data/timeline.py`, `tests/test_timeline.py`
- Test: `tests/test_journal.py`, `tests/test_loader.py`

**Interfaces:**
- Produces: `journal.spend_by_session(day_events: dict[datetime.date, list[dict]], first_day:
  datetime.date) -> dict[str, float]` — per session, the highest cost reported inside the
  window minus the last cost reported before `first_day` (0 when none).
- Produces: `loader.branch_of(cwd: str) -> str` — the checked-out branch, `""` when the folder
  is not a git repository or is gone; cached per folder for 5 seconds.
- Produces: `loader.pace(window: dict, window_seconds: int, now: float) -> float | None` —
  seconds until the window reaches 100% at the average rate since it started, None when the
  rate is zero or the window is over.
- Produces: `Row.branch: str`; `Snapshot.spend: list[tuple[str, float]]` (project folder name,
  dollars, largest first, only projects with spend > 0); `Snapshot.pace: dict[str, float|None]`
  keyed `five_hour`/`seven_day`. `Snapshot` loses `cost_today`, `cost_open`, `waited`,
  `timeline`.

- [ ] **Step 1: Write failing tests** in `tests/test_journal.py`:
  `test_spend_counts_only_cost_added_in_window` (events day1 cost 3.0, day2 cost 5.0, first
  day = day2 → `{"s": 2.0}`) and `test_spend_of_new_session_is_its_max` (no earlier event →
  `{"s": 5.0}`).
- [ ] **Step 2: Implement** `spend_by_session`; run `uv run pytest tests/test_journal.py -q`.
- [ ] **Step 3: Failing tests** in `tests/test_loader.py`: `test_branch_of_repo` (tmp repo via
  `git init -b main`), `test_branch_of_non_repo_is_empty`, `test_pace_linear`
  (50% after 1h of a 5h window → 3600 s), `test_pace_none_at_zero`,
  `test_snapshot_spend_groups_by_folder_name` (two sessions in `/w/a` and `/w/b`; one id
  missing from history is left out).
- [ ] **Step 4: Implement** the loader pieces. `PanelData` keeps one `journal.DayReader` per
  date of the last 7 days (today's re-reads each tick, past days only parse once), maps
  session id → folder name from the history, and fills `Snapshot.spend`/`pace`. Window length
  is 5h = 18000 s and 7d = 604800 s.
- [ ] **Step 5: Remove** `timeline.py`, its test, and the `cost_today`/`waited` code and tests;
  fix imports. Run `uv run pytest -q` — only `test_app.py` and `test_widgets.py` may still
  fail (Task 3 fixes them).
- [ ] **Step 6: Commit** `feat(panel): load spend, branch and pace`.

### Task 3: Panel view — quota strip, sessions table, spend

**Files:**
- Modify: `src/claude_sessions/panel/app.py` (`compose`, `redraw`; delete `header_line`,
  `usage_lines`, `limit_text`, `USAGE_RIGHT`, `HEADER`)
- Modify: `src/claude_sessions/panel/view/layout.py` (`COLUMNS`, `data_cells`, `layout`)
- Modify: `src/claude_sessions/panel/view/widgets.py` (remove `Timeline`, `hour_axis`; add
  `Quota`, `Spend`)
- Modify: `src/claude_sessions/panel/view/text.py` (add `pace_text`)
- Modify: `src/claude_sessions/panel/view/panel.tcss`
- Test: `tests/test_app.py`, `tests/test_widgets.py`, `tests/test_layout.py`, `tests/test_text.py`

**Interfaces:**
- Consumes: `Row.branch`, `usage["context"]`, `Snapshot.limits`, `.pace`, `.spend` (Task 2).
- Produces: `text.pace_text(seconds: float | None) -> str` — `hits the limit in ~2h10m`,
  empty for None; `widgets.Quota.show(limits, pace, now)`; `widgets.Spend.show(spend)`.
- Panel order from top: `#quota` (fixed, hidden when no limits), `#search`, `#body` holding
  `RowList("Sessions")`, `Spend`, `RowList("Recent")`, `#empty`, then `#keys`.

- [ ] **Step 1: Failing tests.** `test_text.py`: `test_pace_text` (7800 s → `hits the limit
  in ~2h10m`; None → `""`). `test_layout.py`: `test_open_columns_in_order` asserts the kept
  columns at width 140 are `model, branch, context, time, lines, cost` after the title, and at
  width 60 only the title and the rightmost-first drops remain (cost, lines, time dropped
  first; `model` last). `test_app.py`: `test_quota_strip_shows_both_windows`,
  `test_quota_hidden_without_limits`, `test_spend_section_lists_projects`,
  `test_row_without_usage_has_empty_cells`.
- [ ] **Step 2: Run** them; expect FAIL.
- [ ] **Step 3: Implement** `COLUMNS` as
  `(("model",8,"left"),("branch",14,"left"),("context",5,"right"),("time",6,"right"),
  ("lines",6,"right"),("cost",7,"right"))` (drop order from the right is the table's own
  order reversed, so keep `cost` last); `data_cells` builds `branch` from `Row.branch` and
  `context` as `NN%` coloured with `text.limit_colour`. Rename the open section label to
  `Sessions`. Add `Quota` (two gauges on one line using `text.limit_bar`, `text.resets`,
  `text.pace_text` for the 5h window) and `Spend` (one `limit_bar`-style bar per project,
  scaled to the largest, with `money`).
- [ ] **Step 4: Update `app.py`**: remove the header and timeline, compose the order above,
  show the counts of waiting and finished sessions nowhere in the panel (the tab bar carries
  them, Task 4). Update `panel.tcss` (`#quota` height 1 with a bottom border-free margin;
  drop `#header`, `#usage`, `#timeline`).
- [ ] **Step 5: Run** `uv run pytest -q` → PASS. Run `uv run python scripts/screenshots.py` only
  in Task 8.
- [ ] **Step 6: Commit** `feat(panel): stack quota, sessions and spend`.

### Task 4: Tab bar summary on the right

**Files:**
- Modify: `src/claude_sessions/term/tmux.py` (add `publish_summary`)
- Modify: `src/claude_sessions/term/data/tmux.conf` (`status-right`, `status-right-length`)
- Modify: `src/claude_sessions/panel/app.py` (call it from `reload`)
- Test: `tests/test_tmux.py`, `tests/test_tmux_conf.py`

**Interfaces:**
- Produces: `tmux.summary_text(counts: dict[str, int], limits: dict) -> str` — tmux style
  codes included, e.g. `#[fg=red]● 1 #[fg=green]● 1 #[default]· 5h 62%`; omits zero counts and
  the quota part when the 5h window is unknown. `tmux.publish_summary(text: str) -> None` runs
  `set -g @cs-summary <text>`.
- `status-right` becomes `#{E:@cs-summary} %H:%M ` (the per-tab dots go away, replaced by the
  counts).

- [ ] **Step 1: Failing tests:** `test_summary_text_counts_and_quota`,
  `test_summary_text_without_limits`, and in `test_tmux_conf.py` that `status-right` contains
  `@cs-summary` and the clock.
- [ ] **Step 2: Implement** both functions and the config; `PanelApp.reload` publishes after
  each snapshot, only when the text changed.
- [ ] **Step 3: Run** `uv run pytest tests/test_tmux.py tests/test_tmux_conf.py -q` → PASS.
- [ ] **Step 4: Commit** `feat(tmux): show counts and quota in the tab bar`.

### Task 5: Mode on the status line row

Gated by Task 1: **if no mode field was found, skip this task and tell the user.**

**Files:**
- Modify: `src/claude_sessions/claude/statusline.py` (`print_statusline`)
- Test: `tests/test_statusline.py`

**Interfaces:**
- Consumes: `record["mode"]` (Task 1).
- Produces: `statusline.with_mode(line: str, mode: str | None, width: int) -> str` — pads
  `line` so `mode` ends at column `width`; returns `line` unchanged when there is no mode or no
  room.

- [ ] **Step 1: Failing tests:** `test_with_mode_right_aligns`, `test_with_mode_none_unchanged`,
  `test_with_mode_no_room_unchanged`.
- [ ] **Step 2: Implement** `with_mode` and use it in `print_statusline`, reading the width from
  the `COLUMNS` Claude Code provides or `shutil.get_terminal_size` as fallback.
- [ ] **Step 3: Run** `uv run pytest tests/test_statusline.py -q` → PASS.
- [ ] **Step 4: Commit** `feat(statusline): right-align the Claude mode`.

### Task 6: tig as a split on the right

**Files:**
- Modify: `src/claude_sessions/term/data/tmux.conf` (the `M-g` binding)
- Test: `tests/test_tmux_conf.py`

**Interfaces:**
- Produces: Alt+g in a tab ≥ 2: if the window has a pane with `@cs-tig` set, kill it; else, in
  a git repo, `split-window -h -p 50 -c "#{pane_current_path}" tig` (the new pane takes
  focus), then `set-option -p @cs-tig 1`. In tab 1 the key does
  nothing; outside a repo it shows the existing "Not a git repository" message.

- [ ] **Step 1: Failing test** in `test_tmux_conf.py`: the config's `M-g` binding contains
  `split-window -h`, `-p 50` and `@cs-tig`, and no longer contains `display-popup` for tig.
- [ ] **Step 2: Implement** the binding with `if -F` branches for tab 1, existing tig pane
  (`#{P:...}` over `@cs-tig`) and the repo check.
- [ ] **Step 3: Manual check** in `cst`: Alt+g splits, Alt+g again closes, `q` in tig closes
  the pane, tab 1 ignores it.
- [ ] **Step 4: Run** `uv run pytest tests/test_tmux_conf.py -q` → PASS; **commit**
  `feat(tmux): open tig as a right split`.

### Task 7: New-session picker with preview

**Files:**
- Modify: `src/claude_sessions/term/data/new_session.sh`
- Modify: `src/claude_sessions/cli.py` (a `cs new --preview PATH` option beside `--projects`)
- Modify: `src/claude_sessions/term/data/tmux.conf` and `term/tmux.py` (popup size 80% × 70%)
- Test: `tests/test_cli.py`, `tests/test_tmux_conf.py`

**Interfaces:**
- Produces: `cs new --preview PATH` prints, for a folder (the `~` form is accepted): branch,
  number of changed files, last three commits, one per line; prints `not a git repository`
  otherwise; never raises.
- `cs new --projects` lists folders of open sessions first, then the rest in today's order,
  without duplicates.

- [ ] **Step 1: Failing tests** in `test_cli.py`: `test_new_preview_in_repo`,
  `test_new_preview_outside_repo`, `test_new_projects_lists_open_first`.
- [ ] **Step 2: Implement** the option and the listing change; add
  `--preview 'cs new --preview {}' --preview-window right,45%` to the `fzf` call.
- [ ] **Step 3: Run** `uv run pytest tests/test_cli.py tests/test_tmux_conf.py -q` → PASS.
- [ ] **Step 4: Commit** `feat(cli): preview the folder in the picker`.

### Task 8: README only

**Files:**
- Rewrite: `README.md`
- Delete: `docs/install.md`, `docs/usage.md`
- Modify: `scripts/screenshots.py` (demo data for the new panel and the tig split)
- Regenerate: `docs/screenshots/*.svg`
- Test: `tests/test_package.py` if it references the docs

**Interfaces:** none.

- [ ] **Step 1: Regenerate screenshots** with `uv run python scripts/screenshots.py`; drop any
  shot of the removed timeline; add the tig split.
- [ ] **Step 2: Rewrite the README** as a summary: what it is (two sentences), a screenshot,
  a statuses table, a keys table, one line each for the panel, the picker and tig, install as
  a link to the install script in the remote repository, and the organisation tree. No code
  blocks, no local paths.
- [ ] **Step 3: Delete** the two docs; `rg "install.md|usage.md" .` finds nothing outside
  `docs/superpowers/`.
- [ ] **Step 4: Run** `uv run pytest -q` → PASS; **commit** `docs: fold install and usage into the README`.

### Task 9: Review

- [ ] **Step 1: Re-read** the whole diff for unnecessary complexity and naming.
- [ ] **Step 2: Run** `uv run pytest -q` and `uv run ruff check .` → clean.
- [ ] **Step 3: Run** `/code-review`; fix the findings that hold and report the rejected ones
  with the reason.
