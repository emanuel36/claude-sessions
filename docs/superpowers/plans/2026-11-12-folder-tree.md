# Folder Tree Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the Alt+n panel and the landing page create a folder or open an unlisted one from
two rows that open the disk as a tree inside the list, in Claude Code's look.

**Architecture:** A new `src/ui/tree.rs` reads folders (children, a bounded breadth-first walk,
last-use order) and caches them. `Picker` (`src/ui/picker.rs`) gains two entry rows, the tree
state, the disk search and the create row; its drawing gains the gutter pointer, the search box
and per-row hints. `landing.rs`/`landing_app.rs` reuse them. The prototype in `/tmp/cst-tree`
(uncommitted) is a reference only.

**Tech Stack:** Rust 2021 (1.92), ratatui, crossterm, tmux; tests with `tempfile`, `proptest`
and the JSON screen goldens in `tests/golden`.

**Spec:** `docs/superpowers/specs/2026-11-12-folder-tree-design.md`

## Global Constraints

- No new crates; folders read with `std::fs`. Linux only.
- Walk limits: 6 levels deep, 40 000 folders scanned, 50 matches listed.
- Left out: names starting with `.` (unless the search starts with `.`), `node_modules`,
  `target`.
- Order: Claude session in or below the folder first, newest first; then newest mtime first.
  Search: shallower first, then that order, then same name / starts with / contains.
- Matching ignores case; the create row's "exact name" check respects case.
- Copy (exact): `+ New folder…`, `Open another folder…`, `+ create <~path> and start`,
  `in <~dir>/`, `pick the parent, then type the name`, `type to search below`,
  `Search folders…`, `Search folders, or type a new folder's name…`, hints in the form
  `Enter to start here · → to open · Esc to cancel`.
- Colours from `src/ui/words.rs`: pointer and selected text `PERMISSION`, `❯` in the box
  `CLAUDE`, match letters `CLAUDE` bold, hints `INACTIVE`, separators `SUBTLE`, box border
  `SOFT`, tree lines `SUBTLE`, `+` `GREEN`.
- Never create a folder or start a session unless Enter was pressed on that row.
- 100-column lines, absolute `crate::` imports, Google-style doc comments as the repo writes
  them; `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` pass.
- Commits: Conventional Commits, subject ≤ 50 chars, body bullets, trailer
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use the `-L claude` tmux
  server in tests or trials.

## Review Focus

- **Unreadable folder** (permission denied) in the tree or the walk: shows no subfolders, no
  panic, the rest of the tree still lists. Test in Task 1.
- **Symlink loop** (`a/loop -> ..`): the walk ends (depth bound) and does not list the loop's
  target again through the link. Walk does not follow symlinked folders; the tree lists them
  (they open as folders). Test in Task 1.
- **Name whose lowercase changes byte length** (`İstanbul`, `ǅ`): search still matches by
  name and never panics on byte offsets; marks may be dropped. Test in Task 1.
- **Create over an existing file** (`touch ~/x`, then create `x`): `mkdir` fails, the error
  goes to `errors.log`, no window opens. Test in Task 3.
- **Spaces around the text** (`  Meu-Projeto `): search and create use the trimmed text; the
  created folder has no leading or trailing spaces. Test in Task 3.

---

### Task 1: Disk reading and order (`src/ui/tree.rs`)

**Files:**
- Create: `src/ui/tree.rs`
- Modify: `src/ui/mod.rs` (add `pub mod tree;`)

**Interfaces:**
- Produces:
  - `pub const DEPTH: usize = 6; pub const SCAN_LIMIT: usize = 40_000; pub const HITS: usize = 50;`
  - `pub type Use = (u8, std::cmp::Reverse<i64>);`
  - `pub fn usage(path: &str, session: Option<f64>) -> Use` — `(0, Reverse(session secs))`
    when a session is known, else `(1, Reverse(mtime secs))` (0 when unreadable).
  - `pub fn sessions_below<'a>(folders: impl Iterator<Item = (&'a str, f64)>) -> HashMap<String, f64>`
    — each folder's last session propagated to every ancestor (max).
  - `pub fn join(dir: &str, rel: &str) -> String`
  - `pub struct Hit { pub rel: String, pub depth: usize, pub hits: Vec<usize> }` (byte offsets
    in `rel` of matched chars; empty when lowercasing changes lengths).
  - `#[derive(Default)] pub struct Disk` with `pub fn children(&self, dir: &str) -> Vec<String>`
    (names, case-insensitive alphabetical, cached), `pub fn search(&self, dir: &str, query: &str,
    order: &dyn Fn(&str) -> Use) -> Vec<Hit>` (walk cached per `(dir, hidden)`),
    `pub fn clear(&self)`.

- [ ] **Step 1: Write the failing tests** in `src/ui/tree.rs` `mod tests`, on a `tempfile`
  tree:
  - `children_leave_out_hidden_and_build_folders`: `a/ B/ .git/ node_modules/ target/` and a
    file `f` → `["a", "B"]`.
  - `search_ignores_case_and_lists_shallow_first`: `vtp/`, `lta/vtp-sdk/`, `x/y/VTP/` →
    `rel`s `["vtp", "lta/vtp-sdk", "x/y/VTP"]` for query `"Vt"` with a constant `order`.
  - `search_orders_by_use_within_a_level`: `core-vtp/`, `vtp/`; `order` ranks `core-vtp` first
    → `["core-vtp", "vtp"]`.
  - `search_finds_hidden_only_when_asked`: `.cfg/` matched by `".c"`, not by `"cfg"`.
  - `search_stops_at_six_levels`: a chain `l1/…/l8/needle` is not found; `l1/…/l5/needle` is.
  - `search_marks_the_matched_letters`: `lta/vtp-sdk`, query `"sdk"` → `hits == [8, 9, 10]`.
  - `an_unreadable_folder_lists_nothing` (Review Focus): `chmod 000` a subfolder; `children` of
    it is empty and `search` from the parent still returns its siblings. Skip when run as root.
  - `a_symlink_loop_ends` (Review Focus): `a/loop -> ..`; `search(dir, "a")` returns `["a"]`.
  - `a_name_changing_length_when_lowercased_matches` (Review Focus): `İstanbul/`, query `"stan"`
    → one hit, no panic.
  - `sessions_below_reaches_every_ancestor`: `[("/w/a/b", 5.0), ("/w/c", 9.0)]` → `"/w"` 9.0,
    `"/w/a"` 5.0.
  - `usage_puts_sessions_before_disk_changes`: `usage(p, Some(1.0)) < usage(q, None)`.
- [ ] **Step 2: Run** `cargo test --lib tree::` — expect compile failure (module missing).
- [ ] **Step 3: Implement** the interfaces above. The walk is breadth first from `dir`, reads
  each folder with `read_dir`, skips entries whose `file_type()` is a symlink, and stops queuing
  at `DEPTH` or after `SCAN_LIMIT` folders. Search sort key: `(depth, order(full path), rank,
  rel.to_lowercase())`, rank 0 same name, 1 prefix, 2 contains; then `take(HITS)`.
- [ ] **Step 4: Run** `cargo test --lib tree::` — expect PASS; `cargo clippy --all-targets --
  -D warnings` clean.
- [ ] **Step 5: Commit** `feat(ui): read folders for the folder tree`.

### Task 2: Entry rows and the tree in the list

**Files:**
- Modify: `src/ui/picker.rs` (types, `Picker`, `shown`, `key`)
- Test: `src/ui/picker.rs` `mod tests`

**Interfaces:**
- Consumes: Task 1's `Disk`, `usage`, `sessions_below`, `join`.
- Produces:
  - `pub enum Root { New, Open }` (`Copy, Eq, Ord, Hash, Debug`); list order `[New, Open]`.
  - `RowKind` gains `Root`, `Dir`, `Create`; `RowKind` and `Row` derive `Default` (`Folder`).
  - `Row` gains `pub root: Option<Root>`, `pub branch: String` (tree lines; for a root row, its
    hint), `pub label: String`, `pub hits: Vec<usize>`.
  - `Picker` gains `pub home: String` (default `paths::home`), `pub disk: Disk`,
    `pub open: BTreeSet<Root>`, `pub dirs: BTreeSet<(Root, String)>`,
    `pub anchor: Option<(Root, String)>`; `Picker::new(is_dir)` keeps its signature.
  - `Picker::shown()` returns root rows (each followed by its tree when open) then the history
    rows of today's `rows()` (unchanged signature).

- [ ] **Step 1: Write the failing tests** (home = a tempdir with `Documents/`, `workspace/vtp/`,
  `Music/`; folders from today's test helpers):
  - `the_two_entries_come_first_and_the_cursor_starts_on_new_folder`: kinds of the first two
    rows are `Root` with `root` `New`, `Open`; `cursor == 0`.
  - `right_opens_an_entry_as_a_tree_of_the_home_folder`: after `Down, Right`, rows under `Open`
    are `Dir`s labelled `Documents/`, `Music/`, `workspace/` with branches `├ `, `├ `, `└ `.
  - `the_tree_puts_folders_with_sessions_first`: a history folder at `<home>/workspace/vtp` →
    `workspace/` is the first `Dir`.
  - `right_on_a_folder_opens_it_and_left_closes_it`: with `workspace/` open, the next row is
    `vtp/` with branch `"  └ "`; `Left` removes it; `Left` again moves the cursor to the `Open`
    entry; Backspace with empty query behaves as `Left`.
  - `enter_on_an_entry_toggles_it`.
  - `enter_on_an_open_another_folder_row_returns_it`: `key(Enter)` returns the `Dir` row with
    `path == <home>/Music`.
  - `enter_on_a_new_folder_row_picks_it_as_parent`: returns `None`, `dirs` holds
    `(New, <home>/Documents)`, cursor stays on that row.
- [ ] **Step 2: Run** `cargo test --lib picker::` — expect FAIL.
- [ ] **Step 3: Implement** the types and `Picker` methods: `root_rows(root) -> Vec<Row>`,
  `tree_rows(root, dir, prefix, out)` (children sorted by `usage(path, recent.get(path))`),
  `recent() -> Rc<HashMap<String, f64>>` cached on `folders.len()`, `close(root, &Row)`,
  `reveal(root, path)` (opens every folder between `home` and `path`), `select(pred)`; the
  `Left`/`Right` arms of today keep handling `Folder`/`Session`. Update today's `Row { … }`
  literals with `..Row::default()`. Fix the existing tests that count rows from 0 (they now see
  two entry rows first).
- [ ] **Step 4: Run** `cargo test --lib` — expect PASS (goldens are Task 4).
- [ ] **Step 5: Commit** `feat(ui): open the disk as a tree in the list`.

### Task 3: Search below the current folder, and the create row

**Files:**
- Modify: `src/ui/picker.rs` (`key` for `Char`/`Backspace`/`Right`, `root_rows`, `command`)
- Modify: `src/ui/pane.rs` (`open`)
- Test: `src/ui/picker.rs`, `src/ui/pane.rs` `mod tests`

**Interfaces:**
- Consumes: Task 2's state and rows; Task 1's `Disk::search`.
- Produces: `Picker::anchor_of(&Row) -> Option<(Root, String)>`; `pane::open` creates a
  `Create` row's folder (`create_dir_all`) before `new-window`, logs with
  `paths::log_error` and returns without running tmux on failure.

- [ ] **Step 1: Write the failing tests:**
  - `typing_on_a_closed_entry_filters_the_history`: cursor 0, type `api` → `anchor == None`,
    history filtered as today.
  - `typing_in_the_tree_searches_below_the_current_folder`: open `Open`, `→` on `workspace/`,
    type `VT` → `anchor == Some((Open, <home>/workspace))`, the `Open` root row's `branch` is
    `in ~/workspace/` (with `HOME` pointing at the tempdir), first `Dir` label `vtp`, cursor on it.
  - `a_new_name_offers_create_last_under_open_and_first_under_new`: query `Meu-Projeto` below
    `Documents` → `Create` row `path == <home>/Documents/Meu-Projeto`; position last under
    `Open`, first (and selected) under `New`.
  - `create_respects_case`: with `<home>/Documents/meu-projeto`, query `Meu-Projeto` still
    offers `Create`; query `meu-projeto` does not.
  - `text_with_a_slash_only_offers_create`: query `a/b` → no `Dir` rows, `Create` path
    `<home>/Documents/a/b`.
  - `spaces_around_the_text_are_trimmed` (Review Focus): query `"  Meu-Projeto "` →
    `Create` path ends with `/Meu-Projeto`.
  - `right_on_a_match_opens_the_tree_down_to_it`: query `vtp` from home, `Right` → query empty,
    `anchor None`, `dirs` holds `(Open, <home>/workspace)`, cursor on `vtp/`.
  - `clearing_the_text_returns_to_the_tree`.
  - `open_creates_the_folder_then_starts_there` (pane): `FakeTmux`, a `Create` row on a tempdir
    → folder exists, one `new-window -c <path> claude` call.
  - `open_logs_and_starts_nothing_when_create_fails` (pane, Review Focus): a file at the path →
    no tmux call; `CST_STATE_DIR` set to a tempdir and `errors.log` names the path. Tests that
    set environment variables follow the repo's existing pattern for serialising them.
- [ ] **Step 2: Run** `cargo test --lib picker:: pane::` — expect FAIL.
- [ ] **Step 3: Implement.** On the first `Char` of an empty query set
  `anchor = anchor_of(selected row)`: `None` for history rows and for closed entries; open
  entry → `(root, home)`; open `Dir` → its path; closed `Dir` → its parent. `Backspace` to an
  empty query clears `anchor`. `root_rows` lists `Disk::search` hits (label `rel`, branches
  `├ `/last `└ `) or, when the trimmed text holds `/`, only the create row; create offered when
  no `children(anchor dir)` name equals the trimmed text exactly. After each typed key put the
  cursor on: a hit of depth 1 whose name starts with the text (ignoring case); else `Create`
  under `New`, the first hit under `Open`; history rows when `anchor` is `None`.
- [ ] **Step 4: Run** `cargo test --lib` — expect PASS.
- [ ] **Step 5: Commit** `feat(ui): search folders and create one from the list`.

### Task 4: Claude Code look for the panel

**Files:**
- Modify: `src/ui/picker.rs` (`row_lines`, `list_lines`, `screen`; new `root_line`,
  `dir_line`, `search_box`, `hints`, `tilde`; drop the panel `FOOTER`)
- Modify: `tests/golden.rs` (write mode), `tests/golden/expected/panel_*.json`
- Test: `src/ui/picker.rs` `mod tests`

**Interfaces:**
- Produces (used by Task 5):
  - `pub fn search_box(query: &str, placeholder: &str, focused: bool, width: usize) -> Vec<Line<'static>>`
    — three lines; `▌` only when `focused`.
  - `pub fn hints(row: Option<&Row>, picker: &Picker) -> Vec<(&'static str, &'static str)>`
    and `pub fn hints_line(keys: &[(&str, &str)], width: usize) -> Line<'static>` —
    `"{key} to {does}"` in `INACTIVE`, joined by `" · "` in `SUBTLE`, `…` when cut.
  - `pub fn placeholder(picker: &Picker) -> &'static str`.
  - `list_lines` keeps its signature; it now draws a one-column gutter with the pointer.

- [ ] **Step 1: Add a golden write mode:** in `screens_match_python`, when `UPDATE_GOLDENS=1`,
  write `runs(&lines)` pretty-printed to the expected file instead of comparing.
- [ ] **Step 2: Write the failing tests:**
  - `the_selected_row_gets_a_lavender_pointer_not_a_bar`: first line of the selected folder
    starts with `❯` styled `PERMISSION` bold; no span has `bg`.
  - `an_open_entry_shows_its_hint`: `▾ Open another folder…  type to search below`.
  - `a_match_greys_its_parent_and_marks_its_letters`: `lta/vtp-sdk` with hits → `lta/` spans
    `INACTIVE`, `sdk` spans `CLAUDE` bold.
  - `the_search_box_is_rounded_and_holds_the_placeholder`: `╭`, `│ ❯ ▌Search folders…`, `╰`.
  - `hints_follow_the_selected_row`: `Dir` under `Open` → `["Enter to start here",
    "→ to open", "Esc to cancel"]`; `Create` → `Enter to create & start`; `Dir` under `New` →
    `Enter to pick as parent · ← to back`; `Folder` → `Enter to new session · → to sessions`.
- [ ] **Step 3: Run** `cargo test --lib picker::` — expect FAIL.
- [ ] **Step 4: Implement** per Global Constraints' copy and colours; `screen` lays out box
  (3), blank, list (`height - 6` rows), filler, hints line.
- [ ] **Step 5: Regenerate the panel goldens:** `UPDATE_GOLDENS=1 cargo test --test golden`,
  inspect the diff of `tests/golden/expected/panel_*.json` (pointer, box, hints, two entry rows
  only), then `cargo test` — expect PASS.
- [ ] **Step 6: Commit** `feat(ui): give the panel Claude Code's look`.

### Task 5: Landing page

**Files:**
- Modify: `src/ui/landing.rs` (`TOP`, `screen`, drop `search_line` and its test, `FOOTER`)
- Modify: `src/ui/landing_app.rs` (`key`, `reload`)
- Modify: `tests/golden/expected/landing_*.json`
- Test: `src/ui/landing.rs`, `src/ui/landing_app.rs` `mod tests`

**Interfaces:**
- Consumes: Task 4's `search_box`, `hints`, `hints_line`, `placeholder`; Task 1's
  `Disk::clear`.

- [ ] **Step 1: Write the failing tests:**
  - `the_landing_uses_the_search_box`: lines 4–6 are the box; idle placeholder
    `/ to search`; while searching, the query with `▌`.
  - `the_landing_footer_adds_search_and_quit`: hints of the selected row then
    `/ to search · q to quit`.
  - `enter_while_searching_opens_a_disk_folder_at_once` (FakeTmux): open `Open`, `/`, type,
    Enter → one `new-window`.
  - `enter_on_a_new_folder_match_picks_the_parent_and_keeps_searching`: `searching` stays
    true, query empty, `dirs` holds the parent.
  - `a_reload_drops_the_folder_cache`: after a folder is created on disk and `reload` runs with
    stale records, the tree lists it.
- [ ] **Step 2: Run** `cargo test --lib landing` — expect FAIL.
- [ ] **Step 3: Implement.** `TOP` becomes 8 (header 3, blank, box 3, blank). `reload` calls
  `self.picker.disk.clear()` when it reloads folders; `Picker::close` on an entry also clears.
  Enter while searching: `Dir` under `Open` or `Create` → `open` and clear query/anchor; `Dir`
  under `New` → `picker.key(Enter)` and stay searching; else end the search as today.
- [ ] **Step 4: Regenerate the landing goldens** (`UPDATE_GOLDENS=1 cargo test --test golden`),
  inspect the diff, run `cargo test` — expect PASS.
- [ ] **Step 5: Commit** `feat(ui): folder tree and new look on the landing`.

### Task 6: Docs and a trial run

**Files:**
- Modify: `README.md` (Keys section: the two rows, → / ← in the tree, typing, create)

- [ ] **Step 1: Update the README's Keys section** for users (no code, as the README is
  written today).
- [ ] **Step 2: Run** `cargo fmt --check && cargo clippy --all-targets -- -D warnings &&
  cargo test` — expect all PASS.
- [ ] **Step 3: Trial on a private server:** build release, run
  `tmux -L cst-trial-tree -f /dev/null new-session -d -x 75 -y 20 'cst new --pick'` with a
  fake home (`HOME=/tmp/cst-trial-home`) and a fake `claude` first on the server's PATH; drive
  both tasks with `send-keys`, confirm the folder exists and
  `list-windows -F '#{pane_current_path}'` shows it; kill only `cst-trial-tree`.
- [ ] **Step 4: Commit** `docs: describe the folder tree keys`.
