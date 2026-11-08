# Sidebar and new-session adjustments

Decisions of 2026-11-04, taken from previews the owner picked from. Code follows them.

## Sidebar
- **Groups by urgency:** `NEEDS YOU` (permission, question), `RUNNING` (working, shell),
  `DONE` (finished, unseen) and `IDLE`; a heading only for a group that has sessions. Inside a
  group, windows keep their order.
- **Row tint:** both lines of a session take the status colour as a background. A working
  session's tint breathes (1.4 s period, redrawn 8 times a second only while one works); the
  working mark is a static `●`, no spinner.
- **No tab numbers.** Names start next to the mark. A thin rule separates sessions of a group.
- **Second line:** what the session waits on (`approve`, `question`, `running`), then cost and
  the time Claude spent answering (API time, not time since the start).
- **Today section** (owner's choice A), shown when room is left: activity by hour, lines
  changed, sessions opened and time spent waiting on Claude.
- **Mascot:** same shape, colour changed from orange (close to Claude Code's own) to teal.

## Session state kept by the hook
- `SessionEnd` with reason `clear` or `resume` leaves the record: the session that replaces it
  may start first, and a resumed session keeps its id.

## Launching
- `cst` attaches in the terminal it runs from; `cst --ghostty` opens a Ghostty window that
  leaves Alt+Up/Down to tmux. Alt+1…9 are gone. In the owner's Ghostty config, Alt+Up/Down are
  `performable:`, so they reach tmux wherever no Ghostty split lies that way.
- The landing page stays as the first tab for the life of the server, so closing the last
  session returns to it. Hidden, it reads nothing.

## New-session panel (Alt+n)
- Folders ordered by how many sessions they had, with branch and changed files; → opens the
  folder's last 3 sessions (title, age, cost when known), ← closes them. Enter starts a new
  session or resumes the selected one. Typing searches; an existing typed path is offered first.
- No frequency chart. Messages and model per session are not shown: they need transcript
  parsing the panel does not do.
- fzf is no longer a dependency.

## Round 2 (owner's choices from the visual-adjustments preview)
- **Sidebar B:** no status mark, the tint tells the status and names start in the first
  column. Second line: what it waits on, cost, API time and the model, which shows only whole.
  The current session's tint is a little lighter.
- **Statistics at the bottom,** one pattern: a rule, a grey heading with the total on the
  right. TODAY, USAGE (bar, percent and reset time on one line) and SPEND 7D. TODAY steps
  aside when the sessions need its room.
- **New-session panel B, without branch:** folders by last use, two lines each (name and last
  use; sessions and 7-day spend), a rule between folders, the last 3 sessions under an open
  one. Open sessions are not offered to resume. The popup is 52×22.
- **Landing:** the panel's list on the left, left-aligned and as wide as the terminal allows so
  session titles show whole; the sidebar's statistics (TODAY, USAGE, SPEND 7D) in a column on
  the right. Spend per project and sessions per day are gone. (Round 3: the centred column of
  option B read badly.)
- **Navigation:** closing the session on screen moves to the next session window, never the
  landing page; Alt+Up/Down step over the landing page (`@cs-landing`, set on every `cst`
  start). Hidden sidebars and the hidden landing page read nothing; when a window shows, a
  tmux hook sends F12 to its sidebar or landing pane (the hook never fails: run-shell would
  show a failure over the pane), which redraws at once (a 1 s check
  remains as a fallback). The folder scan (titles, folders that exist) is kept until the
  history changes.

## Round 4 (owner's choices of 2026-11-05)
- **Sidebar in Claude Code's own look** (option A, "transcript"): each session is a `⏺` line
  (bullet in the status colour, name, waiting time) and a `⎿` line (what it waits on, cost, API
  time, model). A working session's bullet is Claude Code's spinner. No group headings, no
  tints: sessions are ordered by urgency (needs you, running, then finished and idle together,
  so looking at a finished session does not move it), a blank line between them, and the current one has a
  lighter background. Colours are Claude Code's dark-theme ones; the mascot is Claude orange.
  Statistics follow the same pattern: `⏺ Today`, `⏺ Usage`, `⏺ Spend 7d` (one block per day).
- **Mascot level with Claude Code's banner:** a blank line above it.
- **Session titles as names:** the hook names a window after the session's title (`/rename`,
  else Claude's own), the folder until there is one. A name given with Alt+r still wins.
- **Navigation:** Alt+Up/Down follow the sidebar's order (`cs step`), not the window order.
- **Folder sessions:** an open folder shows 5 sessions; moving past the last one shown scrolls
  to older ones.
- **No tab bar:** considered and dropped; the sidebar stays the only list of sessions.
