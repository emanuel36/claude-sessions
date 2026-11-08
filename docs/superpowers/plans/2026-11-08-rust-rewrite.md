# Rust Rewrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Python claude-sessions with one static Rust binary, `cst`, that looks and
behaves the same and costs a fraction of the CPU and memory.

**Architecture:** A sync hot path (`cst hook`, `cst statusline`) that locks, reads, applies a pure
state machine and writes atomically. UI panes (sidebar, landing page, Alt+n panel) are ratatui
apps driven by one channel fed by an input thread and an inotify thread, with a pure scheduler
deciding the next wake-up; tmux tells panes when they are shown, hidden or re-laid out.

**Tech Stack:** Rust 2021 (rustc 1.92), ratatui 0.29 + crossterm 0.28, serde/serde_json,
inotify 0.11, rustix (fs, process), chrono, unicode-width, sha2; dev: proptest, insta, tempfile.
Releases: GitHub Actions, musl targets.

**Spec:** `docs/superpowers/specs/2026-11-08-rust-rewrite-design.md`. The behavioural reference
is the Python code at commit `937dfa7` (`src/claude_sessions/`, `tests/`): "port X" means
reproduce what that file does, edge cases included, and port the named Python tests.

## Global Constraints

- Linux only; targets `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`.
- Crate and binary name `cst`, version `2.0.0`.
- `rustfmt.toml`: `max_width = 100`. Imports by `crate::` path, never `super::`.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` pass at every
  commit.
- No async runtime. No `unwrap`/`expect`/panic on the hook or status line path; errors are
  `Result` (use `anyhow` only in `main.rs` and command entry points, typed errors elsewhere).
- `cst hook` and `cst statusline` always exit 0; failures go to the error log.
- Rustdoc on non-trivial public items; Google-style sections (`# Errors`) where they apply.
- Tests never touch `tmux -L claude`; tmux tests use `tmux -L cst-test-<uuid>` and kill it.
- Tests point `CST_RUNTIME_DIR`, `CST_STATE_DIR` and `CLAUDE_CONFIG_DIR` at a temp dir.
- Conventional Commits, subject ≤ 50 chars, body lines ≤ 72, each commit ends with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push.
- On-disk paths: records and limits in `$CST_RUNTIME_DIR` or `$XDG_RUNTIME_DIR/cst` or
  `/tmp/cst-<uid>` (0700, refuse symlink, foreign owner or group/other-writable); everything
  else in `$CST_STATE_DIR` or `$XDG_STATE_HOME/cst` or `~/.local/state/cst`.
- tmux: socket `-L claude`, session `claude`, sidebar width 29 (pane) / content 27, tmux call
  timeout 1 s, hook timeout 10 s, status line chain timeout 5 s.
- Signal keys sent by tmux to sidebar and landing panes: F12 shown, F11 hidden, F10 layout.

## Review Focus

1. **A user's `settings.json` with other hooks, odd shapes or a symlink** → install and
   uninstall keep everything not ours byte-for-byte in meaning, refuse malformed files with
   nothing written (Task 10).
2. **Parallel tool calls firing hooks for one session at once** → no update lost, journal lines
   never torn (Task 5, Task 8).
3. **Malformed, empty or unexpected hook input** (non-JSON, missing `session_id`, unsafe id like
   `../x`, unknown event, `background_tasks` not a list) → exit 0, nothing written outside the
   records dir, error logged (Task 8).
4. **Files caught mid-write** (a journal or history line with no newline yet, a torn line, a file
   truncated or replaced) → incremental readers skip or restart, never crash (Task 4, Task 6).
5. **Wide characters and tiny terminals** (CJK or emoji in names and titles, a 5-line sidebar,
   a 30-column landing page) → text is cut by cell width with `…`, nothing overflows the pane,
   no panic (Task 12, Task 14, Task 16).

---

## File Structure

```
Cargo.toml  rustfmt.toml  .cargo/config.toml (none: no global env)
.github/workflows/ci.yml          fmt, clippy, test on push/PR
.github/workflows/release.yml     tag v* → musl binaries + .sha256 on a release
install.sh                        download, verify, install, register
assets/tmux.conf                  embedded with include_str!
bench/run.sh                      performance check against the spec's Goals
src/main.rs                       argument dispatch (hand-rolled, hook/statusline first)
src/lib.rs                        module declarations
src/paths.rs                      dirs, atomic write, error log
src/state.rs                      types + pure state machine
src/store.rs                      records on disk: lock, read, write, sweep
src/journal.rs                    day files: append, incremental reader, totals
src/claude/history.rs             history.jsonl incremental reader
src/claude/transcript.rs          transcript path, tail, interrupt, titles
src/claude/hook.rs                handle_event, live_sessions, settle, focus/seen
src/claude/statusline.rs          usage, limits, chain
src/claude/install.rs             settings.json merge/unmerge, migration
src/term/tmux.rs                  Runner trait, Cli runner, pane/window queries and actions
src/term/launcher.rs              open the server, --ghostty, conf file
src/term/layout.rs                layout/step/close/focus commands
src/update.rs                     self-update from releases
src/ui/line.rs                    Line helpers: cell_len, pad_right, truncate, fit
src/ui/words.rs                   formats, styles, mascot, bars
src/ui/usage.rs                   journal totals for the screens
src/ui/sidebar.rs                 sidebar column rendering
src/ui/picker.rs                  folder list + Alt+n panel rendering, Picker state
src/ui/folders.rs                 folders from history + titles + costs
src/ui/landing.rs                 landing page rendering
src/ui/runtime.rs                 event channel, threads, Schedule
src/ui/sidebar_app.rs  src/ui/landing_app.rs  src/ui/panel_app.rs
tests/support/mod.rs              temp env, private tmux server, binary runner
tests/golden/fixtures.json        golden inputs   tests/golden/expected/*.json  outputs
tests/golden.rs                   screen parity
tools/golden.py                   one-off Python generator (removed in Task 23)
```

---

### Task 1: Crate scaffold and CI

**Files:**
- Create: `Cargo.toml`, `rustfmt.toml`, `src/main.rs`, `src/lib.rs`,
  `.github/workflows/ci.yml`, `tests/support/mod.rs`, `tests/cli.rs`
- Modify: `.gitignore` (add `/target`)

**Interfaces:**
- Produces: `tests/support`: `fn env() -> TestEnv` (temp dir with the three env vars set for
  spawned commands), `fn cst(env: &TestEnv, args: &[&str], stdin: &[u8]) -> Output` (runs
  `env!("CARGO_BIN_EXE_cst")`), `struct PrivateTmux { fn new() -> Option<Self>; fn run(&self,
  args: &[&str]) -> String }` killing its server and socket file on drop, `None` without tmux.
- Produces: `main.rs` dispatch: `--version`, `--help` (public commands only), unknown command →
  usage on stderr, exit 2.

- [ ] **Step 1:** Commit the spec and this plan first:
  `git add docs/superpowers && git commit -m "docs: add Rust rewrite design and plan"`.
- [ ] **Step 2: Failing tests** in `tests/cli.rs`: `version_prints_2_0_0` (stdout
  `cst 2.0.0\n`), `help_lists_only_public_commands` (contains `update`, `uninstall`,
  `--ghostty`; does not contain `hook`, `statusline`, `sidebar`), `unknown_command_exits_2`.
- [ ] **Step 3:** Run `cargo test --test cli` → fails (no crate).
- [ ] **Step 4:** Write `Cargo.toml` (deps from Tech Stack; `[profile.release]` `lto = true`,
  `codegen-units = 1`, `panic = "abort"`, `strip = true`, `opt-level = 3`), `rustfmt.toml`,
  `main.rs`, `lib.rs`, `tests/support/mod.rs`, CI workflow (ubuntu-latest, tmux installed,
  `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`).
- [ ] **Step 5:** `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
  → all pass.
- [ ] **Step 6:** Commit `build: scaffold the cst crate`.

### Task 2: Paths, atomic writes, error log

**Files:** Create `src/paths.rs`; Test: unit tests in the module.

**Interfaces:**
- Produces: `pub fn runtime_dir() -> io::Result<PathBuf>` (creates 0700; fallback checks),
  `pub fn state_dir() -> PathBuf`, `pub fn config_dir() -> PathBuf` (`CLAUDE_CONFIG_DIR` or
  `~/.claude`), `pub fn write_atomic(path: &Path, data: &[u8], mode: Option<u32>) ->
  io::Result<()>` (temp in same dir, keeps the old file's mode when `None`, no temp left on
  error), `pub fn log_error(what: &dyn std::fmt::Display)` (never fails; `errors.log` in
  `state_dir()`, truncated once past 1 MiB, opened with `O_NOFOLLOW`, line
  `YYYY-mm-dd HH:MM:SS <what>`). Constants `LIMITS_FILE = "limits.json"`.
- Port: `core/paths.py`, `tests/test_paths.py`.

- [ ] **Step 1: Failing tests:** the ported `test_paths.py` cases, plus
  `fallback_refuses_a_symlink`, `fallback_refuses_group_writable`,
  `write_atomic_keeps_mode_of_replaced_file`, `log_error_truncates_past_one_mib`.
- [ ] **Step 2:** `cargo test paths` → fail.
- [ ] **Step 3:** Implement with rustix/std (`OpenOptionsExt::custom_flags(O_NOFOLLOW)`).
- [ ] **Step 4:** `cargo test paths` → pass.
- [ ] **Step 5:** Commit `feat(paths): add state dirs and atomic writes`.

### Task 3: State machine

**Files:** Create `src/state.rs`.

**Interfaces:**
- Produces (all `Serialize`/`Deserialize`, `Clone`, `Debug`, `PartialEq`):
  - `enum Status { Permission, Input, Done, Working, Shell, Idle }`, serde lowercase,
    `#[serde(other)] Unknown` on deserialize sorts last. Consts `NEEDS_YOU: [Status; 2]`,
    `TIMED: [Status; 3]`, `INTERRUPTIBLE: [Status; 3]` (Working, Permission, Input).
  - `enum Waiting { Permission, Input }`; `struct Pending { agent: String, kind: Waiting }`
    (`agent` is `"main"`, a subagent id, or `"notification"`).
  - `struct Mux { socket: String, pane: String }`;
    `struct Usage { cost: Option<f64>, api_duration: Option<f64>, model: Option<String>,
    added: u64, removed: u64 }`.
  - `struct Record { id: String, status: Status, cwd: String, pid: Option<i32>,
    pid_start: Option<u64>, transcript: Option<PathBuf>, background: u32,
    pending: Vec<Pending>, status_since: f64, updated_at: f64, started_at: f64,
    mux: Option<Mux>, headless: bool, title: String, usage: Option<Usage> }`
    (`#[serde(default)]` on optional fields).
  - `struct Event { session_id: String, hook_event_name: String, agent_id, tool_name, cwd,
    transcript_path, source, reason, notification_type: Option<String>,
    background_tasks: Vec<BackgroundTask> }` with a lenient deserializer: anything not a list
    is empty, non-object items skipped. `struct BackgroundTask { status: Option<String>,
    kind: Option<String> }` (`kind` from JSON `type`).
  - `fn new_record(id: &str) -> Record`, `fn apply_event(record: Option<&Record>, event: &Event,
    now: f64) -> Record`, `fn mark_seen(r: &Record, now: f64) -> Record`,
    `fn mark_interrupted(r: &Record, now: f64) -> Record`, `fn since(r: &Record) -> f64`,
    `fn project(r: &Record) -> &str`, `fn sort_key(r: &Record) -> (u8, f64)`.
- Port: `core/state.py`, `tests/test_state.py` (all 17 tests).

- [ ] **Step 1: Failing tests:** ported tests; proptest `invariants_hold_for_any_event_sequence`
  over sequences of 1–40 events drawn from the 11 event names, agent ids `{None, "a1", "a2"}`,
  tool names `{"Bash", "AskUserQuestion", "ExitPlanMode"}`, notification types
  `{"permission_prompt", "elicitation_dialog", "idle_prompt"}`, background task lists. Assert
  after each step: no agent twice in `pending`; `status == Permission` iff some pending kind is
  Permission; `status == Input` iff pending non-empty and none is Permission; `status_since`
  changed iff status changed (or record was new); `updated_at == now`.
- [ ] **Step 2:** `cargo test state` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test state` → pass.
- [ ] **Step 5:** Commit `feat(state): port the session state machine`.

### Task 4: Journal

**Files:** Create `src/journal.rs`.

**Interfaces:**
- Consumes: `paths::state_dir`, `paths::log_error`.
- Produces: `struct JournalEvent { t: f64, id: String, status: Option<String>,
  cost: Option<f64>, added: Option<u64>, removed: Option<u64>, model: Option<String> }`
  (`status` is a `Status` name or `ENDED = "ended"`); `fn append(e: &JournalEvent)` (never
  fails; file `journal/<local date>.jsonl`, single `O_APPEND` write, prunes days older than
  `KEEP_DAYS = 7` when it creates a day file); `struct DayReader` with `fn read(&mut self,
  day: NaiveDate) -> &[JournalEvent]` and `fn generation(&self) -> u64` (bumps only when new
  events were parsed or the file reset); `type Days<'a> = BTreeMap<NaiveDate,
  &'a [JournalEvent]>`; `struct SessionTotals { cost: f64, added: u64, removed: u64 }`;
  `fn window_totals(days: &Days, first: NaiveDate) -> HashMap<String, SessionTotals>`,
  `fn model_costs(days: &Days, first: NaiveDate) -> HashMap<String, f64>` (`"unknown"` when no
  model), `fn day_costs(days: &Days) -> BTreeMap<NaiveDate, f64>`.
- Port: `core/journal.py`, `tests/test_journal.py`.

- [ ] **Step 1: Failing tests:** ported tests, plus `reader_waits_for_a_line_without_newline`,
  `reader_skips_a_torn_line`, `reader_restarts_when_the_file_shrinks_or_is_replaced`,
  `concurrent_appends_never_interleave` (8 threads × 500 appends, every line parses).
- [ ] **Step 2:** `cargo test journal` → fail.
- [ ] **Step 3:** Implement (local date via `chrono::Local`).
- [ ] **Step 4:** `cargo test journal` → pass.
- [ ] **Step 5:** Commit `feat(journal): port the day journal`.

### Task 5: Session records on disk

**Files:** Create `src/store.rs`.

**Interfaces:**
- Consumes: `paths`, `state`, `journal::append`.
- Produces: `fn is_session_id(s: &str) -> bool`; `struct Lock` (flock held until drop);
  `fn lock(id: &str) -> io::Result<Lock>`; `fn read(id: &str) -> Option<Record>`;
  `fn write(r: &Record) -> io::Result<()>` (journals a status change or a new record, at
  `since(r)`); `fn remove(id: &str, keep_lock: bool)` (journals `ended` when a record existed);
  `fn remove_replaced(r: &Record)`; `fn process_start(pid: i32) -> Option<u64>`;
  `fn is_alive(r: &Record) -> bool`; `fn load_all(now: f64) -> Vec<Record>` (drops dead
  records, stale locks older than 60 s, sorted by `sort_key`).
- Port: `core/store.py`, `tests/test_store.py`.

- [ ] **Step 1: Failing tests:** ported tests, plus `parallel_writers_lose_no_update`
  (16 threads each lock → read → append its own pending agent → write; final record has all
  16), `unsafe_ids_are_refused` (`""`, `"../x"`, `"a/b"`, `"a b"`).
- [ ] **Step 2:** `cargo test store` → fail.
- [ ] **Step 3:** Implement (`rustix::fs::flock`).
- [ ] **Step 4:** `cargo test store` → pass.
- [ ] **Step 5:** Commit `feat(store): port session records on disk`.

### Task 6: Claude Code files: history and transcripts

**Files:** Create `src/claude/mod.rs`, `src/claude/history.rs`, `src/claude/transcript.rs`.

**Interfaces:**
- Produces (history): `struct HistorySession { id, cwd: String, last: f64,
  first_prompt: String }`; `struct HistoryReader` with `fn new(stored: bool) -> Self`,
  `fn load(&mut self) -> &[HistorySession]` (newest first; incremental from the last offset;
  restart when the file is replaced, shrank, or its last 64 read bytes differ; persisted to
  `state_dir()/history.json` when `stored`), `fn generation(&self) -> u64`.
- Produces (transcript): `fn transcript_path(id: &str, cwd: &str) -> PathBuf`,
  `fn tail(path: &Path, n: u64) -> io::Result<Vec<u8>>`, `fn was_interrupted(path: &Path) ->
  bool` (8 KiB tail), `fn find_title(chunk: &[u8]) -> String` (`customTitle` over `aiTitle`,
  last valid occurrence), `fn title_of(path: &Path) -> String` (512 KiB tail),
  `struct Titles` with `fn new(stored: bool)`, `fn get(&mut self, id: &str, path: &Path) ->
  String` (mtime cache), `fn save(&mut self)` (`state_dir()/titles.json`, only ids asked for).
- Port: `claude/files.py`, `tests/test_files.py`.

- [ ] **Step 1: Failing tests:** ported tests, plus `history_line_without_newline_waits`,
  `history_splits_on_newline_only` (a prompt holding U+2028 stays one entry),
  `title_with_escaped_quotes_and_unicode`.
- [ ] **Step 2:** `cargo test claude::` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test claude::` → pass.
- [ ] **Step 5:** Commit `feat(claude): read history and transcripts`.

### Task 7: tmux queries and actions

**Files:** Create `src/term/mod.rs`, `src/term/tmux.rs`; Test: unit tests with a fake runner,
`tests/tmux.rs` with `PrivateTmux`.

**Interfaces:**
- Produces: `trait Runner { fn run(&self, args: &[&str]) -> String; }` (`""` on any failure,
  never panics); `struct Cli { socket: Option<String> }` (spawns `tmux [-S socket] args`, kills
  it after 1 s); `struct Pane { pane_id, window_id: String, index: u32, window_index: u32,
  sidebar: bool, landing: bool, window_name: String }`; `struct Window { window_id: String,
  lead: Record, leader: Option<Record> }` with `fn head(&self) -> &Record`;
  `fn detect(env: &HashMap<String, String>, session_id: &str, tmux: &dyn Runner) -> Option<Mux>`,
  `fn is_headless(env) -> bool`, `fn attaches(argv: &[String], session_id: &str) -> bool`,
  `fn panes(t: &dyn Runner) -> Vec<Pane>`, `fn windows_of(records: &[Record], panes: &[Pane],
  socket: &str) -> Vec<Window>`, `fn window_entries(panes: &[Pane]) -> Vec<(String, u32,
  String)>`, `fn window_name(r: &Record, title: &str) -> String` (60 cells, `…`),
  `fn name_window(r: &Record, title: &str, t: &dyn Runner)`, `fn pane_in_view(m: &Mux,
  t: &dyn Runner) -> bool`, `fn settle_if_viewed(r: Record, now: f64, t: &dyn Runner) ->
  Record`, `fn close_window(window_id: &str, t: &dyn Runner)`, `fn next_session_window(...)`,
  `fn add_sidebar(window_id: &str, t: &dyn Runner)` (marks `@cs-sidebar`),
  `fn fit_sidebars(t: &dyn Runner)`. `Pane.landing` reads `@cs-landing`.
- Port: `term/tmux.py`, `tests/test_tmux.py`.

- [ ] **Step 1: Failing tests:** ported tests (fake `Runner` recording calls and replaying
  outputs); `tests/tmux.rs`: `add_sidebar_on_a_private_server` (split at width 29, marked once),
  `runner_returns_empty_when_the_server_is_gone`.
- [ ] **Step 2:** `cargo test tmux` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test tmux` → pass.
- [ ] **Step 5:** Commit `feat(term): port tmux queries and actions`.

### Task 8: `cst hook`

**Files:** Create `src/claude/hook.rs`; Modify `src/main.rs`; Test: unit tests,
`tests/hook.rs`.

**Interfaces:**
- Consumes: Tasks 3–7.
- Produces: `fn handle_event(event: &Event, env: &HashMap<String, String>, now: f64,
  t: &dyn Runner) -> anyhow::Result<()>`; `fn live_sessions(now: f64) -> Vec<Record>`
  (settles Esc interrupts for interruptible, non-background records quiet > `QUIET_SECONDS =
  3.0`); `fn settle_interrupted(id: &str, now: f64) -> Option<Record>`;
  `fn seen(pane: &str, socket: &str, now: f64)`. `main`: `cst hook` reads stdin, calls
  `handle_event` with `Cli`, logs any error, exits 0.
- Port: `claude/hook.py`, `tests/test_hook.py`.

- [ ] **Step 1: Failing tests:** ported tests; `tests/hook.rs`:
  `hook_survives_bad_input` (stdin `""`, `"nope"`, `{}`, `{"session_id":"../x",
  "hook_event_name":"Stop"}`, `{"session_id":"s","hook_event_name":"Bogus"}`,
  `background_tasks: 5`): exit 0, no file outside the records dir, an error logged for the
  first four; `parallel_hooks_keep_every_pending_agent` (8 `cst hook` processes with
  `PermissionRequest` for agents a0..a7 at once → record has 8 pending, status permission).
- [ ] **Step 2:** `cargo test hook` → fail.
- [ ] **Step 3:** Implement; the dispatch for `hook` must not touch argument parsing beyond
  `args[1]`.
- [ ] **Step 4:** `cargo test hook` → pass.
- [ ] **Step 5:** Commit `feat(hook): port the Claude Code hook`.

### Task 9: `cst statusline`

**Files:** Create `src/claude/statusline.rs`; Modify `src/main.rs`; Test: unit, `tests/statusline.rs`.

**Interfaces:**
- Produces: `fn parse(data: &Value) -> Usage`, `struct Quota { pct: f64, resets_at: f64 }`,
  `fn parse_limits(data: &Value) -> BTreeMap<String, Quota>` (`five_hour`, `seven_day`),
  `fn load_limits(now: f64) -> BTreeMap<String, Quota>`, `fn save_limits(limits, now)` (higher
  use wins for the same reset), `fn saved_statusline_path() -> PathBuf`
  (`state_dir()/statusline.json`), `fn saved_command() -> Option<String>`,
  `fn record_usage(data: &Value, now: f64)` (record + journal cost line when cost changed),
  `fn print_statusline(raw: &[u8], data: &Value, out: &mut dyn Write)` (chain via `sh -c` with
  stdin, killed after 5 s, else `"{model}  ${cost:.2}"` or empty line).
- Port: `claude/statusline.py`, `tests/test_statusline.py`.

- [ ] **Step 1: Failing tests:** ported tests, plus `chain_is_killed_after_five_seconds`
  (saved command `sleep 10; echo late` → returns within 6 s, prints nothing),
  `bool_is_not_a_number` (`"total_cost_usd": true` → cost None).
- [ ] **Step 2:** `cargo test statusline` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test statusline` → pass.
- [ ] **Step 5:** Commit `feat(statusline): port the status line`.

### Task 10: `cst install` / `cst uninstall` with migration

**Files:** Create `src/claude/install.rs`; Modify `src/main.rs`; Test: unit,
`tests/install.rs`.

**Interfaces:**
- Produces: `const EVENTS: [&str; 11]` (as `install.py`), `fn is_ours(command: &str) -> bool`
  (argv of 2, basename `cs` or `cst`, second `hook` or `statusline`), `fn shape_error(s: &Value)
  -> Option<String>`, `fn merge(settings: &Value, cst: &str, saved: Option<Value>) -> (Value,
  Option<Value>)`, `fn unmerge(settings: &Value, saved: Option<Value>) -> Value`,
  `fn install(cst: &Path) -> i32`, `fn uninstall() -> i32`. Legacy saved status line:
  `$CLAUDE_SESSIONS_STATE/statusline.json` or `~/.local/state/claude-sessions/statusline.json`,
  read when the new saved file is missing and the current status line is ours. Output messages
  and backup name (`settings.json.before-claude-sessions`) as `install.py`. JSON written with
  2-space indent, key order preserved (`serde_json` `preserve_order` feature), trailing newline.
- Port: `claude/install.py`, `tests/test_install.py`.

- [ ] **Step 1: Failing tests:** ported tests, plus `migrates_python_hooks_to_cst`
  (settings with `/home/u/.local/bin/cs hook` on all events and `cs statusline` → every entry
  becomes `<cst> hook`/`<cst> statusline`, no duplicate groups),
  `carries_over_the_python_saved_status_line` (legacy file `{"type":"command","command":
  "~/.claude/statusline.sh"}` → new saved file equal, uninstall restores it),
  `keeps_unrelated_hooks_and_key_order`, `follows_a_symlinked_settings_file`,
  `malformed_settings_change_nothing` (bytes before == after).
- [ ] **Step 2:** `cargo test install` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test install` → pass.
- [ ] **Step 5:** Commit `feat(install): register hooks and migrate cs`.

### Task 11: Golden harness

**Files:** Create `tools/golden.py`, `tests/golden/fixtures.json`,
`tests/golden/expected/<name>.json`, `tests/support/golden.rs`, `tests/golden.rs`.

**Interfaces:**
- Produces the neutral form both sides emit: a JSON array of lines; a line is an array of runs
  `{"text": str, "fg": str, "bg": str, "bold": bool}`, adjacent runs with equal style merged,
  empty runs dropped. Colours: `"default"`, `"idx:N"` (rich standard/`color(N)` numbers and
  ratatui named colours by ANSI index: Black 0, Red 1, Green 2, Yellow 3, Blue 4, Magenta 5,
  Cyan 6, Gray 7, DarkGray 8, LightRed 9, LightGreen 10, LightYellow 11, LightBlue 12,
  LightMagenta 13, LightCyan 14, White 15; `Indexed(n)` → `idx:n`), `"rgb:#rrggbb"`.
- Fixture schema (`fixtures.json`, an array): `{"name", "kind": "sidebar"|"landing"|"panel",
  "now", ...inputs}`; sidebar: `entries`, `current`, `stats`, `height`, `runs`; landing:
  `folders`, `query`, `cursor`, `expanded`, `scroll`, `searching`, `opened`, `stats`, `width`,
  `height`; panel: `folders`, `query`, `cursor`, `expanded`, `scroll`, `height`. `stats`:
  `limits`, `today`, `week` (`[[ "YYYY-MM-DD", dollars ]]`), `day` (or null), `totals` (or null:
  `by_project`, `by_model`, `added`, `removed`, `sessions`), `hours` (24 numbers). Typed paths
  are never folders (`is_dir` false) except those listed in a fixture's `dirs`.
- Produces for Rust tests: `fn runs(lines: &[Line]) -> serde_json::Value`.
- `TZ=UTC` for generation and comparison.

- [ ] **Step 1:** Write `fixtures.json` with at least these states: `sidebar_all_statuses`
  (one entry per status, current = the working one, costs, durations, models, one model too
  long to show), `sidebar_long_names` (CJK and emoji names cut with `…`), `sidebar_no_quota`,
  `sidebar_background_runs`, `sidebar_short_5`, `sidebar_short_20` (Today dropped),
  `sidebar_empty`, `landing_wide_200x50`, `landing_medium_100x40`, `landing_narrow_30x20`,
  `landing_searching_no_match`, `landing_expanded_scrolled`, `panel_default`,
  `panel_expanded_scrolled`, `panel_typed_path`, `panel_empty`.
- [ ] **Step 2:** Write `tools/golden.py` (run with `uv run python tools/golden.py`): builds
  the Python objects from each fixture, calls `sidebar.column`, `landing.screen`,
  `new_panel.screen`, renders each `Text` through `rich` segments, writes
  `expected/<name>.json`. Run it; inspect two outputs by eye against the live app.
- [ ] **Step 3:** Write `tests/support/golden.rs` and `tests/golden.rs` holding only
  `runs_merge_adjacent_equal_styles` (two spans of equal style and one of another → two runs;
  an empty span → dropped; `Color::White` → `"idx:15"`, `Color::Rgb(215,119,87)` →
  `"rgb:#d77757"`).
- [ ] **Step 4:** `cargo test --test golden` → pass.
- [ ] **Step 5:** Commit `test: add screen goldens from the Python UI`.

### Task 12: Line helpers and words

**Files:** Create `src/ui/mod.rs`, `src/ui/line.rs`, `src/ui/words.rs`.

**Interfaces:**
- Produces (`line.rs`): `fn cell_len(line: &Line) -> usize`, `fn pad_right(line: &mut Line,
  cells: usize)`, `fn truncate(line: &mut Line, cells: usize)`, `fn styled(parts: &[(&str,
  Style)]) -> Line<'static>`, `fn cells(c: char) -> usize` (unicode-width; combining 0).
- Produces (`words.rs`): style consts matching `words.STYLES` (`INK`, `GREEN`, `BLUE`, `MAGENTA`,
  `SOFT` idx 244, `FAINT` 240, `RULE` 238, `STRONG` White+bold, `CLAUDE` #d77757, `SUCCESS`
  #4eba65, `ERROR` #ff6b80, `WARNING` #ffc107, `PERMISSION` #b1b9f9, `INACTIVE` #999999,
  `SUBTLE` #505050, `MASCOT` = CLAUDE, `CURRENT` bg idx 235); `MASCOT_ROWS`, `WEEKDAYS`,
  `MONTHS`, `FPS = 8`; fns `span`, `remaining`, `waiting_label`, `money`, `fit(text: &str,
  width: usize) -> String`, `bar(pct: f64, cells: usize) -> Vec<Span>`, `bar_style`, `age`,
  `thousands`, `next_tick(now: f64, per_second: u32) -> f64`.
- Port: `ui/words.py`, `tests/test_words.py`.

- [ ] **Step 1: Failing tests:** ported tests, plus `fit_cuts_wide_chars_by_cells`
  (`fit("日本語テキスト", 6) == "日本…"`: 2 + 2 cells, then the ellipsis),
  `fit_zero_width_is_empty` (`fit("abc", 0) == ""`).
- [ ] **Step 2:** `cargo test ui::` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test ui::` → pass.
- [ ] **Step 5:** Commit `feat(ui): add text helpers and formats`.

### Task 13: Usage totals

**Files:** Create `src/ui/usage.rs`.

**Interfaces:**
- Consumes: `journal::{DayReader, window_totals, model_costs, day_costs}`, `HistorySession`.
- Produces: `struct Day { added: u64, removed: u64, sessions: u32, waited: f64 }`,
  `struct Totals { today: f64, by_project: Vec<(String, f64)>, by_model: Vec<(String, f64)>,
  added, removed: u64, sessions: u32, per_session: HashMap<String, f64>,
  days: Vec<(NaiveDate, f64)>, today_by_session: HashMap<String, SessionTotals> }`,
  `struct Reader` with `fn totals(&mut self, history: &[HistorySession], history_gen: u64,
  now: f64) -> &Totals` (recomputes only when a day reader's generation or `history_gen`
  changed), `fn day(&self, now: f64) -> Day`, `fn hours(&self, now: f64) -> [f64; 24]`;
  `fn working_hours(...)`, `fn day_summary(...)` (`MAX_OPEN = 7200.0`).
- Port: `ui/usage.py`, `tests/test_usage.py`.

- [ ] **Step 1: Failing tests:** ported tests.
- [ ] **Step 2:** `cargo test usage` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test usage` → pass.
- [ ] **Step 5:** Commit `feat(ui): port usage totals`.

### Task 14: Sidebar rendering

**Files:** Create `src/ui/sidebar.rs`; Modify `tests/golden.rs` (sidebar cases).

**Interfaces:**
- Produces: `struct Entry { window_id, name: String, status: Status, since: f64,
  cost, duration: Option<f64>, model: Option<String> }`, `struct Stats { limits:
  BTreeMap<String, statusline::Quota>, today: f64, week: Vec<(NaiveDate, f64)>,
  day: Option<Day>, totals: Option<Totals>, hours: Vec<f64> }`, `struct Run { name: String,
  started: f64 }` (all `Deserialize` for fixtures); `fn load_entries(records: &[Record],
  panes: &[Pane], socket: &str) -> Vec<Entry>`, `fn ordered`, `fn step(entries: &[Entry],
  current: &str, offset: i32) -> Option<String>`, `fn entry_at(entries, y: u16, height: u16,
  stats) -> Option<&Entry>`, `fn column(entries: &[Entry], current: &str, stats: &Stats,
  now: f64, height: u16, runs: &[Run]) -> Vec<Line<'static>>`, `fn stats_lines`,
  `fn panel_lines(stats, now, content: usize, models: bool) -> Vec<Line>`; consts `WIDTH = 29`,
  `CONTENT = 27`, `PANEL_WIDTH = 56`.
- Port: `ui/sidebar.py`, `tests/test_sidebar.py`.

- [ ] **Step 1: Failing tests:** ported tests; `tests/golden.rs` single `#[test] fn
  screens_match_python()` that sets `TZ=UTC` first, loads every fixture, and asserts
  `runs(render(fixture)) == expected` for the kinds implemented so far (sidebar), printing the
  first differing line on failure; `column_never_exceeds_width_or_height` (proptest over
  heights 1–60 and random entries with wide-char names: every line `cell_len == 29`, line count
  `== height`, no panic).
- [ ] **Step 2:** `cargo test sidebar && cargo test --test golden` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Both → pass.
- [ ] **Step 5:** Commit `feat(ui): port the sidebar column`.

### Task 15: Folder list, Alt+n panel rendering, folders

**Files:** Create `src/ui/picker.rs`, `src/ui/folders.rs`; Modify `tests/golden.rs` (panel).

**Interfaces:**
- Produces (`picker.rs`): `struct Session { id, title: String, last: f64, cost: Option<f64> }`,
  `struct Folder { path, name: String, last: f64, count: u32, spend: f64,
  sessions: Vec<Session> }`, `enum RowKind { Path, Folder, Session }`, `struct Row { kind,
  path: String, folder: Option<usize>, session: Option<usize>, end: bool }` (indices into
  `folders`), `enum Key { Up, Down, Left, Right, Enter, Backspace, Char(char) }`,
  `struct Picker { folders: Vec<Folder>, query: String, cursor: usize,
  expanded: BTreeSet<String>, scroll: HashMap<String, usize>, is_dir: Box<dyn Fn(&str) ->
  bool> }` with `fn shown(&mut self) -> Vec<Row>`, `fn key(&mut self, k: Key) -> Option<Row>`;
  `fn list_lines(...)`, `fn keys_line(keys: &[(&str, &str)], width: usize) -> Line`,
  `fn screen(picker: &mut Picker, now: f64, height: u16) -> Vec<Line>`,
  `fn command(row: &Row, folders: &[Folder]) -> Vec<String>` (tmux `new-window` args);
  consts `SHOWN = 5`, `WIDTH = 48`.
- Produces (`folders.rs`): `fn load(history: &[HistorySession], costs: &HashMap<String, f64>,
  opened: &HashSet<String>, titles: &mut Titles) -> Vec<Folder>` (rescans titles only when the
  history generation changed; caller passes it).
- Port: `ui/new_panel.py`, `ui/folders.py`, `tests/test_new_panel.py`.

- [ ] **Step 1: Failing tests:** ported tests; panel fixtures enabled in `screens_match_python`;
  `command_quotes_session_ids` (`claude --resume 'a b'` shape via shell quoting).
- [ ] **Step 2:** `cargo test picker && cargo test folders && cargo test --test golden` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Both → pass.
- [ ] **Step 5:** Commit `feat(ui): port the folder list and panel`.

### Task 16: Landing page rendering

**Files:** Create `src/ui/landing.rs`; Modify `tests/golden.rs` (landing).

**Interfaces:**
- Produces: `fn header(opened: usize, now: f64) -> Vec<Line>`, `fn search_line(query: &str,
  searching: bool) -> Line`, `fn screen(picker: &mut Picker, searching: bool, opened: usize,
  stats: &Stats, now: f64, width: u16, height: u16) -> Vec<Line<'static>>`; consts `GAP = 4`,
  `LIST_MAX = 72`.
- Port: `ui/landing.py`, `tests/test_landing.py`.

- [ ] **Step 1: Failing tests:** ported tests; landing fixtures enabled;
  `screen_fits_any_size` (proptest widths 10–250, heights 3–80: line count == height, every
  `cell_len <= width`, no panic).
- [ ] **Step 2:** `cargo test landing && cargo test --test golden` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Both → pass; all fixtures in `fixtures.json` are now compared.
- [ ] **Step 5:** Commit `feat(ui): port the landing page`.

### Task 17: UI runtime and scheduler

**Files:** Create `src/ui/runtime.rs`.

**Interfaces:**
- Produces: `enum Msg { Key(crossterm::event::KeyEvent), Mouse(MouseEvent), Resize,
  Changed(PathBuf) }`; `struct Runtime { rx: Receiver<Msg>, watcher: Watcher }` with
  `fn start() -> io::Result<Self>` (input thread reading crossterm events, inotify thread
  sending `Changed`), `fn watch(&mut self, path: &Path)`, `fn unwatch_all(&mut self)`,
  `fn next(&self, deadline: Option<f64>) -> Option<Msg>` (`recv_timeout`; `None` on timeout);
  `enum Signal { Shown, Hidden, Layout }` and `fn signal(key: &KeyEvent) -> Option<Signal>`
  (F12, F11, F10);
  `struct Schedule { visible: bool, animating: bool, labels: Vec<f64>, quiet: Vec<f64>,
  last_draw: f64, dirty: bool }` with `fn next_wake(&self, now: f64) -> Option<f64>`:
  hidden → `None`; else the minimum of: next 1/8 s boundary if `animating`; for each `since`
  in `labels`, the next `since + 60k` after `now`; each `quiet` deadline after `now`;
  `last_draw + 0.1` if `dirty`.
- Produces: `fn run_terminal<F: FnMut(&mut Frame)>(...)` helper or equivalent setup/restore
  (alternate screen off, raw mode, mouse capture on, cursor hidden; restored on exit and on
  panic hook).

- [ ] **Step 1: Failing tests:** `hidden_never_wakes`, `animating_wakes_at_next_eighth`,
  `label_wakes_at_next_minute_boundary`, `quiet_deadline_wins_when_earlier`,
  `dirty_coalesces_to_100ms`, `f_keys_map_to_signals`, and
  `watcher_reports_a_file_written_atomically` (tempdir, `write_atomic` into a watched dir →
  `Changed` within 1 s).
- [ ] **Step 2:** `cargo test runtime` → fail.
- [ ] **Step 3:** Implement (watch directories, not files, so atomic renames are seen).
- [ ] **Step 4:** `cargo test runtime` → pass.
- [ ] **Step 5:** Commit `feat(ui): add the event runtime and scheduler`.

### Task 18: Sidebar app

**Files:** Create `src/ui/sidebar_app.rs`; Modify `src/main.rs`; Test: `tests/sidebar_app.rs`.

**Interfaces:**
- Consumes: Tasks 6, 7, 8 (`live_sessions`), 13, 14, 17.
- Produces: `fn main_sidebar() -> anyhow::Result<()>` behind `cst sidebar`. Behaviour:
  - Starts visible; resolves its window id from `TMUX_PANE` once.
  - `Hidden` → unwatch all, schedule hidden, no reads. `Shown` → watch records dir, journal dir,
    `config_dir()/history.jsonl`'s dir, transcripts' dirs of working/waiting sessions; reload
    everything (panes included) and draw.
  - `Layout` → re-list panes, redraw. `Changed` → reload records/journal/history as touched,
    mark dirty. Pane list is cached between `Layout` signals.
  - `quiet` deadlines = `updated_at + 3` of interruptible non-background records; on wake,
    `live_sessions` settles them.
  - Click → `select-window` on `entry_at`. Ctrl+C/Ctrl+Q do nothing.
  - Failed load or draw logs once per distinct error (as `WindowApp.log_once`).
  - No tmux call in steady state (only on `Shown`, `Layout`, click).
- Port: `ui/sidebar_app.py`, `tests/test_sidebar_app.py` (behaviour, not Textual internals).

- [ ] **Step 1: Failing tests** (`PrivateTmux`, binary under test on PATH):
  `sidebar_draws_sessions_of_its_server` (two fake records with panes of the private server →
  `capture-pane` shows both names), `hidden_sidebar_spawns_nothing` (send F11, wait 3 s:
  `/proc/<pid>/stat` utime+stime unchanged within 1 tick and no child processes),
  `shown_sidebar_redraws_on_record_change` (write a record with a new status → capture shows it
  within 0.5 s), `layout_signal_picks_up_a_new_window`.
- [ ] **Step 2:** `cargo test --test sidebar_app` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test --test sidebar_app` → pass.
- [ ] **Step 5:** Commit `feat(ui): run the sidebar on events`.

### Task 19: Landing page and Alt+n panel apps

**Files:** Create `src/ui/landing_app.rs`, `src/ui/panel_app.rs`; Modify `src/main.rs`; Test:
`tests/landing_app.rs`, `tests/panel_app.rs`.

**Interfaces:**
- Produces: `cst landing` and `cst new --pick`. Keys as `landing_app.py` / `new_app.py`:
  landing — `/` search, Enter/Esc leave search, `q` kills the server, arrows/Enter on the
  picker open `new-window`; panel — Esc exits, Enter opens and exits, Ctrl+C/Ctrl+Q exit,
  first frame (search line) drawn before history loads. Landing uses Shown/Hidden/Layout like
  the sidebar and watches the same paths plus nothing else; history and titles persisted
  (`stored = true`).
- Port: `ui/landing_app.py`, `ui/new_app.py`, their tests.

- [ ] **Step 1: Failing tests:** ported behaviour tests driven through `PrivateTmux`
  `send-keys` + `capture-pane`: `landing_enter_opens_a_window_in_the_folder`,
  `landing_q_kills_the_private_server`, `panel_escape_exits`, `panel_typing_filters`.
- [ ] **Step 2:** `cargo test --test landing_app --test panel_app` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Both → pass.
- [ ] **Step 5:** Commit `feat(ui): port the landing page and panel apps`.

### Task 20: tmux.conf, launcher, layout/step/close/focus

**Files:** Create `assets/tmux.conf`, `src/term/launcher.rs`, `src/term/layout.rs`; Modify
`src/main.rs`; Test: unit, `tests/tmux_conf.rs`, `tests/launcher.rs`.

**Interfaces:**
- Produces commands:
  - `cst` / `cst --ghostty`: as `term/launcher.py`; conf written to `state_dir()/tmux.conf`
    (rewritten only when its bytes differ), `PATH` prefixed with the binary's dir, landing
    window marked `@cs-landing 1`.
  - `cst layout [WINDOW]`: add sidebar to WINDOW if given; close windows whose only pane is a
    sidebar (selecting the neighbour as `close_window` does); fit sidebars; send F10 to the
    sidebar or landing pane of the current window.
  - `cst step up|down WINDOW`, `cst close WINDOW`: as `cli.py`.
  - `cst focus --pane P --socket S`: replaces `cs seen` and `@cs-wake`: marks the finished
    session in P seen; sends F11 to the sidebar/landing pane of `@cs-last-window` when it is a
    different window; sends F12 to the current window's sidebar/landing pane; sets
    `@cs-last-window`.
- `assets/tmux.conf`: `tmux.conf` of `937dfa7` with `cs` → `cst`, `@cs-seen`/`@cs-wake` replaced
  by `cst focus …` on `session-window-changed`, `window-pane-changed`, `client-attached`,
  `client-session-changed`; `cst layout` also on `pane-exited`, `after-kill-pane`,
  `window-renamed`, `window-unlinked`.
- Port: `term/launcher.py`, `cli.py` (step, close, layout), `tests/test_tmux_conf.py`,
  `tests/test_launcher.py`, `tests/test_cli.py`.

- [ ] **Step 1: Failing tests:** ported tests; `tests/tmux_conf.rs`:
  `conf_loads_without_errors_on_tmux` (private server `-f`; `show-hooks -g` lists the new hooks),
  `closing_claude_closes_a_sidebar_only_window` (window with sidebar + `sleep 1` pane → after
  exit, window gone), `focus_sends_hidden_to_the_previous_window` (two windows with a pane
  marked sidebar running `cat -v`; switch windows → previous pane shows the F11 sequence,
  current shows F12).
- [ ] **Step 2:** `cargo test --test tmux_conf --test launcher` → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Both → pass.
- [ ] **Step 5:** Commit `feat(term): add launcher, conf and tmux commands`.

### Task 21: Update, install script, release workflow

**Files:** Create `src/update.rs`, `.github/workflows/release.yml`; Modify `install.sh`,
`src/main.rs`; Test: `tests/update.rs`, `tests/install_sh.rs`.

**Interfaces:**
- Produces: `cst update`: base URL `CST_RELEASE_BASE` or
  `https://github.com/emanuel36/claude-sessions/releases`; latest tag from
  `<base>/latest` redirect (`curl -fsSLI -o /dev/null -w '%{url_effective}'`) or, for a
  `file://` base, the file `<base>/latest/tag`; asset `<base>/download/<tag>/cst-<arch>-unknown-
  linux-musl` and `.sha256` (`sha256sum` format); fetched with `curl -fsSL`; verified with
  `sha2`; written beside `current_exe()` then renamed over it; runs `<new> install`; prints
  `updated to <tag>; quit and reopen cst` or `already up to date (<version>)`.
- `install.sh`: Linux + tmux ≥ 3.3 checks (kept); arch from `uname -m` (`x86_64`, `aarch64`/
  `arm64`); `uv tool uninstall claude-sessions` when `uv tool list` shows it; download + verify
  (`sha256sum -c`) into `${XDG_BIN_HOME:-$HOME/.local/bin}/cst`; `CST_SOURCE=<dir>` builds with
  `cargo build --release --manifest-path <dir>/Cargo.toml` and copies `target/release/cst`;
  `cst install`; PATH hint; "quit and reopen cst" when a `tmux -L claude` server runs.
  `CST_RELEASE_BASE` honoured as in `cst update`.
- `release.yml`: on tag `v*`; matrix `ubuntu-24.04` (`x86_64-unknown-linux-musl`) and
  `ubuntu-24.04-arm` (`aarch64-unknown-linux-musl`), `musl-tools`, `cargo build --release
  --target`, asset renamed `cst-<target>`, `sha256sum` file, `gh release create`/`upload`.

- [ ] **Step 1: Failing tests:** `tests/update.rs` with a `file://` base in a temp dir holding a
  copy of the test binary as the asset: `update_replaces_the_binary_after_verifying`,
  `update_refuses_a_bad_checksum` (binary untouched, exit 1), `update_says_up_to_date`;
  `tests/install_sh.rs`: `install_sh_from_source_installs_and_registers` (`CST_SOURCE=` repo,
  `HOME`/`XDG_*` in temp; `cst` exists and `settings.json` has 11 hooks),
  `install_sh_refuses_old_tmux` (fake `tmux` on PATH printing `tmux 3.2`).
- [ ] **Step 2:** `cargo test --test update --test install_sh` → fail.
- [ ] **Step 3:** Implement; `actionlint` on the workflow if available.
- [ ] **Step 4:** Both → pass.
- [ ] **Step 5:** Commit `feat: add self-update, installer and releases`.

### Task 22: Performance check

**Files:** Create `bench/run.sh`, `bench/README.md` (one paragraph: needs hyperfine and tmux).

**Interfaces:**
- Consumes: the release binary (`cargo build --release`).
- Produces: `bench/run.sh` printing, per Goals: hook median and CPU vs `/bin/true` (hyperfine
  `-N`, 3 interleaved rounds of 200 runs, 30 warm-ups, `PreToolUse` on stdin, temp dirs,
  `TMUX` unset), status line the same, peak RSS of both (`/usr/bin/time -f %M`), sidebar CPU %
  and RSS/PSS over 60 s in a private server with 10 records (4 working), hidden sidebar CPU
  ticks and child count over 60 s; each line ends `PASS` or `FAIL` against the spec's targets.

- [ ] **Step 1:** Write the script.
- [ ] **Step 2:** Run `sh bench/run.sh` → every line `PASS`. A `FAIL` is a bug to fix in the
  owning task's code before continuing.
- [ ] **Step 3:** Commit `test: add the performance check`.

### Task 23: Remove Python, rewrite README

**Files:** Delete `src/claude_sessions/`, `tests/*.py`, `tests/__pycache__`, `pyproject.toml`,
`uv.lock`, `tools/golden.py`; Modify `README.md`, `.gitignore`.

- [ ] **Step 1:** Delete the files; `cargo test` → pass (goldens remain).
- [ ] **Step 2:** Rewrite `README.md` for users: what it is, statuses, keys, install
  (`curl … | sh`), update (`cst update`), uninstall (`cst uninstall`, then delete
  `~/.local/bin/cst`), troubleshooting, how it is organised (tree of `src/`), license. No code
  blocks other than the install one-liner; links to the GitHub repository only; keep
  `docs/images/*.svg`.
- [ ] **Step 3:** `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo
  test` → pass.
- [ ] **Step 4:** Commit `refactor!: replace the Python version with Rust` with body noting
  `BREAKING CHANGE: cs is replaced by cst; history before 2.0.0 is not read`.
