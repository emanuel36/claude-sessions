# Sidebar, Landing Page and Shortcuts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the tab 1 panel and the tmux status bar with a landing page at start and a
38-column sidebar on the left of every session, with shortcut scheme A and no tig.

**Architecture:** `cst` starts the server with one window running `cs landing` (Textual). A
tmux `after-new-window` hook runs `cs layout`, which splits a 37-column `cs sidebar` pane
(Textual) on the left of every new session window; tmux draws the 34th column as the pane
border. The landing page closes itself once another window exists; a sidebar kills its
window once it is the last pane in it, so closing the last session ends the server. Rendering
is pure functions returning `rich.text.Text` lines, tested against the spec's exact text; the
Textual apps only draw those lines and handle keys and clicks.

**Tech Stack:** Python ≥ 3.11, Textual, rich, tmux ≥ 3.3, pytest, ruff.

**Spec:** `docs/superpowers/specs/2026-11-03-sidebar-and-landing-design.md`

## Global Constraints

- Screen text is English, exactly as in the spec's layouts; README, code and commits stay
  English.
- Sidebar: 38 columns including the border, always, in every session window. Pane width 37:
  one padding column, 35 content columns, one padding column.
- Status marks: `!` permission, `?` question, `✓` finished, `●`/`○` working (blinking once a
  second, `●` on even seconds), `⧗` shell, blank for idle.
- Colours (rich styles, ANSI so they follow the terminal palette): mascot `color(173)`; red,
  yellow, green, blue, cyan as named ANSI; grey `color(244)`; faint `color(240)`; separator
  `color(238)`; current-session background `color(235)`; names and amounts that the spec
  calls bright white and bold: `Style(color="bright_white", bold=True)`.
- Quota bars: `█` filled, `░` empty; filled cells blue below 70 %, yellow from 70 %, red from
  85 %; empty cells faint.
- Weekday abbreviations: `Mon Tue Wed Thu Fri Sat Sun`.
- The hook's import path must not import Textual or rich (`tests/test_cli.py` already checks
  `cs` itself).
- 100-character lines, absolute imports, Google-style docstrings on non-trivial public APIs,
  `rg`/`fd` in scripts.
- Commits: Conventional Commits, subject ≤ 50 characters, body bullets, ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **A session with no status line yet** (no model, cost or context known): line 2 shows only
   the known parts and never crashes. Test in Task 3.
2. **A terminal too short for everything:** stats stay at the bottom, the empty space goes
   first, then sessions are cut from the end. Test in Task 3.
3. **A window with teammates** (two Claude panes plus the sidebar): one entry, the most urgent
   status, the sidebar still 37 columns. Test in Task 5.
4. **No usage limits known** (status line never ran): the sidebar and landing leave out the
   quota lines and still show spend. Tests in Tasks 3 and 7.
5. **A recent folder that is not a git repository or no longer exists:** not-git shows empty
   branch and state; a missing folder is not listed. Test in Task 7.

---

## File Structure

| File | Responsibility |
|---|---|
| `src/claude_sessions/ui/__init__.py` | new package for the sidebar and the landing page |
| `src/claude_sessions/ui/words.py` | English text, bars, styles shared by both screens |
| `src/claude_sessions/ui/usage.py` | totals from the journal and history: spend, lines, activity |
| `src/claude_sessions/ui/sidebar.py` | pure rendering of the sidebar column |
| `src/claude_sessions/ui/sidebar_app.py` | Textual app `cs sidebar`, data loading, clicks |
| `src/claude_sessions/ui/landing.py` | pure rendering of the landing page columns |
| `src/claude_sessions/ui/landing_app.py` | Textual app `cs landing`, data loading, keys |
| `src/claude_sessions/core/journal.py` | cost events also carry lines; window totals |
| `src/claude_sessions/claude/statusline.py` | journals lines with the cost |
| `src/claude_sessions/claude/files.py` | history sessions keep the days they were active |
| `src/claude_sessions/term/tmux.py` | sidebar layout, close window; tig, next, summary removed |
| `src/claude_sessions/term/data/tmux.conf` | scheme A keys, hooks, no status bar |
| `src/claude_sessions/term/launcher.py` | landing at start; Ghostty keybinds |
| `src/claude_sessions/cli.py` | `sidebar`, `landing`, `layout`, `close`; `panel`, `next`, `tig` removed |
| `src/claude_sessions/panel/` | deleted in Task 9 |
| `README.md` | rewritten for the new shape |

---

### Task 1: English words, bars and styles

**Files:**
- Create: `src/claude_sessions/ui/__init__.py`, `src/claude_sessions/ui/words.py`
- Test: `tests/test_words.py`

**Interfaces:**
- Produces, in `claude_sessions.ui.words`:
  - `WEEKDAYS = ("Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun")`
  - `STYLES: dict[str, Style]` with keys `ink, red, yellow, green, blue, cyan, soft, faint,
    rule, mascot, strong`, values per Global Constraints (`ink` = `Style()`, `rule` =
    `color(238)`, `strong` = bright white bold); `CURRENT = Style(bgcolor="color(235)")`.
  - `MASCOT = (" ▐▛███▜▌", "▝▜█████▛▘", "  ▘▘ ▝▝  ")`
  - `fit(text: str, width: int) -> str`, `money(amount: float | None) -> str`,
    `waiting_label(seconds: float) -> str`, `spin_frame(now: float) -> str` — moved
    unchanged from `panel/view/text.py` (that file stays until Task 9).
  - `bar(pct: float, cells: int, style: Style | None = None) -> Text`
  - `bar_style(pct: float) -> Style`
  - `reset_at(resets_at: float, now: float) -> str`
  - `ago(timestamp: float, now: float) -> str`
  - `short_model(name: str | None) -> str`
  - `thousands(n: int) -> str`

- [ ] **Step 1: Write the failing tests**

```python
NOW = time.mktime((2026, 11, 3, 14, 7, 0, 0, 0, -1))  # a Tuesday

def test_bar_fills_by_rounding_and_colours_by_level():
    b = words.bar(58, 14)
    assert b.plain == "████████░░░░░░"
    assert words.bar_style(69) == words.STYLES["blue"]
    assert words.bar_style(70) == words.STYLES["yellow"]
    assert words.bar_style(85) == words.STYLES["red"]
    assert words.bar(31, 16).plain == "█████░░░░░░░░░░░"

def test_reset_at_today_or_weekday():
    assert words.reset_at(NOW + 2 * 3600 + 33 * 60, NOW) == "16:40"
    friday = time.mktime((2026, 11, 6, 9, 0, 0, 0, 0, -1))
    assert words.reset_at(friday, NOW) == "Fri 09:00"

def test_ago_minutes_hours_yesterday_weekday_date():
    assert words.ago(NOW - 20, NOW) == "1m"
    assert words.ago(NOW - 2 * 3600, NOW) == "2h"
    assert words.ago(NOW - 24 * 3600, NOW) == "yesterday"
    assert words.ago(NOW - 6 * 24 * 3600, NOW) == "Wed"
    assert words.ago(NOW - 9 * 24 * 3600, NOW) == "25 Oct"

def test_short_model_is_the_lowercase_family():
    assert words.short_model("Opus 5.5") == "opus"
    assert words.short_model("claude-sonnet-5-5") == "sonnet"
    assert words.short_model(None) == ""

def test_thousands_uses_a_comma():
    assert words.thousands(4210) == "4,210"
    assert words.thousands(23) == "23"
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_words.py -v`
Expected: FAIL with `ModuleNotFoundError: claude_sessions.ui`

- [ ] **Step 3: Implement `claude_sessions/ui/words.py`**

`ago`: under an hour `f"{max(1, m)}m"`; same local date `f"{h}h"`; the day before `yesterday`;
under 7 days the weekday abbreviation; else the date as `25 Oct`. `short_model`: drop a `claude-` prefix,
take the first word split on space or `-`, lowercase. `bar` returns the filled part in
`style or bar_style(pct)` and the empty part in `faint`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `uv run pytest tests/test_words.py -v`
Expected: PASS

- [ ] **Step 5: Commit** — `feat(ui): add English words and bars`

---

### Task 2: Usage totals: spend, lines and activity

**Files:**
- Modify: `src/claude_sessions/core/journal.py` (add `window_totals`, keep `spend_by_session`
  as a thin wrapper over it)
- Modify: `src/claude_sessions/claude/statusline.py:150-165` (journal `added`/`removed` with
  the cost)
- Modify: `src/claude_sessions/claude/files.py:23-58` (`HistorySession.days`)
- Create: `src/claude_sessions/ui/usage.py`
- Test: `tests/test_journal.py`, `tests/test_statusline.py`, `tests/test_files.py`,
  `tests/test_usage.py`

**Interfaces:**
- Consumes: `journal.DayReader`, `files.load_history`.
- Produces:
  - `journal.window_totals(day_events, first_day) -> dict[str, dict]` — per session
    `{"cost": float, "added": int, "removed": int}`, each the growth from the last value
    before `first_day` to the highest value from it on (the rule `spend_by_session` uses).
  - `HistorySession.days: set[datetime.date]` — local dates of the session's prompts.
  - `usage.Totals` dataclass: `today: float`, `week: float`,
    `by_project: list[tuple[str, float]]`, `added: int`, `removed: int`, `sessions: int`,
    `per_session: dict[str, float]`.
  - `usage.Reader()` with `totals(history, now) -> Totals` (caches `DayReader`s as
    `PanelData.spend` does) and `usage.activity(history, now, days=14) -> list[int]`
    (sessions per day, oldest first, today last).

- [ ] **Step 1: Write the failing tests**

```python
def test_cost_events_carry_lines(...):  # tests/test_statusline.py
    # record_usage with cost 1.0, total_lines_added 10, total_lines_removed 2
    assert last_journal_event == {"t": NOW, "id": SID, "cost": 1.0, "added": 10, "removed": 2}

def test_window_totals_grow_from_before_the_window():  # tests/test_journal.py
    events = {D0: [{"id": "a", "cost": 1.0, "added": 5, "removed": 1}],
              D1: [{"id": "a", "cost": 3.0, "added": 25, "removed": 4},
                   {"id": "b", "cost": 0.5}]}
    assert journal.window_totals(events, first_day=D1) == {
        "a": {"cost": 2.0, "added": 20, "removed": 3},
        "b": {"cost": 0.5, "added": 0, "removed": 0},
    }

def test_history_keeps_the_days_a_session_was_active():  # tests/test_files.py
    s = files.read_history([prompt("a", ts(2026, 11, 1)), prompt("a", ts(2026, 11, 3))])[0]
    assert s.days == {date(2026, 11, 1), date(2026, 11, 3)}

def test_activity_counts_sessions_per_day_oldest_first():  # tests/test_usage.py
    counts = usage.activity([session(days={TODAY, TODAY - 1}), session(days={TODAY})], NOW)
    assert len(counts) == 14 and counts[-1] == 2 and counts[-2] == 1 and counts[0] == 0

def test_totals_today_week_projects_and_lines():  # tests/test_usage.py
    t = usage.Reader().totals(history, NOW)  # journal written under tmp state
    assert (t.today, t.week) == (2.0, 3.5)
    assert t.by_project == [("api", 3.0), ("web", 0.5)]
    assert (t.added, t.removed, t.sessions) == (20, 3, 2)
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_journal.py tests/test_statusline.py tests/test_files.py tests/test_usage.py -v`
Expected: FAIL on the new tests only.

- [ ] **Step 3: Implement**

`record_usage` adds `added`/`removed` from `record["usage"]` to the event it already writes.
`window_totals` generalises `spend_by_session` to the three keys (missing keys count as 0).
`read_history` adds `datetime.date.fromtimestamp(ts)` to `days`. `Reader.totals` reads the
last 7 days plus the day before (as `PanelData.spend`); `today` uses `first_day=today`;
`by_project` groups `per_session` by the history session's folder basename, largest first,
costs > 0 only; `sessions` counts sessions with any cost or lines in the week.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `uv run pytest -q`
Expected: all pass.

- [ ] **Step 5: Commit** — `feat(ui): total spend, lines and activity`

---

### Task 3: Sidebar rendering

**Files:**
- Create: `src/claude_sessions/ui/sidebar.py`
- Test: `tests/test_sidebar.py`

**Interfaces:**
- Consumes: `words.*` (Task 1), `state.TIMED`, `state.since`.
- Produces, in `claude_sessions.ui.sidebar`:
  - `WIDTH = 37`, `CONTENT = 35`
  - `@dataclass Entry`: `number: int, window_id: str, name: str, status: str, since: float,
    model: str | None = None, branch: str = "", context: float | None = None,
    cost: float | None = None`
  - `entry_lines(entry: Entry, current: bool, now: float) -> list[Text]` — two lines of
    `WIDTH` cells
  - `stats_lines(limits: dict, today: float, now: float) -> list[Text]`
  - `column(entries: list[Entry], current: str, limits: dict, today: float, now: float,
    height: int) -> list[Text]` — exactly `height` lines of `WIDTH` cells; also returns
    nothing else. `column_rows(...)` is not needed: the app maps a click's `y` with
    `entry_at(entries, y) -> Entry | None`, also in this module.

- [ ] **Step 1: Write the failing tests**

Build the spec's five sample entries (`web` permission waited 180 s, `vehicle.k265` working,
`api` shell waited 360 s and current, `docs` done, `notes` idle with no branch), limits 58 %
5h resetting at 16:40 today and 31 % 7d resetting Friday 09:00, today $9.59, `NOW` on an even
second.

```python
def test_column_matches_the_spec_text():
    lines = [l.plain for l in sidebar.column(ENTRIES, "@3", LIMITS, 9.59, NOW, height=30)]
    assert all(len(l) == 37 for l in lines)
    body = [l[1:36].rstrip() for l in lines]
    assert body[:16] == [
        " " * 13 + " ▐▛███▜▌", " " * 13 + "▝▜█████▛▘", " " * 13 + "  ▘▘ ▝▝", "",
        "! 1 web                          3m", "    opus · feat/login · 42% · $3.10",
        "● 2 vehicle.k265", "    opus · VP-562 · 18% · $0.90",
        "⧗ 3 api                          6m", "    sonnet · main · 65% · $5.42",
        "✓ 4 docs", "    haiku · main · 9% · $0.12",
        "  5 notes", "    sonnet · — · 5% · $0.05", "", "",
    ]
    assert body[-8:] == [
        "───────────────────────────────────", "5h  ████████░░░░░░ 58%", "    resets 16:40",
        "7d  ████░░░░░░░░░░ 31%", "    resets Fri 09:00", "today  $9.59",
        "alt ↑↓ session · n new", "alt c close · r rename",
    ]

def test_current_entry_has_the_background_and_strong_name():
    first, second = sidebar.entry_lines(ENTRIES[2], current=True, now=NOW)
    assert first.style == words.CURRENT and second.style == words.CURRENT
    assert any(s.style == words.STYLES["strong"] for s in first.spans)

def test_waiting_time_shows_only_for_timed_statuses_from_one_minute():
    done = sidebar.entry_lines(entry("done", waited=600), False, NOW)[0].plain
    assert "10m" not in done
    fresh = sidebar.entry_lines(entry("permission", waited=30), False, NOW)[0].plain
    assert fresh.strip() == "! 1 web"

def test_unknown_usage_leaves_parts_out():  # Review Focus 1
    lines = sidebar.entry_lines(sidebar.Entry(1, "@1", "new", "working", NOW), False, NOW)
    assert lines[1].plain.strip() == "—"

def test_short_terminal_keeps_stats_and_cuts_sessions():  # Review Focus 2
    lines = [l.plain for l in sidebar.column(ENTRIES, "@3", LIMITS, 9.59, NOW, height=16)]
    assert len(lines) == 16 and "alt c close" in lines[-1] and "! 1 web" in lines[4]
    assert not any("notes" in l for l in lines)

def test_no_limits_leaves_quota_out():  # Review Focus 4
    lines = [l.plain.strip() for l in sidebar.column(ENTRIES, "@3", {}, 0.0, NOW, height=30)]
    assert "today  $0.00" in lines and not any(l.startswith("5h") for l in lines)

def test_entry_at_maps_rows_to_entries():
    assert sidebar.entry_at(ENTRIES, 4).name == "web"
    assert sidebar.entry_at(ENTRIES, 5).name == "web"
    assert sidebar.entry_at(ENTRIES, 6).name == "vehicle.k265"
    assert sidebar.entry_at(ENTRIES, 0) is None
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_sidebar.py -v`
Expected: FAIL with `ModuleNotFoundError`

- [ ] **Step 3: Implement `claude_sessions/ui/sidebar.py`**

Rules from the spec "Sidebar rules" and "Colours": line 1 is `mark number fit(name, 14)` with
the waiting label (`words.waiting_label`, only for `state.TIMED`) right-aligned to column 35;
line 2 is four spaces and `short_model · fit(branch or "—", 12) · round(context)% · money`,
leaving out unknown model, context and cost. Mascot lines are left-padded by
`(CONTENT - 9) // 2` columns. Height order: mascot (3) + blank + entries, then empty filler,
then stats (separator, quota pairs, `today`, two key lines). When short, drop filler first,
then whole entries from the end. Each line is padded with one column on each side.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `uv run pytest tests/test_sidebar.py -v`
Expected: PASS

- [ ] **Step 5: Commit** — `feat(ui): render the sidebar`

---

### Task 4: Sidebar app

**Files:**
- Create: `src/claude_sessions/ui/sidebar_app.py`
- Modify: `src/claude_sessions/cli.py` (add `cs sidebar`, lazy import as `cmd_panel` does)
- Modify: `src/claude_sessions/term/tmux.py` (add `window_entries`)
- Test: `tests/test_sidebar_app.py`, `tests/test_cli.py`

**Interfaces:**
- Consumes: `sidebar.column`, `sidebar.entry_at`, `sidebar.Entry` (Task 3);
  `usage.Reader.totals` (Task 2); `hook.live_sessions`, `tmux.windows_of`,
  `statusline.load_limits`, `loader.branch_of` (moved to `ui/sidebar_app.py` in Task 9).
- Produces:
  - `tmux.window_entries() -> list[tuple[str, int, str]]` — `(window_id, index, name)` of the
    server's windows, by index.
  - `sidebar_app.load(now) -> tuple[list[Entry], dict, float]` — entries in window order,
    limits, today's spend. A window with no live record is left out (the landing page).
  - `sidebar_app.SidebarApp(load=load, window_id=None)` — `window_id` defaults to the window
    of `$TMUX_PANE`.

- [ ] **Step 1: Write the failing tests**

```python
def test_app_draws_the_column_and_highlights_its_window():
    app = sidebar_app.SidebarApp(load=lambda now: (ENTRIES, LIMITS, 9.59), window_id="@3")
    # run_test(size=(37, 30)); screen line 8 contains "⧗ 3 api" on the current background

def test_click_selects_that_window(monkeypatch):
    # click at y=6 → tmux.run called with ("select-window", "-t", "@2")

def test_last_pane_left_kills_its_window(monkeypatch):
    # tmux.run("display-message", ...) returns "1" panes → ("kill-window", "-t", "@3") called

def test_load_orders_by_window_index_and_skips_windows_without_sessions(monkeypatch):
    # window_entries: [("@1", 1, "api"), ("@4", 2, "web")]; records only for @4
    # → [Entry(number=2, window_id="@4", name="web", ...)]

def test_sidebar_command_runs_the_app(monkeypatch):  # tests/test_cli.py
    # cs sidebar calls sidebar_app.main()
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_sidebar_app.py tests/test_cli.py -v`
Expected: FAIL on the new tests.

- [ ] **Step 3: Implement**

The app has one `Static` filling the screen, redrawn by a one-second timer chain aligned to
the second (as `PanelApp.tick`). Each tick: `load`, then `column(..., height=self.size.height)`;
then if the window has one pane left, `kill-window` on it. A failed load keeps the last
lines and logs once (as `PanelApp.reload`). No key bindings; `ctrl+c`/`ctrl+q` do nothing.
`load` builds entries from `tmux.windows_of(records)` (one per window, lead = most urgent)
joined with `window_entries()` for number and name; usage from the lead's `usage`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `uv run pytest -q`
Expected: all pass.

- [ ] **Step 5: Commit** — `feat(ui): add the sidebar app`

---

### Task 5: Sidebar layout in tmux

**Files:**
- Modify: `src/claude_sessions/term/tmux.py` (add `add_sidebar`, `fit_sidebars`)
- Modify: `src/claude_sessions/cli.py` (add `cs layout [WINDOW_ID]`)
- Modify: `src/claude_sessions/term/data/tmux.conf` (hooks, mouse binding, borders)
- Test: `tests/test_tmux.py` (throwaway server), `tests/test_tmux_conf.py`

**Interfaces:**
- Consumes: none from earlier tasks besides `cs sidebar` existing.
- Produces:
  - `tmux.SIDEBAR_WIDTH = 37`, pane option `@cs-sidebar` set to `1` on sidebar panes.
  - `tmux.add_sidebar(window_id: str, command: str = "cs sidebar") -> None` — splits a pane
    of `SIDEBAR_WIDTH` columns on the far left with `split-window -hbd -f -l 37`, marks it,
    and leaves the other pane active; does nothing if the window already has one.
  - `tmux.fit_sidebars() -> None` — resizes every `@cs-sidebar` pane back to 37 columns.

- [ ] **Step 1: Write the failing tests**

```python
def test_add_sidebar_puts_a_33_column_pane_on_the_left(server):
    tmux.add_sidebar(window_of(server, 2), command="sleep 600")
    panes = list_panes(server, 2, "#{pane_left}\t#{pane_width}\t#{@cs-sidebar}\t#{pane_active}")
    assert panes[0] == "0\t33\t1\t0" and panes[1].endswith("\t\t1")

def test_add_sidebar_twice_adds_one(server): ...

def test_teammate_split_keeps_the_sidebar_at_33(server):  # Review Focus 3
    tmux.add_sidebar(window_of(server, 2), command="sleep 600")
    split_right(server, 2)  # a second Claude pane
    tmux.fit_sidebars()
    assert sidebar_width(server, 2) == 37

def test_conf_hooks_and_mouse(run):  # tests/test_tmux_conf.py
    hooks = run("show-hooks", "-g")
    assert "after-new-window" in hooks and "cs layout" in hooks
    assert "client-resized" in hooks and "after-split-window" in hooks
    assert "@cs-sidebar" in run("list-keys", "-T", "root", "MouseDown1Pane")
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_tmux.py tests/test_tmux_conf.py -v`
Expected: FAIL on the new tests.

- [ ] **Step 3: Implement**

tmux.conf: `set-hook -g after-new-window 'run-shell "cs layout #{window_id}"'`;
`after-split-window` and `client-resized` run `cs layout` (no argument = `fit_sidebars`).
`MouseDown1Pane` on a `@cs-sidebar` pane sends the click without selecting the pane
(`if -F "#{@cs-sidebar}" { send-keys -M } { select-pane -t = ; send-keys -M }`), so the
sidebar never takes keyboard focus. `pane-border-style` and `pane-active-border-style`
`fg=colour238`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `uv run pytest -q`
Expected: all pass.

- [ ] **Step 5: Commit** — `feat(term): add the sidebar to every session`

---

### Task 6: Scheme A keys, closing, no status bar, no tig

**Files:**
- Modify: `src/claude_sessions/term/data/tmux.conf` (keys; status off; tab formats and
  `@cs-*` status options removed; Alt+g removed)
- Modify: `src/claude_sessions/term/tmux.py` (add `close_window`; remove `toggle_tig`,
  `next_pending`, `summary_text`, `publish_summary`, `SUMMARY`)
- Modify: `src/claude_sessions/cli.py` (add `cs close WINDOW_ID`; remove `next`, `tig`)
- Modify: `src/claude_sessions/term/launcher.py:17-27` (Ghostty keybinds)
- Modify: `src/claude_sessions/panel/app.py` (drop `publish_summary`; the panel goes in
  Task 9)
- Test: `tests/test_tmux_conf.py`, `tests/test_tmux.py`, `tests/test_cli.py`,
  `tests/test_launcher.py`

**Interfaces:**
- Consumes: `tmux.attaches`, `tmux._cmdline`, `tmux._children` (existing).
- Produces: `tmux.close_window(window_id: str) -> None` — runs `claude stop <id>` for every
  pane that runs `claude attach <id>`, then `kill-window`.

- [ ] **Step 1: Write the failing tests**

```python
def test_scheme_a_keys(run):  # tests/test_tmux_conf.py
    keys = run("list-keys", "-T", "root")
    assert "M-Up previous-window" in keys and "M-Down next-window" in keys
    assert "M-n display-popup" in keys and "cs new --pick" in keys
    assert "M-c confirm-before" in keys and "close #{window_name}? (y/n)" in keys
    assert "cs close #{window_id}" in keys
    for gone in ("M-Left", "M-Right", "M-0", "M-,", "M-.", "M-g", "cs next"):
        assert gone not in keys

def test_status_bar_is_off(run):
    assert run("show", "-gv", "status").strip() == "off"

def test_close_window_stops_attached_background_sessions(monkeypatch):  # tests/test_tmux.py
    # panes of @2: pid 100 runs ["claude", "attach", "685e0c8a"]
    # → subprocess "claude stop 685e0c8a" called, then ("kill-window", "-t", "@2")

def test_ghostty_leaves_alt_left_right_and_unbinds_alt_up_down():  # tests/test_launcher.py
    cmd = launcher.ghostty_command("/bin/cst")
    assert "--keybind=alt+up=unbind" in cmd and "--keybind=alt+down=unbind" in cmd
    assert "--keybind=alt+1=unbind" in cmd
    assert not any("alt+left" in a or "alt+right" in a for a in cmd)
```

Delete `test_next_*`, `test_tig_*` and the tab-mark tests in `test_tmux_conf.py`.

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_tmux_conf.py tests/test_tmux.py tests/test_cli.py tests/test_launcher.py -v`
Expected: FAIL on the new tests.

- [ ] **Step 3: Implement**

Keys: `M-Up previous-window`, `M-Down next-window`, `M-1`…`M-9 select-window -t :N`,
`M-n` the popup now on `M-c`, `M-c confirm-before -p "close #{window_name}? (y/n)"
"run-shell 'cs close #{window_id}'"`, `M-r` as today with the prompt ` Session name:`.
`set -g status off`. `claude stop` runs through `subprocess.run(..., timeout=10)` and its
failure is ignored, so the window still closes.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `uv run pytest -q`
Expected: all pass.

- [ ] **Step 5: Commit** — `feat(term): move to the new shortcuts`

---

### Task 7: Landing page rendering

**Files:**
- Create: `src/claude_sessions/ui/landing.py`
- Test: `tests/test_landing.py`

**Interfaces:**
- Consumes: `words.*` (Task 1), `usage.Totals` (Task 2).
- Produces, in `claude_sessions.ui.landing`:
  - `@dataclass Recent`: `session_id: str, project: str, title: str, cost: float | None,
    last: float, cwd: str`
  - `@dataclass Folder`: `path: str` (with `~`), `branch: str`, `changed: int | None`
    (`None` = not a git repository)
  - `left_lines(recent, folders, selected: int, search: str | None, now) -> list[Text]` —
    `selected` indexes recent rows, then folder rows, then `other folder…`
  - `right_lines(limits: dict, totals: Totals, activity: list[int], first_day: date,
    now) -> list[Text]`
  - `FOOTER = "↑↓ choose · Enter resume · n new session · / search · q quit"`
  - `screen(left, right, width: int) -> list[Text]` — side by side with a 4-column gap when
    `width ≥ LEFT_WIDTH + 4 + RIGHT_WIDTH`, stacked otherwise, then a blank line and the
    footer. `LEFT_WIDTH = 66`, `RIGHT_WIDTH = 43` (the widest right row at the sample data).

- [ ] **Step 1: Write the failing tests**

Use the spec's sample data exactly (five recent sessions, four folders, the spend, activity
`[2, 4, 3, 0, 0, 5, 6, 4, 3, 5, 0, 1, 4, 3]` from 20/10, lines 4210/1388 in 23 sessions).

```python
def test_left_column_matches_the_spec():
    lines = [l.plain.rstrip() for l in landing.left_lines(RECENT, FOLDERS, 0, None, NOW)]
    assert lines == [
        " ▐▛███▜▌", "▝▜█████▛▘", "  ▘▘ ▝▝    claude-sessions", "",
        "Recent sessions  / search",
        "› api              refatora o cliente HTTP para … $5.42  2h",
        "  vehicle.k265     comunicação com tablet (VP-56… $8.10  5h",
        "  web              corrige o fluxo de login com … $3.10  yesterday",
        "  claude-sessions  estado para comando shell em … $4.77  yesterday",
        "  docs             atualiza o guia de instalação  $0.42  Wed",
        "", "New session in…  n",
        "  ~/workspace/api              main       3 changed",
        "  ~/workspace/vehicle.k265     VP-562     clean",
        "  ~/workspace/web              feat/login 1 changed",
        "  ~/Documents/claude-sessions  main       clean",
        "  other folder…",
    ]

def test_right_column_matches_the_spec():
    lines = [l.plain.rstrip() for l in landing.right_lines(LIMITS, TOTALS, ACTIVITY, D20_10, NOW)]
    assert lines == [
        "", "Usage", "5h   █████████░░░░░░░ 58%  resets 16:40",
        "7d   █████░░░░░░░░░░░ 31%  resets Fri 09:00", "today $9.59   7 days $84.30",
        "", "Spend 7d by project",
        "api              ██████████████ $31.20", "vehicle.k265     ██████████░░░░ $22.75",
        "web              █████░░░░░░░░░ $12.04", "claude-sessions  █████░░░░░░░░░ $10.90",
        "docs             █░░░░░░░░░░░░░ $2.10",
        "", "Sessions per day (14 days)", "▄ ▆ ▅     ▇ █ ▆ ▅ ▇   ▃ ▆ ▅",
        "20 Oct" + " " * 16 + "today",
        "", "Week", "+4,210 -1,388 lines in 23 sessions",
    ]

def test_selection_moves_to_folders_and_other_folder():
    lines = [l.plain for l in landing.left_lines(RECENT, FOLDERS, 9, None, NOW)]
    assert lines[-1].startswith("› other folder…")

def test_search_replaces_the_hint():
    lines = [l.plain for l in landing.left_lines(RECENT[:1], FOLDERS, 0, "api", NOW)]
    assert lines[4].startswith("Recent sessions  / api")

def test_not_a_git_folder_shows_no_branch_or_state():  # Review Focus 5
    lines = landing.left_lines([], [landing.Folder("~/notes", "", None)], 0, None, NOW)
    assert lines[-2].plain.rstrip() == "  ~/notes"

def test_no_limits_leaves_quota_out():  # Review Focus 4
    lines = [l.plain for l in landing.right_lines({}, TOTALS, ACTIVITY, D20_10, NOW)]
    assert not any(l.startswith("5h") for l in lines) and any("today $9.59" in l for l in lines)
```

Sessions-per-day blocks: index `min(8, n + 2)` into `" ▁▂▃▄▅▆▇█"`, blank for 0, joined by
single spaces (27 columns); the date line is the first day as `25 Oct` (fixed English month abbreviations), then `today`
right-aligned to column 27. Selected rows get `›` and the strong style; first prompts grey;
times faint; branches cyan; git states faint; project bars blue; amounts strong; blocks
green; `+N` green and `-N` red; headings grey; hints and footer faint.

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_landing.py -v`
Expected: FAIL with `ModuleNotFoundError`

- [ ] **Step 3: Implement `claude_sessions/ui/landing.py`** per the spec's "Landing rules".

- [ ] **Step 4: Run the tests to verify they pass**

Run: `uv run pytest tests/test_landing.py -v`
Expected: PASS

- [ ] **Step 5: Commit** — `feat(ui): render the landing page`

---

### Task 8: Landing app and start-up

**Files:**
- Create: `src/claude_sessions/ui/landing_app.py`
- Modify: `src/claude_sessions/cli.py` (add `cs landing`)
- Modify: `src/claude_sessions/term/launcher.py:12-15,52-72` (`LANDING = "cs landing"`
  replaces the panel loop)
- Test: `tests/test_landing_app.py`, `tests/test_launcher.py`, `tests/test_cli.py`

**Interfaces:**
- Consumes: `landing.*` (Task 7), `usage.Reader`, `usage.activity` (Task 2),
  `files.load_history`, `files.recent_projects`, `files.Titles`, `filter_rows`.
- Produces: `landing_app.LandingApp(load=load)`; `landing_app.load(now)` returning
  `(recent: list[Recent], folders: list[Folder], limits, totals, activity, first_day)`.
  Recent: 5 newest sessions with a title, not open; folders: 4 newest existing folders.

- [ ] **Step 1: Write the failing tests**

```python
def test_enter_on_recent_resumes_in_a_new_window(monkeypatch):
    # Enter → tmux.run("new-window", "-c", "/w/api", "claude --resume sid-api")

def test_n_on_a_folder_starts_a_new_session_there(monkeypatch):
    # down ×5 (first folder), n → tmux.run("new-window", "-c", "/home/u/workspace/api", "claude")

def test_n_on_a_recent_session_starts_in_its_folder(monkeypatch): ...

def test_enter_on_other_folder_opens_the_picker(monkeypatch):
    # → tmux.new_session_popup() called

def test_q_ends_the_server(monkeypatch):
    # q → tmux.run("kill-server")

def test_closes_itself_once_another_window_exists(monkeypatch):
    # window count "2" on a tick → tmux.run("kill-window", "-t", own window)

def test_slash_filters_recent_sessions(monkeypatch): ...

def test_launcher_starts_the_landing_page(monkeypatch):  # tests/test_launcher.py
    # new-session ... "cs landing"; attach afterwards; existing session → attach only
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_landing_app.py tests/test_launcher.py tests/test_cli.py -v`
Expected: FAIL on the new tests.

- [ ] **Step 3: Implement**

The app draws `landing.screen(...)` in one `Static`, ticks every second like the sidebar, and
handles ↑/↓ (also k/j), Enter, `n`, `/` (typing, Backspace, Enter keeps the filter, Esc
clears), `q`. Opening only creates the window; the tick sees a second window and kills the
landing window, so the popup path closes it too. Git state per folder: branch from
`git branch --show-current`, `changed` from `git status --porcelain` line count, `None` when
git fails; cached 5 s per folder like `branch_of`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `uv run pytest -q`
Expected: all pass.

- [ ] **Step 5: Commit** — `feat(ui): open on the landing page`

---

### Task 9: Remove the panel and rewrite the README

**Files:**
- Delete: `src/claude_sessions/panel/` (move `branch_of` and `filter_rows` and their tests
  first: `branch_of` to `ui/sidebar_app.py`, `filter_rows` to `ui/landing_app.py`)
- Delete: `tests/test_app.py`, `tests/test_layout.py`, `tests/test_loader.py` (after moving
  the two functions' tests), `tests/test_text.py` (after moving the tests of the helpers that
  now live in `ui/words.py`)
- Modify: `src/claude_sessions/cli.py` (remove `cs panel`)
- Modify: `src/claude_sessions/claude/install.py` (drop tig mentions)
- Modify: `README.md`, `install.sh` (tig note)
- Modify: `pyproject.toml` package data if it lists `panel.tcss`

- [ ] **Step 1: Check nothing imports the panel**

Run: `rg -n "claude_sessions.panel|cs panel|tig" src tests install.sh pyproject.toml`
Expected after the edits: no matches.

- [ ] **Step 2: Rewrite the README** for users: what it is; the landing page; the sidebar and
  the statuses table (Shell included); the shortcuts table of scheme A with Alt+←/→ left to
  Ghostty splits; get started. No code, links to remote repos only. Replace
  `docs/screenshots/panel.svg` with a sidebar screenshot or drop the image.

- [ ] **Step 3: Run everything**

Run: `uv run ruff check . && uv run ruff format --check . && uv run pytest -q`
Expected: all checks pass, all tests pass, `ls /tmp/tmux-$(id -u)` shows no leftover
`cs-test-*` servers.

- [ ] **Step 4: Commit** — `refactor: drop the panel and tig`

---

### Task 10: Try it for real

- [ ] **Step 1:** `uv tool install --force --reinstall --python '>=3.11' .` then
  `tmux -L claude kill-server` (after asking the owner: it closes their open sessions) and
  `cst --ghostty`.
- [ ] **Step 2:** Check against the spec: landing text and colours; opening a session closes
  the landing and adds the 38-column sidebar; Alt+↑/↓, Alt+1…5, Alt+n, Alt+r, Alt+c with
  `close …? (y/n)`; clicking a sidebar entry; Alt+←/→ move between Ghostty splits; closing
  the last session ends `cst`.
- [ ] **Step 3:** Run `/code-review`, fix what holds, report what was rejected and why.
