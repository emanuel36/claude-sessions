# Rust rewrite

Decisions of 2026-11-08, taken with the owner. Calls marked *(builder's call)* or *(my call)* were
not made by the owner and may be revisited.

## Intent

claude-sessions shows which Claude Code session waits for its user. It runs on every Claude Code
hook event and status line update, and keeps a sidebar process in every tmux window. Measured on
the owner's machine (2026-11-08, hyperfine and /proc sampling, prototypes doing the same work):

| What | Python today | Rust prototype |
|---|---|---|
| Hook, median wall time | 69 ms | 6.8 ms (floor: 4.6 ms) |
| Hook, CPU per event | 70 ms | 5 ms (floor: 3.6 ms) |
| Sidebar, RSS / PSS | 34 MB / 26 MB | 2.5 MB / 0.9 MB |
| Sidebar, CPU while animating | 6.4 % | 0.4 % |

Go was measured too and lost on every row (hook 20 ms, sidebar 9.8 MB and 1.9 %).

A second cost is independent of language: each `tmux` call spawns a client that costs about 5 ms
of CPU. The visible sidebar today asks tmux whether it is on screen on every frame (8 per second
while a session works, about 4 % of a core), and every hidden sidebar asks once a second.

The rewrite makes the tool as cheap as it can be while it looks and behaves as it does today.

## Goals

- **Same behaviour and look.** Every status, transition, screen, key, colour and character stays
  as the Python version at commit `937dfa7` has it. That code is the behavioural reference; where
  this document is silent, port what it does.
- **Cheap hot path.** `cst hook` on an event that needs no tmux call (such as `PreToolUse` or
  `PostToolUse`, the most frequent), and `cst statusline` with no user status line chained, within
  3 ms of a `/bin/true` floor (median), and under 5 MB peak RSS. Events that call tmux (naming
  events, a finished session on screen, background sessions) cost that floor plus about 5 ms per
  tmux call, as the Python version's calls do; their number does not grow *(my call)*.
- **Idle means idle.** A hidden sidebar or landing page uses no CPU, reads no files and spawns no
  tmux client. The visible one spawns no tmux client in steady state; it redraws on events.
  Animating with a working session, it stays at or under 1 % of a core and 5 MB RSS
  *(my call)*.
- **Install, update and run are all the user deals with.** One `curl | sh`, `cst update`, `cst`.

## Non-goals

- macOS, native Windows (WSL2 works as Linux). Linux x86_64 and aarch64 only.
- New features or visual changes. If a behaviour is costly, raise it with the owner before
  changing it.
- A daemon or any long-lived background process other than the UI panes.
- Async runtimes (tokio and the like): every loop here is a single-threaded event loop.
- Compatibility with the Python version's files on disk; past days of history may be lost.

## Anti-goals (a failure even if it works)

- A hook that fails, blocks or slows a Claude Code session. Hooks always exit 0; errors go to the
  error log.
- Damage to the user's `settings.json`: install and uninstall must keep every hook and status
  line that is not ours, and uninstall must restore the user's own status line.
- A test, benchmark or development run that touches the live `tmux -L claude` server.
- A status shown wrong where the Python version shows it right.

## Product surface

One static binary, `cst` *(my call: the owner only cares about install, update and run)*.

| Command | Does |
|---|---|
| `cst` | Opens the `claude` tmux server (creating it), as the Python `cst` does |
| `cst --ghostty` | Same, in a new Ghostty window with Alt+Up/Down unbound, as today |
| `cst update` | Installs the latest release in place, then re-registers the hooks |
| `cst uninstall` | Takes the hooks and status line out of `settings.json`, restoring the user's |
| `cst --version` | Prints the version |

Internal subcommands, hidden from `--help`, keep the Python `cs` subcommands' meaning: `hook`,
`statusline`, `install`, `landing`, `sidebar`, `new --pick`, `layout [WINDOW]`,
`step up|down WINDOW`, `close WINDOW`, `seen --tmux-pane P --tmux-socket S`. Exact flag spelling is
the builder's call; tmux.conf and settings.json are the only callers.

The version becomes 2.0.0.

## Architecture

### Hot path: hook and status line

Short-lived, synchronous, no tmux calls beyond those the Python version makes (rename the window
on naming events, settle a finished session whose pane is on screen, find the pane of a
background session). Same steps as `claude/hook.py` and `claude/statusline.py`:

- Per-session `flock` on a lock file, read the record, apply the state machine, atomic write
  (temp file + rename), append a journal line when the status or cost changes.
- The state machine (`core/state.py`) is pure and ported exactly, including subagent-aware
  `pending`, notifications that only count when nothing else explains them, background tasks on
  `Stop`, compaction on `SessionStart`, and `/clear`/`/resume` replacement.
- Process identity is pid plus start time from `/proc/<pid>/stat`, as today.
- The status line records usage and limits first, then runs the user's saved status line with the
  same stdin (5 s timeout) and prints its output, or prints the short default.

### UI panes: sidebar, landing page, Alt+n panel

ratatui with crossterm, one single-threaded loop per process that waits on: terminal input,
inotify, and a timer that exists only while the pane is visible and something animates or a
clock label can change.

**Visibility comes from tmux, not from polling.**

- Shown: tmux already sends F12 to the sidebar or landing pane of the window that comes on screen
  (`@cs-wake` in tmux.conf). Kept.
- Hidden: new. On `session-window-changed`, tmux sends a second key to the sidebar or landing pane
  of the window that was current before, kept in a server option the hook updates (for example
  `@cs-last-window`). A hidden pane drops its timer and inotify watches, or ignores their events,
  until shown again.
- Layout: new. When windows or panes change (new window, pane exit, rename, kill), tmux sends a
  third key to the visible pane, which then lists panes and windows once.
- Which keys are used (F10 to F12 or others that Claude Code never sees, since they go only to the
  sidebar and landing panes) is the builder's call.
- A pane that starts visible assumes it is visible until told otherwise.

**Data comes from inotify while visible.** Watches on the records directory, the journal
directory, the limits file, Claude Code's `history.jsonl`, and the transcripts of sessions that
are working or waiting. Events are coalesced: at most about 10 redraws a second. The spinner runs
at 8 fps only while a session works and the pane is visible; otherwise the timer fires at the next
whole second only when a waiting label can change (labels change by the minute).

**Esc detection.** Esc fires no hook. A session is checked for an interrupt marker only once it
has had no hook for 3 s (`QUIET_SECONDS`), as today *(my call, for parity)*: without that grace,
an Esc followed at once by a new prompt could settle a working session as idle, because
`UserPromptSubmit` marks it working before the prompt reaches the transcript. What changes is how
the check is woken: the visible pane sets a one-shot timer at `updated_at + 3 s` for each working
or waiting session, and re-checks on a transcript change only when that session is already past
its grace. No polling. Settling keeps the Python guard: under the lock, only if the record is
still in an interruptible status.

**Closing a window when only its sidebar is left** moves from the sidebar's tick to tmux: a hook
on pane exit runs `cst layout`, which closes windows whose only pane is a sidebar.

**Cached reading stays incremental.** Journal days and `history.jsonl` are read from the last
offset, as `journal.DayReader` and `files.load_history` do; titles are re-read only when a
transcript's mtime changes. Whether these caches persist on disk between processes (they do today
for the Alt+n panel and landing page) is the builder's call, judged by the panel's open time.

### Files on disk

The format is free to change *(owner's decision)*. Keep the guarantees: one record per session,
written atomically under its lock; journal lines appended with a single `O_APPEND` write; a
private records directory (`0700`, refusing a symlink or a foreign owner when it falls back to the
shared temp folder); journal days older than 7 kept no longer. New locations *(my call)*:

- Records and limits: `$XDG_RUNTIME_DIR/cst/`, falling back to `/tmp/cst-<uid>/` with the checks
  above.
- Journal, saved status line, caches, error log: `$XDG_STATE_HOME/cst/` (default
  `~/.local/state/cst/`).
- Environment overrides for tests: `CST_RUNTIME_DIR`, `CST_STATE_DIR`, and Claude Code's own
  `CLAUDE_CONFIG_DIR`.

The Python version's directories are ignored, except for the one migration below.

## Install, update, release

**Release** *(owner's decision: prebuilt binaries)*. A GitHub Actions workflow, triggered by a
`v*` tag, builds static musl binaries for `x86_64-unknown-linux-musl` and
`aarch64-unknown-linux-musl`, and publishes them on a GitHub release with a SHA-256 for each.
The owner creates and pushes tags; nothing here pushes. Build tooling (cross, cargo-zigbuild, or
native runners) is the builder's call.

**`install.sh`** (run with `curl -fsSL …/install.sh | sh`, as today):

1. Checks Linux and tmux 3.3 or newer.
2. Downloads the release asset for the machine's architecture, verifies its SHA-256, installs it
   to `~/.local/bin/cst` (`XDG_BIN_HOME` respected).
3. Runs `cst install` to register the hooks and status line.
4. Says when `~/.local/bin` is not on `PATH`.
5. With `CST_SOURCE=<path to a checkout>`, builds with `cargo build --release` instead of
   downloading. This is the development path.

**`cst update`** queries the latest release, downloads and verifies the asset, replaces the
binary atomically (temp file in the same directory, then rename), runs `install` again and tells
the user to quit and reopen `cst`. Open panes keep running the old binary until then, as today.

**`cst install` merge rules** are those of `claude/install.py`: our hook on the 11 events with a
10 s timeout; the user's status line saved and chained; a one-time backup of `settings.json`
beside it; nothing written when already installed; malformed settings refused with nothing
changed.

**Migrating a Python install** *(my call)*:

- Commands that are `cs hook` or `cs statusline` (any path to `cs`) count as ours and are pointed
  at `cst`, exactly like a moved `cst`.
- When the status line in `settings.json` is the old `cs statusline`, the user's own status line
  lives in the Python version's saved file (`~/.local/state/claude-sessions/statusline.json`, or
  `$CLAUDE_SESSIONS_STATE/statusline.json`). Install carries it over to the new saved file, so the
  chain survives the upgrade.
- uv records `~/.local/bin/cst` and `cs` as the old package's entry points, so uninstalling it
  after the upgrade would delete the new binary. `install.sh` therefore runs
  `uv tool uninstall claude-sessions` itself, before installing, when uv lists that package, and
  says so *(my call: an upgrade replaces the old version)*.
- A `tmux -L claude` server still running from the Python version keeps calling `cs`; its panes go
  blank once the hooks point at `cst`. `install.sh` and `cst update` say to quit and reopen `cst`;
  neither kills the server.

## Code

- One Cargo crate at the repository root, a binary with a library target so integration tests
  reach the modules. Module layout is the builder's call; the Python packages (`core`, `claude`,
  `term`, `ui`) are a reasonable map.
- Crates: ratatui and crossterm, serde and serde_json, a small inotify crate, rustix or libc for
  `flock`, a local-time crate for day boundaries, unicode-width for cell widths. Dev: proptest,
  insta, tempfile. Exact picks are the builder's call; prefer small, synchronous crates.
- Lines of 100 characters at most (`rustfmt.toml`: `max_width = 100`); `crate::` paths, never
  `super::` chains; errors as `Result` with context, never a panic on the hot path; rustdoc on
  non-trivial public items.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass on every
  commit.

## Testing

- **Screen parity with golden files.** The Python screen functions are pure:
  `ui/sidebar.column`, `ui/landing.screen`, `ui/new_panel.screen`. Before the Python code is
  removed, a one-off script renders them for a fixed set of states (a fixed `now`, records, stats,
  history) to a neutral form: per line, runs of text with foreground, background and bold. Those
  goldens are committed; the Rust renderers, drawn into a ratatui `Buffer` and converted to the
  same form, must match them exactly. The states cover every status, the current-window
  highlight, a long name cut with `…`, no quota, background runs, short terminals that drop
  blocks, the wide and narrow landing layouts, an expanded folder with scrolled sessions, and a
  search with no match. The colour mapping (rich's `color(N)`, hex and named colours to ratatui's)
  is the builder's call, written down once in the converter.
- **State machine:** unit tests ported from `tests/test_state.py`, and proptest invariants over
  random event sequences, for example: no agent appears twice in `pending`; the status is
  `permission` exactly when some pending entry is a permission, `input` when only questions are
  pending; `status_since` changes only when the status does.
- **Hook and status line:** integration tests run the binary with stdin and temporary
  directories, as `tests/test_hook.py` and `tests/test_statusline.py` do, including concurrent
  hooks for one session.
- **Install:** the cases of `tests/test_install.py`, plus the Python-install migration.
- **tmux:** integration tests on a private server (`tmux -L cst-test-<pid>`), killed at the end;
  skipped when tmux is missing. Never `-L claude`.
- **Performance check before calling it done**, with a script committed under `bench/`
  *(my call)*, reproducing the 2026-11-08 method:
  - hook: hyperfine (`-N`, 3 interleaved rounds of 200 runs, 30 warm-ups), `/bin/true` as the
    floor, a `PreToolUse` event on stdin, a temporary records directory, `TMUX` unset;
  - status line: the same, with a status line input and no user status line saved;
  - sidebar: the real `cst sidebar` in a private tmux server, 10 records with 4 working, sampled
    60 s from `/proc/<pid>/stat` and `smaps_rollup`;
  - hidden sidebar: 60 s with zero CPU ticks and no tmux child processes.

  Results are checked against Goals.

## Rollout

- All work on a branch in an isolated worktree; conventional commits; no push.
- The Python code, `pyproject.toml`, `uv.lock` and the Python tests are removed once the Rust
  version passes the goldens and the checks above. The golden-generation script is removed with
  them; the goldens stay.
- tmux.conf moves into the crate (embedded with `include_str!` and written where tmux can read it,
  or passed through `-f` from a cached path: builder's call), with the new hide and layout signals
  and `cst` in place of `cs`.
- The README is rewritten for users: what it is, install, update, uninstall, statuses, keys,
  troubleshooting. No code; links to the remote repository only. The screenshots stay.

## Left to the builder

Module layout, crate picks within the constraints above, the signal keys, the disk format,
whether UI caches persist, release build tooling, and the golden colour mapping.
