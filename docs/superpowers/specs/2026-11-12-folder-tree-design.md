# Folder tree in the new-session list

Decisions of 2026-11-12, taken from prototypes the owner used and picked from. Code follows
them. The prototype is the uncommitted worktree `/tmp/cst-tree` on branch `prop/tree`
(throwaway; read it for reference only).

## Why

Starting a session needs a folder that is already in Claude's history. The owner wants two more
things, as fast as possible and obvious on screen, with no shortcut to memorise:

1. **New project:** create a folder under a parent that changes from project to project, and
   start a session in it.
2. **Unlisted folder:** start a session in a folder that exists but never had one, so it is not
   in the list.

A first prototype (an fzf popup behind Ctrl+F) was dropped: nothing on screen said how to
create a folder, it listed every subfolder at once, and it left the panel's look.

## Goals

- Both tasks reachable from the Alt+n panel and the landing page, from rows written in the list.
- Interactive navigation of the disk, one level at a time, inside the same list as the history.
- Matching that ignores case everywhere.
- Few keys: today's prototype takes 10 keys to start in `~/workspace/vtp` and 17 to create
  `~/Documents/Meu-Projeto` (11 of them the name).
- The list reads like part of Claude Code.

## Non-goals and anti-goals

- No external tool (fzf, fd): `cst` stays one program.
- No separate mode or screen for the disk: a modal browser was tried and rejected.
- No "start here" row: it was rejected as noise.
- Never create a folder or start a session unless the user pressed Enter on that row.

## The list

Two rows sit at the top of the list, above the history folders, always on show (also while a
search filters the history):

```
  ▸ + New folder…
  ▸ Open another folder…
  ──────────────────────────────
  ▸ vehicle.k265          2m ago
    69 sessions · $331.73 7d
```

The cursor starts on `+ New folder…` (as in the prototype the owner picked). Each row is an
entry to the disk: → or Enter opens it as a tree below it, starting at the home folder; ← or
Enter on the open entry closes it. The two trees are independent.

### The tree

- Subfolders show one level at a time under the entry, drawn with `├ └ │` as a folder's
  sessions are: `├ workspace/`. → on a folder opens its subfolders below it; ← on an open folder
  closes it, and ← on a closed one moves the cursor to the folder holding it (or to the entry).
  Backspace with nothing typed does what ← does.
- **Order, last use first:** folders with a Claude session in them or below them come first,
  newest session first (from the history the panel already reads); the rest follow, newest
  change on disk first (the folder's mtime).
- Hidden folders (name starting with `.`) are left out unless the search starts with `.`;
  `node_modules` and `target` are always left out.
- A folder that cannot be read shows no subfolders, silently.

### Enter

- **Open another folder…:** Enter on a folder starts a session in it.
- **New folder…:** Enter on a folder picks it as the parent: it opens and the cursor stays on
  it, ready for a name. It never starts a session.
- **Create row:** creates the folder (and any missing parents) and starts a session in it.
- History folders and sessions behave as today.

### Typing

- There is one search text. Where the cursor is when the first character is typed decides what
  it searches, until the text is cleared:
  - on a history row, or on an entry row that is **closed**: it filters the history as today
    (so Alt+n, `vtp`, Enter still opens the history folder, although the cursor starts on
    `+ New folder…`);
  - on an **open** entry row or a tree row: it searches the disk **below the current folder**,
    which is the folder under the cursor if it is open, else the folder holding it (the home
    folder on the entry row). The entry row then reads `in ~/workspace/`. The history is
    filtered by the same text meanwhile.
- Text holding a `/` searches nothing on disk: it only offers the create row for that path
  below the current folder (nested folders are created).
- Matches are folders whose name holds the text, ignoring case, up to 6 levels down. Order:
  shallower first; within a level, last use first (as in the tree); then same name, name
  starting with the text, the rest. Each match shows its path below the searched folder, the
  parent part grey and the matched letters in Claude's orange: `lta/vtp-sdk`. At most 50.
- **Create row:** `+ create ~/Documents/Meu-Projeto and start`, offered while nothing exists at
  that path below the current folder (no folder, hidden or not, and no file). The check
  respects case, as Linux does: with `meu-projeto` present, `Meu-Projeto` can still be created.
  Text starting with `~` or `/`, or holding a `.` or `..` component, never offers it. Under New
  folder it comes first; under Open another folder it comes last.
- **Cursor after each key:** on a match one level down whose name starts with the text, if any;
  else on the create row (New folder) or the first match (Open another folder).
- → on a match opens the tree down to it and opens the match itself, clears the search and
  keeps the cursor on it.
- Clearing the text returns to the tree as it was.

## Look

The panel and the landing list take Claude Code's dark-theme look (the colours `words.rs`
already holds):

- **Search box:** the search line sits in a rounded box (`╭─╮ │ ❯ text▌ │ ╰─╯`), border grey,
  `❯` in Claude's orange, a grey placeholder when empty (`Search folders…`; with a tree open,
  `Search folders, or type a new folder's name…`).
- **Selection:** a lavender `❯` (Claude Code's permission colour) in a gutter column before the
  row, and the row's text in lavender. No background bar.
- **Hints:** the footer says what the keys do on the selected row, in grey, in Claude Code's
  form: `Enter to start here · → to open · Esc to cancel`; `Enter to create & start`;
  `Enter to pick as parent · ← to back`; `Enter to new session · → to sessions`.
- The entry rows: grey arrow (`▸`/`▾`), bold name, green `+` for New folder, and when open a grey
  hint (`pick the parent, then type the name`, `type to search below`, `in ~/workspace/`).

## Landing page

The landing list is the same list, so it gets the two rows and the trees, and the owner chose
the full look there too: the rounded search box replaces the `/ to search` line, and the
footer shows the selected row's hints like Alt+n (plus `/ to search` and `q to quit`, which
only the landing has). The mascot and the statistics stay where they are.

Typing still starts with `/`. While searching, Enter on a row of Open another folder or on a
create row opens it at once (today Enter first ends the search), as Alt+n does. Enter on a
folder of New folder picks it as the parent and keeps the search open with the text cleared,
ready for the name. On history rows Enter still only ends the search.

## Prototype parts that do not ship

- The prototype's `CST_PLACE` (row position) and `CST_SOCKET` (tmux socket) variables and its
  other two layouts: the owner picked the two rows at the top, so that is the only layout.
- The key counts above are context, not acceptance criteria.

## Constraints

- Linux only, as the rest of `cst`. Folders are read with `std::fs`, no new crates.
- The panel lives a few seconds: each folder's subfolders and each search's walk are read once
  and kept. The landing page lives for hours: when it reloads the history (a session started or
  ended) it drops each folder's subfolders and order, and it drops a search's walk when the
  search ends or an entry closes, so a search in progress is never walked twice. The walk stops
  at 6 levels or 40 000 folders, so a huge home cannot stall typing.
- Rust conventions of the repo: 100-column lines, `cargo fmt`, `cargo clippy -D warnings`,
  `cargo test`; the screen goldens (`tests/golden`) are updated to the new look.

## The typed path row

The existing `+ open <path>` row for a typed `/…` or `~/…` path that exists stays as it is: it
is the only way to reach folders outside the home folder, where the trees start.

## Left to the builder

- How the tree state, the walk cache and the row kinds are split across modules (the prototype
  put the disk reading in `src/ui/tree.rs`).
- Exact wording of hints and placeholders beyond the ones above, and truncation of long paths.
- A failed folder creation is logged to `errors.log` and starts nothing (builder's call:
  prototype behaviour).
- Scrolling the tree when it is longer than the popup (the list already scrolls to keep the
  selection in view).
