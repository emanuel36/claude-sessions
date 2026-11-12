# Session search in the new-session list

Decisions of 2026-11-12, taken from a prototype the owner picked from (variant A of three, run
on real data). The prototype is the uncommitted worktree `/tmp/cst-search` on branches
`prop/search-a/b/c` (throwaway; read it for reference only; it searched transcripts, which this
design drops).

## Why

The owner wants to find a past session by what it was about, without remembering its folder.
The case that started it: a session about LinkedIn, in a folder they did not recall. Today the
text typed in the Alt+n panel and the landing page only matches folder paths.

## Goals

- Typing finds sessions from any folder whose **title and prompts the user typed**, taken
  together, hold every typed word, ignoring case; the words may sit in different prompts
  (builder's call, settled in review: the goal is finding the session, not one prompt).
- The results show in the same list, as rows of their own, and Enter resumes one.
- Each result says how often the words occur, so the right session stands out.
- Typing stays instant on the owner's history (9 400 prompts, 1 800 sessions, 850 KB of text).

## Non-goals and anti-goals

- **No search in Claude's replies or tool output.** Searching the transcripts (3.2 GB, 42 MB of
  message text) was measured and dropped: it needs its own index and refresh, and the owner's
  case is found by the prompts alone.
- No new index file, no new crate, no external tool.
- No separate search mode or screen.

## Source of the text

`cst` already reads `~/.claude/history.jsonl` incrementally (`src/claude/history.rs`) and keeps
only each session's first prompt (cut to 200 characters) in `history.json` in the state folder.
It keeps **every prompt** of each session instead, whole, in the same store (about 1 MB today).
Commands (prompts starting with `/`) are not kept for search, as today for the first prompt.
The title is the one the list already shows: the transcript title, else the first prompt.

A store written by the current version (first prompts only) must not leave sessions
unsearchable: the builder rereads the history from the start once when the stored form is the
old one.

## The list

When the typed text filters the history (see the folder-tree spec: the cursor was on a history
row or a closed entry when typing started) and at least one session matches, a block sits below
`+ New folder…` and `Open another folder…` and above the folders that match by path:

```
  ▸ + New folder…
  ▸ Open another folder…
  ──────────────────────────────────────────────
  Sessions  5 found
❯ Publicação no LinkedIn em inglês  claude-sessions      3×  20h ago
  Banner options for LinkedIn       cv (deleted)         2×  3d ago
  ──────────────────────────────────────────────
  ▸ vehicle.k265 …
```

- **Row:** title, folder name in grey, occurrences, age right-aligned. Look as in the prototype
  (`prop/search-a`): the header `Sessions` bold with `N found` grey.
- **Occurrences:** how many times the typed words occur in the session's title and prompts,
  summed over the words, shown as `N×`.
- **Order:** sessions whose title alone holds every word first, then last use, newest first.
- **Size:** at most 5 rows shown in the Alt+n panel and 10 on the landing page; the block
  scrolls inside itself as a folder's sessions do today (`SHOWN`, `scroll`).
- No block when the text is blank or nothing matches. A path typed with `/` or `~` (the
  `+ open <path>` row) still shows its row first.
- Text typed inside an open folder tree searches the disk as today, not sessions.
- Sessions open in a window right now are left out, as the list does today.

## Deleted folders

Sessions whose folder no longer exists are hidden from the folder list today. In the search
they **show**, with `(deleted)` after the folder name in grey. Enter on one creates the folder
again (with missing parents) and resumes the session there; Claude Code finds the transcript by
the folder path, so the same path resumes it. A failed creation is logged to `errors.log` and
starts nothing, as the New folder row does.

## Keys

- **Cursor after each key:** on the first folder that matches by path, so `vtp`, Enter still
  starts a new session in that folder (the folder-tree spec's promise); on the first result
  only when no folder matches by path.
- Enter on a result resumes that session in its folder, as Enter on a session row does today.
- On the landing page, as with other rows while searching, Enter acts at once.
- Footer hint on a result: `Enter to resume · Esc to cancel` (landing adds its own keys).

## Constraints

- Matching runs on every key over the kept text; on 850 KB it takes well under 1 ms. No cache
  beyond what the history reader already keeps.
- Rust conventions of the repo: 100-column lines, `cargo fmt`, `cargo clippy -D warnings`,
  `cargo test`; goldens in `tests/golden` gain a case with a session search.

## Left to the builder

- Where the match lives (`picker.rs` or a module of its own) and how the rows carry a session
  that is not under a listed folder.
- Truncation of long titles and folder names in the 77-column panel.
- How cursor movement crosses from the block to the folders below it.
