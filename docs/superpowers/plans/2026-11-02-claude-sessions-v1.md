# claude-sessions v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build claude-sessions v1 as an installable Python package, with a Textual panel, in a
fresh repository whose 14 commits read as the project's development history.

**Architecture:** One `src/claude_sessions` package. The hook path (`cs hook`, `cs statusline`)
imports only stdlib modules: `state`, `store`, `tmux`, `usage` and `journal`. Textual loads
only in `cs panel`. The tmux config and the fzf picker ship as package data, and the config
calls `cs <subcommand>`. Most code is carried over from the working prototype, then moved and
reshaped. New code: `usage`, `journal`, `install`, the panel model and the Textual app.

**Tech Stack:**
- Python ≥ 3.11 with `textual>=8.2,<9`;
- hatchling build, uv for install and dev;
- pytest and ruff;
- tmux, fzf, bash.

**Spec:** `docs/superpowers/specs/2026-11-02-claude-sessions-v1-design.md`

**Prototype (read-only source to carry code from):** `$PROTO`, the working prototype's
folder; the executor is given its path. Never modify it. Never copy personal paths, employer
project names or ticket keys from it into this repository.

## Global Constraints

- **Platform:** Linux only. Python ≥ 3.11. Textual is the only runtime dependency.
- **Code rules:** 100-char lines; absolute imports; Google-style docstrings on non-trivial
  public APIs; ruff rules `E F W I B`. Match the prototype's style: short docstrings,
  comments that explain *why*.
- **Commits:**
  - Conventional Commits in the imperative mood; subject ≤ 50 chars, body lines ≤ 72;
  - the body is one-line bullets of what changed and why;
  - every message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`;
  - each commit passes `uv run pytest` and `uv run ruff check . && uv run ruff format --check .`;
  - never push.
- **Repo identity:** the owner's repo-local git identity and SSH key are set by the executor
  before the first commit. Branch `main`.
- **Behaviour parity:** the tab bar, keys, status rules and hook logic behave exactly as the
  prototype's. Carry the prototype's tests over.
- **Fail-safe:** `cs hook` and `cs statusline` never raise, never exit non-zero and never
  block. tmux calls time out after 2 s.
- **Hook weight:** importing `claude_sessions.cli` must not import `textual`.
- **No history churn:** no commit adds something a later commit removes, apart from tab 1
  being a shell in Task 5 only. No ticker process, no old pending format.
- **Docs:** English. The README has no code blocks and uses remote links only.

## Review Focus

1. **`settings.json` already holding other hooks on the same events, plus a user status line
   with extra keys (`padding`), possibly a symlink.** Install keeps all of it. Uninstall
   restores the original byte-for-byte as JSON. Tests: Task 13.
2. **Status line input that is partial or broken.** This covers no `rate_limits`, `cost` null
   early in a session, non-JSON stdin, and an unknown session. Nothing is recorded wrongly,
   and the chained command's output is still printed. Tests: Task 10.
3. **A journal with a torn last line, a missing directory, or yesterday's file only.** Sums
   skip the bad line and return 0 rather than failing. Tests: Task 11.
4. **Hook run outside tmux, or with the tmux server gone.** The record is still written and
   nothing is raised. Tests: Task 4.
5. **Panel in a tiny terminal (40×12) with zero sessions and empty history.** It shows the
   empty message and does not crash. A long title shrinks before the data columns. Tests:
   Task 6.

---

## File Structure

```
pyproject.toml            package, deps, scripts cs/cst, pytest + ruff config
LICENSE                   Apache-2.0 full text
README.md                 (Task 14)
.gitignore
src/claude_sessions/
  __init__.py             __version__ = "1.0.0"
  state.py                pure state machine (prototype core/claude_sessions/state.py)
  store.py                live records on disk (prototype store.py)
  tmux.py                 every tmux call: prototype mux.py + tmux parts of sessions_tmux.py
  claude_files.py         Claude Code's own files: history.jsonl, transcript titles
  usage.py                status line input → usage dict and limits
  journal.py              day history: append, read, sums, timeline
  install.py              settings.json merge / unmerge
  cli.py                  `cs` subcommands
  launcher.py             `cst`
  panel/__init__.py
  panel/model.py          view model, no Textual import
  panel/app.py            Textual app
  panel/panel.tcss
  data/tmux.conf
  data/new_session.sh
tests/
  conftest.py             tmp state dirs via env vars
  test_state.py test_store.py test_tmux.py test_claude_files.py test_usage.py
  test_journal.py test_install.py test_cli.py test_panel_model.py test_panel_app.py
  test_tmux_conf.py       renders tab labels on a throwaway tmux server
docs/install.md docs/usage.md docs/screenshots/
scripts/screenshots.py    regenerates docs/screenshots (Task 14)
```

**Environment overrides (tests and screenshots):**
- `CLAUDE_SESSIONS_DIR`: live records; defaults to `$XDG_RUNTIME_DIR/claude-sessions`.
- `CLAUDE_SESSIONS_STATE`: journal and saved status line; defaults to
  `$XDG_STATE_HOME/claude-sessions`, or `~/.local/state/claude-sessions`.
- `CLAUDE_CONFIG_DIR`: honoured where Claude's files are read; defaults to `~/.claude`.

`tests/conftest.py` sets all three to `tmp_path` directories in an autouse fixture.

---

### Task 1: Scaffold the project

**Files:**
- Create: `pyproject.toml`, `LICENSE`, `.gitignore`, `src/claude_sessions/__init__.py`,
  `tests/conftest.py`, `tests/test_package.py`
- Already present: the design and this plan, under `docs/superpowers/`

**Interfaces:**
- Produces: `claude_sessions.__version__ == "1.0.0"`, and the console scripts
  `cs = "claude_sessions.cli:main"` and `cst = "claude_sessions.launcher:main"`. Each is
  declared in the commit whose module exists. Before that, `[project.scripts]` is omitted.

- [ ] **Step 1: Init the repo** with `git init -b main` and set the repo identity from Global
  Constraints.
- [ ] **Step 2: Write `pyproject.toml`.**
  - `[project]`: `name="claude-sessions"`, `version="1.0.0"`, `requires-python=">=3.11"`,
    `dependencies=["textual>=8.2,<9"]`, `license="Apache-2.0"`, and a one-line description:
    "Tabs that show which Claude Code session needs you".
  - Build with hatchling; the wheel includes `src/claude_sessions` with `data/*` and
    `panel/*.tcss`.
  - `[dependency-groups] dev=["pytest>=8", "ruff>=0.6"]`.
  - `[tool.pytest.ini_options] testpaths=["tests"]`.
  - `[tool.ruff] line-length=100`, `[tool.ruff.lint] select=["E","F","W","I","B"]`.
- [ ] **Step 3: Add the support files.**
  - `LICENSE` holds the Apache-2.0 text from apache.org.
  - `.gitignore` covers `__pycache__/`, `.pytest_cache/`, `.ruff_cache/`, `.venv/`, `dist/`.
  - `conftest.py` is the autouse env fixture above.
- [ ] **Step 4: Write `tests/test_package.py::test_version`,** which asserts
  `claude_sessions.__version__ == "1.0.0"`.
- [ ] **Step 5: Verify.** Run `uv run pytest` (1 passed) and `uv run ruff check .`.
- [ ] **Step 6: Commit** everything, including `docs/superpowers/`, as
  `chore: scaffold the project`.

### Task 2: State machine

**Files:**
- Create: `src/claude_sessions/state.py` (copy of `$PROTO/core/claude_sessions/state.py`)
- Test: `tests/test_state.py` (copy of `$PROTO/core/tests/test_state.py`)

**Interfaces:**
- Produces: `PERMISSION INPUT DONE WORKING IDLE`, `STATUSES`, `NEEDS_YOU`,
  `apply_event(record, event, now) -> dict`, `new_record(id)`, `mark_seen`,
  `mark_interrupted`, `project(record) -> str`, `since(record) -> float`, `sort_key`.

- [ ] **Step 1: Copy the tests** and remove any test that only covers compatibility with older
  record formats, such as a string `pending` or dict pending. v1 has no older format.
- [ ] **Step 2: Run them.** `uv run pytest tests/test_state.py` fails with ImportError.
- [ ] **Step 3: Copy `state.py`.** Drop the compatibility branch in `apply_event`, so
  `pending = [list(p) for p in r.get("pending") or []]`. Keep the sort_key guard for unknown
  statuses.
- [ ] **Step 4: Verify.** `uv run pytest tests/test_state.py` passes.
- [ ] **Step 5: Commit** as `feat(state): track session status from hooks`.

### Task 3: Live record store

**Files:**
- Create: `src/claude_sessions/store.py`, from `$PROTO/core/claude_sessions/store.py`
- Create: `src/claude_sessions/tmux.py`, holding only `notify(record, ended=False)` and its
  helpers from `$PROTO/core/claude_sessions/mux.py`, because `store.settle_interrupted` needs
  it
- Test: `tests/test_store.py`, from `$PROTO/core/tests/test_store.py`

**Interfaces:**
- Consumes: `state.*`.
- Produces:
  - `store.state_dir()`, `locked(id)`, `read(id)`, `write(record)`,
    `remove(id, keep_lock=False)`;
  - `remove_replaced(record)`, `process_start(pid)`, `is_alive(record)`;
  - `load_all(now) -> list[dict]`, most urgent first;
  - `settle_interrupted(id, now)`;
  - `tmux.notify(record, ended=False)`.

- [ ] **Step 1: Copy the tests,** importing `claude_sessions.tmux` wherever they patched
  `mux`. Drop the old-pending-format test.
- [ ] **Step 2: Run them.** They fail with ImportError.
- [ ] **Step 3: Copy the code** and rename the `mux` import to `tmux`.
- [ ] **Step 4: Verify.** `uv run pytest` passes.
- [ ] **Step 5: Commit** as `feat(store): keep live session records`.

### Task 4: `cs hook`

**Files:**
- Create: `src/claude_sessions/cli.py`, built from `$PROTO/core/claude_sessions/cli.py` with
  only the `hook` subcommand for now
- Modify: `src/claude_sessions/tmux.py`, adding `detect(env)`, `name_window(record)`,
  `settle_if_viewed(record, now)` and `window_name(record)` from `mux.py`
- Modify: `pyproject.toml`, adding the `cs` script
- Test: `tests/test_cli.py`, `tests/test_tmux.py` (the latter from `$PROTO/core/tests/test_mux.py`)

**Interfaces:**
- Produces:
  - `cli.main(argv=None) -> int`;
  - `cli.handle_event(event: dict, env: dict, now: float) -> None`;
  - hook errors are appended to `<state_dir>/hook-errors.log`.

- [ ] **Step 1: Write the tests.**
  - `test_hook_outside_tmux_writes_record`: an env without `TMUX`. `UserPromptSubmit` gives
    a record with `status == "working"` and `mux == {}`.
  - `test_hook_with_dead_tmux_socket_does_not_raise`: `TMUX=/nonexistent,1,0`,
    `TMUX_PANE=%1`. The record is still written.
  - `test_hook_bad_stdin_logs_and_returns_0`: `main(["hook"])` with stdin `"{"` returns 0,
    and `hook-errors.log` exists.
  - `test_cli_does_not_import_textual`: a subprocess runs
    `import claude_sessions.cli, sys; assert "textual" not in sys.modules`.
- [ ] **Step 2: Run them.** They fail.
- [ ] **Step 3: Implement.** `handle_event` and `cmd_hook` are carried over unchanged.
- [ ] **Step 4: Verify.** `uv run pytest` passes.
- [ ] **Step 5: Commit** as `feat(cli): add the cs hook command`.

### Task 5: Status on each tab

**Files:**
- Create: `src/claude_sessions/data/tmux.conf`, from `$PROTO/tmux/tmux.conf`. Only the
  terminal, keys (Alt+1…9, Alt+0, Alt+, Alt+., Alt+← →, Alt+r), mouse, status, popup style
  and seen-hook sections go in. Alt+n, Alt+c and Alt+g are added by later tasks.
- Create: `src/claude_sessions/launcher.py` (`cst`)
- Modify: `cli.py`, adding `cs seen --tmux-pane P --tmux-socket S` (carried over)
- Modify: `pyproject.toml`, adding the `cst` script
- Test: `tests/test_tmux_conf.py`, `tests/test_cli.py`

**Interfaces:**
- Produces: `launcher.main(argv=None) -> int`. The tmux server is `tmux -L claude`, session
  `claude`, and window 1 is named `sessions`.
- In the tmux config, the `@cs-seen` command becomes
  `cs seen --tmux-pane #{pane_id} --tmux-socket #{socket_path}`, and the file's header
  comment drops `CLAUDE_SESSIONS_HOME`.

- [ ] **Step 1: Write `test_tmux_conf.py`** (skipped when `tmux` is missing).
  - It starts `tmux -L cs-test-<pid> -f <conf> new-session -d` with `base-index` as in the
    config, and kills the server in teardown.
  - For each status in `permission input done working idle` it sets the pane option
    `@claude_status` on window 2. Then it asserts on
    `display -p -t :2 '#{T:window-status-format}'` with styles stripped:
    - `permission` contains `! `;
    - `input` contains `? `;
    - `done` contains `✓ `;
    - `working` contains `●` or `○`;
    - `idle` contains none of these marks.
  - It asserts that window 1's format contains `≡`.
- [ ] **Step 2: Write `test_seen_marks_done_session_idle`** in `test_cli.py`. A done record
  whose `mux` points at pane `%3` on socket `S` becomes idle after
  `main(["seen","--tmux-pane","%3","--tmux-socket","S"])`.
- [ ] **Step 3: Run them.** They fail.
- [ ] **Step 4: Implement `launcher.main`.** It is the Python form of `$PROTO/tmux/cst`.
  - `--ghostty` re-execs Ghostty with the same unbind flags, running `cst`.
  - It prepends `os.path.dirname(os.path.abspath(sys.argv[0]))` to `PATH`, so tmux finds
    `cs`.
  - The config path is
    `importlib.resources.files("claude_sessions") / "data" / "tmux.conf"`.
  - When there is no session, it creates the `sessions` window in `$HOME` with the user's
    shell, then execs `tmux -L claude -f <conf> attach-session -t claude`.
- [ ] **Step 5: Implement `cs seen`.**
- [ ] **Step 6: Find and document the minimum tmux version.** Check every format and option
  the config uses against tmux's CHANGES file. The expected answer is 3.3, because of the
  `popup-style` / `popup-border-lines` options and `display-popup -T`. Record it as a comment
  in the config's header.
- [ ] **Step 7: Verify.** Run `uv run pytest`. Then, manually, run `uv tool install -e .` and
  `cst`: the tab bar renders and tab 1 is a shell.
- [ ] **Step 8: Commit** as `feat(tmux): show each session's status on its tab`.

### Task 6: Panel listing open and recent sessions

**Files:**
- Create: `src/claude_sessions/claude_files.py`
  - The history parsing, `transcript_path`, `find_title` and `Titles` come from
    `$PROTO/tmux/panel.py`.
  - `recent_projects` comes from `$PROTO/tmux/sessions_tmux.py`.
  - All of them read one parsed history.
- Modify: `src/claude_sessions/tmux.py`, adding `Window`, `run(*args)` (current server),
  `table`, `windows_of(records)`, `window_tabs()` and `select(window)` from
  `sessions_tmux.py`
- Create: `src/claude_sessions/panel/model.py`, `panel/app.py`, `panel/panel.tcss`
- Modify: `cli.py`, adding `cs panel`, which imports `claude_sessions.panel.app` inside the
  function
- Modify: `launcher.py`, so window 1 runs `while :; do cs panel; sleep 1; done`
- Test: `tests/test_claude_files.py`, `tests/test_panel_model.py`, `tests/test_panel_app.py`

**Interfaces:**
- `claude_files`:
  - `HISTORY` (under `CLAUDE_CONFIG_DIR`);
  - `read_history(lines) -> list[HistorySession]`, where `HistorySession` has
    `id, cwd, last, first_prompt`;
  - `load_history() -> list[HistorySession]`, cached on mtime and size;
  - `recent_projects() -> list[str]`;
  - `Titles().get(id, path) -> str`;
  - `transcript_path(id, cwd)`.
- `panel.model`:
  - `Row(session_id, title, cwd, window, tab, last, elsewhere, usage: dict)`;
  - `Snapshot(open: list[Row], recent: list[Row], counts: dict[str, int])`.
    Tasks 10 to 12 add `limits`, `cost_today`, `cost_open`, `waited` and `timeline`;
  - `PanelData().load(now) -> Snapshot`;
  - `filter_rows(rows, query) -> list[Row]`;
  - `status_cell(row, now) -> tuple[str, str, str]`, returning (mark, colour name,
    right-hand text);
  - `when(ts, now) -> str`;
  - `RECENT = 50`.
- `panel.app`:
  - `PanelApp(data: PanelData)`;
  - `actions(tmux module)`, which is patchable in tests.

- [ ] **Step 1: Write the `claude_files` tests,** porting the prototype's history and title
  tests from `$PROTO/tmux/tests/test_panel.py`.
- [ ] **Step 2: Write the `panel.model` tests.**
  - `test_status_cell_permission` expects `("!", "red", "tab 2  permission 3m")` for a
    permission session waiting 200 s.
  - `test_status_cell_working_blinks` expects `●` on even seconds and `○` on odd ones.
  - `test_open_sorted_by_tab`.
  - `test_recent_skips_open_and_command_only_sessions`.
  - `test_filter_rows_matches_all_words_in_title_or_folder`.
  - `test_when_today_yesterday_weekday_date`, with values from the prototype's `when`.
- [ ] **Step 3: Write the `test_panel_app.py` tests,** using `App.run_test` with a stub
  `PanelData`.
  - `test_empty_tiny_terminal`: at size (40, 12) with no sessions and no history, the screen
    shows "No sessions yet. Press n to start one." and nothing raises.
  - `test_long_title_shrinks_first`: at width 80, a 200-character title is cut with `…` and
    the right-hand `tab N` text is fully visible.
  - `test_down_enter_selects_window`: pressing down then enter calls `tmux.select` with the
    second open row's window.
  - `test_search_filters_and_escape_clears`: `/`, typing `api`, then enter leaves only the
    matching rows; escape restores all of them.
  - `test_ctrl_c_does_not_quit`: after ctrl+c the app is still running.
- [ ] **Step 4: Run them.** They fail.
- [ ] **Step 5: Implement `PanelData.load`.** It is a port of the prototype `Panel.load` and
  `recent_rows`, plus counts per status over the open rows.
- [ ] **Step 6: Implement `PanelApp`** to match the spec's layout A, without the usage block
  and the timeline.
  - **Lines:** a header line, "Open" rows, a "Recent" section and a keys line.
  - **Colours:** red, yellow, green, blue and greys 244, 240 and 250, as the tab bar uses.
    The selected row's background is grey 237.
  - **Refresh:** a timer that fires on each second boundary, as the prototype does.
  - **Keys:** ↑↓, j/k, enter, `/`, escape and mouse click (select only), as the prototype
    handles them.
  - **Quit keys:** ctrl+c and ctrl+q are bound with priority to a no-op.
  - **Messages:** shown in the keys line in yellow for 4 s.
  - **Enter:** follows the prototype's `open_selected` rules (`RESUME_SECONDS = 10`, the
    "open elsewhere" and "folder is gone" messages), with resume being
    `tmux new-window -c <cwd> "claude --resume <id>"`.
  - **Widgets:** the choice is free, but a cursor that moves across both sections must be
    kept.
- [ ] **Step 7: Verify.** Run `uv run pytest`. Then, manually, restart `cst`: tab 1 shows
  the panel with live sessions.
- [ ] **Step 8: Commit** as `feat(panel): list open and recent sessions`.

### Task 7: Alt+n

**Files:**
- Modify: `tmux.py`, adding `next_pending(windows, current) -> Window | None`
  (carried over)
- Modify: `cli.py`, adding `cs next`, which shows "Nothing is waiting for you" when none
- Modify: `data/tmux.conf`, adding `bind -n M-n run-shell 'cs next'`
- Test: `tests/test_tmux.py`, porting `$PROTO/tmux/tests/test_sessions_tmux.py`'s
  next-pending tests

- [ ] **Step 1: Port the tests** and run them; they fail.
- [ ] **Step 2: Implement** and run them; they pass.
- [ ] **Step 3: Commit** as `feat(tmux): jump to the next session needing you`.

### Task 8: Alt+c

**Files:**
- Create: `data/new_session.sh`, from `$PROTO/tmux/new_session.sh`. It calls
  `cs new --projects` instead of `sessions_tmux.py projects`.
- Modify: `cli.py`, adding `cs new`, which opens the popup
  (`display-popup -E -w 70% -h 60% -T ' New session ' bash <script>`), and
  `cs new --projects`, which prints `claude_files.recent_projects()` with `~` for home
- Modify: `data/tmux.conf`, adding `bind -n M-c run-shell 'cs new'`
- Modify: `panel/app.py`, adding key `n` (runs `cs new`) and showing "n new session" in the
  keys line
- Test: `test_cli.py::test_new_projects_prints_tilde_paths`,
  `test_panel_app.py::test_n_opens_new_session`

- [ ] **Step 1: Write the tests** and run them; they fail.
- [ ] **Step 2: Implement** and run them; they pass. Then check by hand that Alt+c picks a
  folder and opens `claude` there.
- [ ] **Step 3: Commit** as `feat(tmux): start sessions from recent folders`.

### Task 9: Alt+g

**Files:**
- Modify: `data/tmux.conf`, adding the Alt+g binding exactly as in `$PROTO/tmux/tmux.conf`
  (the `if "git -C … rev-parse"` popup running `tig`, else the "Not a git repository"
  message)
- Test: `test_tmux_conf.py::test_alt_g_bound`, which asserts that `list-keys -T root M-g`
  contains `tig`

- [ ] **Step 1: Write the test** and run it; it fails.
- [ ] **Step 2: Add the binding** and run the test; it passes.
- [ ] **Step 3: Commit** as `feat(tmux): show commit history in a popup`.

### Task 10: Usage from the status line

**Files:**
- Create: `src/claude_sessions/usage.py`
- Modify: `cli.py`, adding `cs statusline`
- Modify: `panel/model.py` and `panel/app.py`, adding the usage columns and the limits block
- Test: `tests/test_usage.py`, `test_cli.py`, `test_panel_model.py`

**Interfaces:**
- `usage.parse(data: dict) -> dict` returns `{"cost": float|None, "duration": float|None
  (seconds), "model": str|None, "added": int, "removed": int}`. Its sources are
  `cost.total_cost_usd`, `cost.total_duration_ms`, `model.display_name` (falling back to
  `model.id`), `cost.total_lines_added` and `cost.total_lines_removed`. Missing or null
  values give `None` or `0`.
- `usage.parse_limits(data: dict) -> dict` returns
  `{"five_hour": {"pct": float, "resets_at": float}, "seven_day": {...}}`, with only the
  windows that are present.
- `usage.save_limits(limits, now)` and `usage.load_limits(now) -> dict`.
  - Limits live in `<store.state_dir()>/limits.json` and are written atomically.
  - On save, each new window replaces the stored one.
  - On load, windows whose `resets_at <= now` are dropped.
- `usage.saved_statusline_path()` is `<CLAUDE_SESSIONS_STATE>/statusline.json`, which holds
  the user's original `statusLine` object (written by Task 13).
- `cli.cmd_statusline`:
  1. Read stdin bytes.
  2. In a `try` block, under `store.locked(session_id)`, if a record exists, set
     `record["usage"] = usage.parse(data)` and `store.write`; also `save_limits`.
  3. Then, if `statusline.json` exists, run its `command` with `shell=True`, the same stdin
     and a 5 s timeout, and write its stdout to ours.
  4. Otherwise print `f"{model}  ${cost:.2f}"`, or `""` when either is unknown.
  5. Always return 0.
- `model.Row.usage` comes from the record, `{}` when absent.
- Formatters: `money(x) -> "$4.12"`; `duration(s) -> "4m" | "35m" | "1h40m"`;
  `lines(a, r) -> ("+820", "−310")`; `None` formats as `""`.
- `Snapshot.limits = usage.load_limits(now)`.

- [ ] **Step 1: Write the tests.**
  - `test_parse_full_input`, using the docs' example values.
  - `test_parse_early_session_nulls`: `cost` null, no `rate_limits`.
  - `test_parse_limits_partial`: only `seven_day`.
  - `test_load_limits_drops_expired`.
  - `test_statusline_unknown_session_still_chains`: no record exists, the saved command is
    `echo hi`, and the output is `hi`.
  - `test_statusline_non_json_stdin_prints_and_returns_0`.
  - `test_statusline_records_usage_without_touching_status`.
  - `test_duration_formats`: 240 → `4m`, 2100 → `35m`, 6000 → `1h40m`.
- [ ] **Step 2: Run them.** They fail.
- [ ] **Step 3: Implement** `usage`, `cs statusline` and the formatters.
- [ ] **Step 4: Update the panel.**
  - Add the columns `cost`, `time`, `model` and `lines`, after the title and before the
    right-hand text, with a dim header row `Open … cost time model lines`.
  - Add the usage block under the header: `5h limit` and `7d limit` bars, 20 cells of
    `▰▱`, `NN%`, and `resets in 1h52m` (under a day) or `resets Fri 09:00`.
  - Bar colour: grey below 70, amber from 70, red from 85.
  - A missing window omits its line.
  - When narrow, data columns drop from the right (lines first, then model) once the title
    reaches 14 cells.
- [ ] **Step 5: Verify.** Run `uv run pytest`.
- [ ] **Step 6: Commit** as `feat(usage): record usage from the status line`.

### Task 11: Journal of the day

**Files:**
- Create: `src/claude_sessions/journal.py`
- Modify: `store.write`, which appends a status event when the status differs from the
  record on disk (it is always called under the lock)
- Modify: `cli.cmd_statusline`, which appends a cost event when the cost changed
- Modify: `panel/model.py` and `panel/app.py`, adding cost today and waiting time
- Test: `tests/test_journal.py`

**Interfaces:**
- **Storage:** `<CLAUDE_SESSIONS_STATE>/journal/YYYY-MM-DD.jsonl`, by local date of `t`.
  - One JSON object per line, appended with a single `os.write` on an `O_APPEND` fd.
  - Lines look like `{"t": float, "id": str, "status": str}` or
    `{"t": float, "id": str, "cost": float}`.
  - Files older than 7 days are deleted on append.
- **Functions:**
  - `journal.append(event: dict)`.
  - `journal.read_day(day: datetime.date) -> list[dict]`. It skips unparsable lines and
    returns `[]` when the file is missing.
  - `journal.cost_today(events) -> float`: the sum, per id, of the last cost seen today.
  - `journal.waited(events, now) -> float`: per id, the seconds spent in `permission` or
    `input`. An interval still open at `now` ends at `now`. Time before an id's first event
    today is not counted.
- **Snapshot:** gains `cost_today`, `cost_open` (the same sum restricted to open ids) and
  `waited`.

- [ ] **Step 1: Write the tests.**
  - `test_cost_today_takes_last_per_session`.
  - `test_waited_sums_permission_and_input_until_now`.
  - `test_read_day_skips_torn_line`.
  - `test_read_day_missing_dir_is_empty`.
  - `test_store_write_logs_only_status_changes`: two writes with the same status give one
    event.
  - `test_old_files_pruned`.
- [ ] **Step 2: Run them.** They fail.
- [ ] **Step 3: Implement** the journal, the `store.write` hook and the statusline cost
  event.
- [ ] **Step 4: Update the panel.** The usage block's right side shows
  `today  $11.40  ($8.67 in open sessions)` and `waited for you today  47m`.
- [ ] **Step 5: Verify.** Run `uv run pytest`.
- [ ] **Step 6: Commit** as `feat(journal): keep a history of the day`.

### Task 12: Timeline

**Files:**
- Modify: `journal.py`, adding `timeline(...)`
- Modify: `panel/model.py`, adding `Snapshot.timeline`
- Modify: `panel/app.py`
- Test: `test_journal.py`, `test_panel_app.py`

**Interfaces:**
- `journal.timeline(events, ids: list[str], start: float, end: float, cells: int)
  -> dict[str, list[str | None]]`. Each cell holds the most urgent status (in
  `state.STATUSES` order) the session had at any moment inside the cell. A short permission
  wait therefore stays visible at 10-minute cells. A cell before the session's first event
  is `None`.
- `Snapshot.timeline`: `(start, end, rows: list[(title, cells)])`, where `start` is the
  earliest event today among open sessions and `end` is now. `cells` is computed by the app
  from its width.

- [ ] **Step 1: Write the tests.**
  - `test_timeline_cell_takes_most_urgent`: working for 9 minutes and permission for 1
    minute in the same cell gives `permission`.
  - `test_timeline_none_before_first_event`.
  - `test_timeline_renders_axis_and_legend`, a pilot test that checks for `today`, an
    `HH:00` label and the legend words `working permission question finished, not seen idle`.
- [ ] **Step 2: Run them.** They fail.
- [ ] **Step 3: Implement** the timeline per the spec's sketch.
  - Each row has a 22-cell name, then the bar cells as a space with a background:
    - working: blue;
    - permission: red;
    - question: yellow;
    - done: green;
    - idle: grey 238;
    - `None`: no background.
  - Above the bars runs the hour axis, below them the legend.
- [ ] **Step 4: Verify.** Run `uv run pytest`.
- [ ] **Step 5: Commit** as `feat(panel): draw today's timeline`.

### Task 13: `cs install` / `cs uninstall`

**Files:**
- Create: `src/claude_sessions/install.py`
- Modify: `cli.py`
- Test: `tests/test_install.py`

**Interfaces:**
- `EVENTS`: the 11 events from the spec, in this order: SessionStart, SessionEnd,
  UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest,
  Notification, Stop, StopFailure, SubagentStop.
- `install.hook_command(cs: str) -> str` returns `f'"{cs}" hook'`.
  `install.statusline_command(cs)` returns `f'"{cs}" statusline'`.
- `install.is_ours(command: str) -> bool` is true when the command is the quoted path of an
  executable named `cs` followed by `hook` or `statusline`.
- `install.merge(settings: dict, cs: str, saved: dict | None) -> tuple[dict, dict | None]`
  returns `(new_settings, statusline_to_save)`.
  - It is pure and idempotent.
  - For each event, it appends one group
    `{"hooks": [{"type": "command", "command": hook_command(cs), "timeout": 10}]}`, unless
    one of ours is already there.
  - It sets `statusLine` to the original object with only `command` replaced, keeping keys
    like `padding`. A new object is `{"type": "command", "command": ...}`.
  - It saves the original only when it was not ours.
- `install.unmerge(settings: dict, saved: dict | None) -> dict`.
  - It removes our hook entries, then any group left with no hooks, then any event left
    empty, then `hooks` if it is empty.
  - It restores `saved` as `statusLine`, or deletes the key if `saved` is None and the
    current one is ours.
- `cs install`:
  1. Reads `~/.claude/settings.json` (under `CLAUDE_CONFIG_DIR`), or `{}` if missing.
  2. Writes a backup copy `settings.json.before-claude-sessions` once.
  3. Writes the result atomically to `os.path.realpath(path)`, so a symlink stays a symlink,
     with `indent=2` plus a trailing newline.
  4. Writes `statusline.json`.
  5. Prints what changed.

  `cs` is `os.path.abspath(sys.argv[0])`.
- `cs uninstall` reverses the install and deletes `statusline.json`.

- [ ] **Step 1: Write the tests.**
  - `test_merge_keeps_foreign_hooks_on_same_event`.
  - `test_merge_is_idempotent`.
  - `test_merge_wraps_user_statusline_keeping_padding`.
  - `test_unmerge_restores_original_exactly`: `unmerge(merge(x)) == x` for a settings dict
    with foreign hooks, a status line with padding, and unrelated keys.
  - `test_unmerge_without_saved_removes_our_statusline`.
  - `test_install_writes_through_symlink`: `settings.json` is a symlink before and after
    install, and the target has the hooks.
  - `test_install_on_missing_settings_creates_file`.
- [ ] **Step 2: Run them.** They fail.
- [ ] **Step 3: Implement** `install.py` and the two subcommands.
- [ ] **Step 4: Verify.** Run `uv run pytest`. Then, manually, with
  `CLAUDE_CONFIG_DIR=$(mktemp -d)`, run `cs install` and `cs uninstall` and diff the
  result.
- [ ] **Step 5: Commit** as `feat(cli): register hooks with cs install`.

### Task 14: Documentation and screenshots

**Files:**
- Create: `README.md`, `docs/install.md`, `docs/usage.md`, `scripts/screenshots.py`
- Create: `docs/screenshots/tabs.png`, `panel.svg`, `new-session.png`, `history.png`

**Interfaces:**
- **`scripts/screenshots.py`** builds a demo world and captures the screenshots.
  - The demo uses a temporary `CLAUDE_SESSIONS_DIR` and `CLAUDE_SESSIONS_STATE`, records
    whose `pid` is a live `sleep` process, and a `tmux -L cs-demo` server.
  - The server has windows named with neutral projects (`api`, `docs-site`, `dashboards`,
    `release-notes`, `infra`), one per status.
  - It sets `@claude_status` and `@cs-since`, writes usage and limits, and fills the journal
    with a plausible day.
  - The panel is captured with Textual's `save_screenshot` at 120×36 to give `panel.svg`.
  - The tab bar, Alt+c and Alt+g are captured from a real Ghostty window attached to the
    demo server, using `gnome-screenshot -w` or `import -window` with `xdotool`.
  - The demo server and processes are cleaned up afterwards.
- **README:**
  - one paragraph on what the tool is;
  - the tab bar and panel screenshots;
  - the statuses table (as in `$PROTO/README.md`);
  - a "How it is organised" table of the modules from the spec's Pieces table, with no code;
  - links to `docs/install.md` and `docs/usage.md` as relative repo links, since those
    render on GitHub;
  - a license line.
- **`docs/install.md`:**
  - requirements: Linux, Python ≥ 3.11, uv, tmux ≥ (the version from Task 5), fzf, and the
    optional tig and Ghostty;
  - `uv tool install git+https://github.com/emanuel36/claude-sessions`;
  - `cs install`, with a table of exactly what it changes (11 hook events, `statusLine`,
    the backup file, `statusline.json`);
  - Ghostty notes for `cst --ghostty`;
  - updating with `uv tool upgrade claude-sessions`;
  - `cs uninstall` followed by `uv tool uninstall`;
  - troubleshooting: `hook-errors.log` location, empty usage columns until the first status
    line update, and a status line shown only for subscribers.
- **`docs/usage.md`:**
  - opening with `cst`;
  - the statuses;
  - the full key table (from `$PROTO/README.md`);
  - the panel section by section, matching the spec's sketch;
  - where each number comes from, in a table of status line versus hooks versus journal;
  - the limitations: Linux only, and cost today counting cross-midnight sessions on both
    days.

- [ ] **Step 1: Write and run `scripts/screenshots.py`.** Look at each image and redo any
  with private names or broken rendering.
- [ ] **Step 2: Write the three documents.** Check every key and command named in them
  against the code.
- [ ] **Step 3: Verify the whole build.**
  - `uv run pytest` and ruff are clean.
  - `uv build` succeeds.
  - In a clean shell: `uv tool install .`, `cs install` against a temporary
    `CLAUDE_CONFIG_DIR`, `cst` opens.
  - A search of the repository (excluding `.git`) for the owner's employer name, the dotfiles
    folder name and `/home/` finds nothing; the executor knows the exact terms.
- [ ] **Step 4: Commit** as `docs: add usage guide and install manual`.

---

## After the last task

Run `/code-review` over the repository and fix the findings that hold. Fold each fix into the
commit that introduced the code, with the fold-branch skill if more than one is involved.
Report the rejected findings with their reasons. Then hand over:

- `git log --oneline`;
- the screenshot paths;
- a pointer to the owner's migration note, kept outside the repository, as their next step.
