# Session Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Text typed in the Alt+n panel and the landing page also finds sessions from any folder
by their title and typed prompts, listed in a `Sessions` block that resumes them on Enter.

**Architecture:** The history reader keeps every typed prompt per session. `Scan` turns the
history into searchable sessions (deleted folders included) once per history generation. A new
pure module `src/ui/search.rs` matches and ranks them; `picker.rs` shows the matches as a new
row kind `Hit` between the entries and the folders; `pane::open` recreates a deleted folder
before resuming.

**Tech Stack:** Rust, ratatui; no new crates.

**Spec:** `docs/superpowers/specs/2026-11-12-session-search-design.md` (read it first; the
layout and decisions come from there). Earlier list behaviour:
`docs/superpowers/specs/2026-11-12-folder-tree-design.md`.

## Global Constraints

- Searched text: the session title plus every typed prompt; prompts starting with `/` are not
  kept for search; no transcript text, no pasted content.
- Matching ignores case; every typed word must occur somewhere in title + prompts together.
- Order: sessions whose title alone holds every word first, then last use, newest first.
- Block: header `Sessions` (bold, `STRONG`) then `  N found` (grey, `INACTIVE`), N = all matches.
- Row: title, folder name in grey with ` (deleted)` when the folder is gone, `N×`, age.
- At most 5 rows on show in the Alt+n panel, 10 on the landing page; the block scrolls.
- No block when the text is blank or nothing matches; text typed inside an open tree searches
  the disk as today and shows no block.
- Sessions open now are never listed.
- Cursor after a key (no tree searched): typed-path row, else first folder row, else first hit.
- Enter on a hit: `create_dir_all` its folder (failure → `errors.log`, nothing starts), then
  `new-window -c <path> claude --resume <id>`.
- Repo rules: 100-column lines, absolute `crate::` imports, `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test` all clean before each commit.
- Commits: Conventional Commits, subject ≤ 50 characters, body bullets, ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- **Accented text in another case:** `INGLÊS` must find a prompt with `inglês` (Task 2 test).
- **A stored history from the current version:** it holds no prompts; it must be read again from
  the start, not leave every session unsearchable (Task 1 test).
- **A title that is the first prompt:** an untitled session must not count its first prompt
  twice in `N×` (Task 2 test).
- **The narrow panel:** at `WIDTH` (48) a long title and long folder name still fit one line
  with the age on show (Task 4 test).
- **Typing then deleting:** clearing the text removes the block and resets its scroll, and the
  cursor goes back to a valid row (Task 4 test).

---

### Task 1: The history keeps every prompt

**Files:**
- Modify: `src/claude/history.rs` (`HistorySession`, `Sessions::add_line`, tests)
- Modify: `src/claude/usage.rs:477`, `src/ui/folders.rs:158` (test constructors gain the field)

**Interfaces:**
- Produces: `HistorySession.prompts: Vec<String>` — every prompt of the session not starting
  with `/`, whole, in the order typed. `first_prompt` stays as it is.

- [ ] **Step 1: Write the failing tests** in `history.rs`'s test module:

```rust
#[test]
fn every_prompt_but_commands_is_kept_whole() {
    let long = "x".repeat(PROMPT_CHARS + 50);
    let sessions = read_history(&[
        &line("s1", "/p", 1.0, "/resume"),
        &line("s1", "/p", 2.0, "first"),
        &line("s1", "/p", 3.0, &long),
    ]);
    assert_eq!(sessions[0].prompts, vec!["first".to_owned(), long]);
}

#[test]
fn a_stored_reading_without_prompts_is_read_again() {
    // Write a history and a stored reading in the old form (sessions without `prompts`),
    // load: the sessions come back with their prompts.
}
```

Use the file's existing helpers for history lines and the temp env (see the tests around
`history.rs:540-575`); name them as they are, `line(...)` above stands for that helper.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --lib claude::history`
Expected: FAIL to compile (`no field prompts`).

- [ ] **Step 3: Add `pub prompts: Vec<String>` to `HistorySession`** (doc comment: the prompts
  typed, commands left out, whole) with **no** `#[serde(default)]`: a stored reading without it
  then fails to parse in `load_stored`, which already falls back to reading from the start.
  `add_line` pushes each non-empty prompt that does not start with `/`.

- [ ] **Step 4: Run the tests**

Run: `cargo test`
Expected: PASS (fix the two test constructors in `usage.rs` and `folders.rs` with
`prompts: Vec::new()`).

- [ ] **Step 5: Commit** — `feat(claude): keep every prompt of a session`

---

### Task 2: Matching and ranking sessions

**Files:**
- Create: `src/ui/search.rs` (add `pub mod search;` to `src/ui/mod.rs`)

**Interfaces:**
- Produces:

```rust
/// A session the search looks in, from any folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Searchable {
    pub id: String,
    pub title: String,
    /// The folder's path, no trailing slash.
    pub path: String,
    /// The folder's last path component.
    pub name: String,
    pub last: f64,
    /// Whether the folder no longer exists.
    pub gone: bool,
    /// The title, lowercased.
    title_lower: String,
    /// The title when the transcript gave it, and every prompt, lowercased, joined by `\n`.
    text_lower: String,
}

impl Searchable {
    /// `own_title`: whether `title` came from the transcript rather than the first prompt; only
    /// then is it added to the searched text, so a first prompt is not counted twice.
    pub fn new(id: String, title: String, path: String, last: f64, gone: bool,
               own_title: bool, prompts: &[String]) -> Self;
}

/// A match: the session's index in the searched slice and how often the words occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit { pub index: usize, pub count: usize }

/// The sessions holding every word of `query`, ignoring case, ranked: title holding every
/// word first, then newest first. Empty when `query` is blank.
pub fn hits(sessions: &[Searchable], query: &str) -> Vec<Hit>;
```

`count` sums, over the words, the non-overlapping occurrences (`str::matches`) in
`text_lower`. `name` is computed in `new` the way `folders::name` does it (move that helper to
`search.rs` as `pub fn name(path: &str) -> String` and use it from `folders.rs`).

- [ ] **Step 1: Write the failing tests** in `search.rs`:

```rust
#[test]
fn words_may_sit_in_different_prompts() {
    let s = [searchable("a", "Banner", 10.0, &["about LinkedIn", "em inglês"])];
    assert_eq!(hits(&s, "linkedin INGLÊS"), vec![Hit { index: 0, count: 2 }]);
}

#[test]
fn every_word_must_occur() {
    let s = [searchable("a", "Banner", 10.0, &["about LinkedIn"])];
    assert!(hits(&s, "linkedin python").is_empty());
}

#[test]
fn title_matches_come_first_then_the_newest() {
    let s = [
        searchable("old", "x", 1.0, &["linkedin"]),
        searchable("new", "x", 3.0, &["linkedin"]),
        searchable("titled", "LinkedIn post", 2.0, &["hello"]),
    ];
    let order: Vec<usize> = hits(&s, "linkedin").iter().map(|h| h.index).collect();
    assert_eq!(order, vec![2, 1, 0]);
}

#[test]
fn a_first_prompt_title_is_not_counted_twice() {
    let s = [Searchable::new("a".into(), "linkedin".into(), "/p".into(), 1.0, false, false,
                             &["linkedin".to_owned()])];
    assert_eq!(hits(&s, "linkedin")[0].count, 1);
}

#[test]
fn blank_text_finds_nothing() {
    let s = [searchable("a", "x", 1.0, &["y"])];
    assert!(hits(&s, "   ").is_empty());
}
```

(`searchable(id, title, last, prompts)` is a test helper calling `Searchable::new` with
`own_title: true`, path `/p`, `gone: false`.)

- [ ] **Step 2: Run them to see them fail** — `cargo test --lib ui::search` → FAIL to compile.
- [ ] **Step 3: Implement** `Searchable::new`, `hits`, `name`.
- [ ] **Step 4: Run** — `cargo test --lib ui::search` → PASS; `cargo test` → PASS.
- [ ] **Step 5: Commit** — `feat(ui): match sessions by title and prompts`

---

### Task 3: The scan offers searchable sessions, deleted folders included

**Files:**
- Modify: `src/ui/folders.rs` (`Found`, `Scan::scan`, `Scan::load`, new `Scan::searchable`)

**Interfaces:**
- Consumes: `HistorySession.prompts` (Task 1), `Searchable::new`, `search::name` (Task 2).
- Produces: `pub fn searchable(&self, opened: &HashSet<String>) -> Vec<Searchable>` on `Scan`:
  the sessions of the last `load`, open ones left out, in the scan's order. Call it after `load`.

`scan` keeps sessions whose folder is gone (it skips only those with an empty title) and builds
each `Searchable` there, once per generation, kept in `Scan` beside `found`; `Found` gains
`gone: bool` and `load` skips gone ones, so `Folder`s are unchanged.

- [ ] **Step 1: Write the failing tests** in `folders.rs`:

```rust
#[test]
fn a_session_of_a_deleted_folder_is_searchable_but_not_listed() {
    // history: one session in a folder that does not exist
    // load(...) has no folder for it; searchable(&empty) has it with gone == true
}

#[test]
fn an_open_session_is_not_searchable() {
    // searchable(&{id}) leaves it out
}
```

Build them with this file's existing test helpers (around `folders.rs:150-280`).

- [ ] **Step 2: Run them to see them fail** — `cargo test --lib ui::folders` → FAIL.
- [ ] **Step 3: Implement** as described above.
- [ ] **Step 4: Run** — `cargo test` → PASS.
- [ ] **Step 5: Commit** — `feat(ui): offer sessions of any folder to search`

---

### Task 4: The Sessions block in the list

**Files:**
- Modify: `src/ui/picker.rs`

**Interfaces:**
- Consumes: `Searchable`, `Hit`, `hits` (Task 2).
- Produces, on `Picker`:
  - `pub sessions: Vec<Searchable>` — what the search looks in (empty by default).
  - `pub found_shown: usize` — how many hits show at once; default `SHOWN` (5).
  - `found_scroll: usize` (private) — hits scrolled past; reset to 0 whenever the text changes.
  - `RowKind::Hit`; a hit row has `path` = the session's folder, `session` = `Some(index into
    picker.sessions)`, and a new `Row` field `pub id: String` = the session id (empty for the
    other kinds). `command(row, folders)` resumes a `Hit` with `row.id` and `row.path`.
  - `pub fn found(&self) -> Vec<Hit>` — `hits(&self.sessions, &self.query)`, or empty while
    `self.anchor` is set (text typed in a tree).

Rows: `shown()` inserts the hit rows on show (`found_scroll .. found_scroll + found_shown`)
after the entry rows and the typed-path row, before the folders. `row_lines` draws, before the
first hit, the header line `   Sessions` + `  N found`; the existing rule logic already puts a
rule after the entries and before the next folder. A hit line, by `hit_line(row, sessions,
count, now, width)`: three spaces, the title (`INK`), two spaces, the folder name plus
` (deleted)` when gone (`INACTIVE`), then right-aligned `N×` and the age as `session_line` draws
it (`SOFT`). The folder name takes at most a third of the room; the title gets the rest, cut
with `words::fit`. A hit cut off below shows the `▼` mark as session rows do.

Keys: `after_typing` with no tree searched puts the cursor on the typed-path row, else the first
`Folder` row, else the first `Hit` row, else the first history row (today's fallback). Up/Down
on the first/last hit on show scroll the block when more lie that way, as `scrolls` does for a
folder's sessions. `hints` gives `("Enter", "resume")` on a hit.

- [ ] **Step 1: Write the failing tests** in `picker.rs`'s test module (build a `Picker` with
  `sessions` from `Searchable::new` and folders from the existing `sessions()` helper):

```rust
#[test]
fn typed_text_lists_the_sessions_found_under_a_header() {
    // query "linkedin", two matching sessions in a folder that does not match by path:
    // the plain text of list_lines holds "Sessions  2 found", then both titles in rank order,
    // each with its folder name and "N×"; the cursor is on the first hit.
}

#[test]
fn a_folder_matching_by_path_keeps_the_cursor() {
    // folder "vtp" and a session whose prompt says "vtp": after typing "vtp" the selected row
    // is the Folder row, and Enter returns that Folder row.
}

#[test]
fn a_deleted_folder_reads_deleted() {
    // gone == true → the hit line holds "(deleted)".
}

#[test]
fn a_hit_line_fits_the_narrow_panel() {
    // long title and folder name at WIDTH: cell_len(line) <= WIDTH and the line ends with the age.
}

#[test]
fn the_block_shows_found_shown_hits_and_scrolls() {
    // 7 hits, found_shown 5: 5 hit rows on show; Down from the 5th scrolls (6th now on show,
    // 1st gone); Up back at the top scrolls back.
}

#[test]
fn clearing_the_text_drops_the_block() {
    // type "linkedin", Down, then Backspace to empty: no Hit row is shown, found_scroll is 0
    // and selected() is Some.
}

#[test]
fn text_typed_in_a_tree_finds_no_sessions() {
    // anchor set (open the Open entry, type): found() is empty.
}

#[test]
fn enter_on_a_hit_resumes_it_in_its_folder() {
    // command(&hit_row, &folders) == ["-c", "/p", "claude --resume 'id'"]
}
```

- [ ] **Step 2: Run them to see them fail** — `cargo test --lib ui::picker` → FAIL.
- [ ] **Step 3: Implement** as described above.
- [ ] **Step 4: Run** — `cargo test` → PASS (existing picker tests unchanged).
- [ ] **Step 5: Commit** — `feat(ui): list the sessions a search finds`

---

### Task 5: Wire the panel and the landing page; resume in a deleted folder

**Files:**
- Modify: `src/ui/pane.rs` (`open_with`), `src/ui/panel_app.rs` (`folders`, `main_panel`),
  `src/ui/landing_app.rs` (`Landing::new`, `load_folders`, `enter_searching`)

**Interfaces:**
- Consumes: `Scan::searchable` (Task 3), `Picker.sessions`, `Picker.found_shown`,
  `RowKind::Hit` (Task 4).

- `open_with`: for `RowKind::Hit` as for `RowKind::Create`, `create_dir_all(&row.path)` first;
  on failure log `cannot create <path>: <e>` and start nothing.
- Panel: `folders(now)` returns the searchable sessions too (same `opened` set) and
  `main_panel` sets `picker.sessions`.
- Landing: `found_shown = 10`; `load_folders` sets `picker.sessions` from
  `self.scan.searchable(&self.live)`; `enter_searching` treats `RowKind::Hit` like a create row:
  open it at once, then end the search.

- [ ] **Step 1: Write the failing tests**

```rust
// pane.rs
#[test]
fn a_hit_in_a_deleted_folder_creates_it_and_resumes() {
    // row kind Hit, path in a temp dir that does not exist: open_with creates it and the fake
    // tmux got ["new-window", "-c", path, "claude --resume 'id'"].
}

// landing_app.rs
#[test]
fn enter_while_searching_on_a_hit_resumes_at_once() {
    // page with picker.sessions holding a match for "linkedin"; press "/", type "linkedin",
    // Enter: the fake tmux ran new-window with --resume, and the search ended.
}

#[test]
fn the_landing_shows_ten_hits() {
    // 12 matching sessions: 10 Hit rows in picker.shown().
}
```

Use the existing fakes (`FakeTmux`, `page()`, `press`, `search`) of those modules.

- [ ] **Step 2: Run them to see them fail** — `cargo test --lib ui::` → FAIL.
- [ ] **Step 3: Implement** as described above.
- [ ] **Step 4: Run** — `cargo test` → PASS.
- [ ] **Step 5: Commit** — `feat(ui): search sessions in Alt+n and the landing`

---

### Task 6: Goldens and README

**Files:**
- Modify: `tests/golden.rs` (`ListFixture` gains `#[serde(default)] sessions: Vec<...>` turned
  into `Searchable`s), `tests/golden/fixtures.json`, new
  `tests/golden/expected/panel_session_search.json`, `README.md` (the keys table near line 76)

- [ ] **Step 1: Add the fixture** `panel_session_search`: query `linkedin`, one folder that does
  not match, three sessions found (one titled, one in a gone folder, one content-only).
- [ ] **Step 2: Generate its expected file** the way the existing goldens are regenerated (see
  `tests/golden.rs` and `tests/support/golden`); read the result and check it shows the header,
  the three rows in rank order and `(deleted)`.
- [ ] **Step 3: README:** in the keys table, the typing row says it also finds sessions from
  any folder by title or prompt, and Enter on one resumes it (recreating a deleted folder).
  Users' wording, no code.
- [ ] **Step 4: Run all checks** — `cargo fmt --check && cargo clippy --all-targets -- -D
  warnings && cargo test` → all clean.
- [ ] **Step 5: Commit** — `test: golden for the session search` (README in a separate
  `docs: describe the session search` commit).
