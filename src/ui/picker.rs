//! The folder list that starts a session: folders by last use, each opening its last sessions.
//!
//! The Alt+n panel shows it under a search line; the landing page shows it under its header.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use ratatui::text::{Line, Span};
use serde::Deserialize;

use crate::core::paths;
use crate::shell;
use crate::ui::line::{cell_len, ljust, pad_right, plain, push, rjust, styled, truncate, width_of};
use crate::ui::words::{self, BLUE, CLAUDE, CURRENT, FAINT, GREEN, INK, RULE, SOFT, STRONG};

/// How many of a folder's sessions open under it at once; the rest scroll into view.
pub const SHOWN: usize = 5;
/// The list's width in the Alt+n popup; the landing page gives it all the room it has.
pub const WIDTH: usize = 48;
/// The cells a session's age takes.
const AGE_WIDTH: usize = 4;
/// The cells a session's cost takes.
const SPEND_WIDTH: usize = 8;
/// The keys the Alt+n panel takes.
const FOOTER: [(&str, &str); 4] = [
    ("↑↓", "move"),
    ("→", "sessions"),
    ("Enter", "new / resume"),
    ("Esc", ""),
];
/// Marks the last line on show when the list goes on below it.
const MORE: char = '▼';

/// An earlier session of a folder, to resume.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Session {
    /// The session id.
    pub id: String,
    /// Its title, or its first prompt.
    pub title: String,
    /// When it was last used, in seconds since the epoch.
    pub last: f64,
    /// What it cost over the last 7 days, if anything.
    #[serde(default)]
    pub cost: Option<f64>,
}

/// A folder Claude ran in: when it was last used, its sessions and its week's spend.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Folder {
    /// The folder's path, with no trailing slash.
    pub path: String,
    /// Its last path component, or the path itself for `/`.
    pub name: String,
    /// When a session last used it, in seconds since the epoch.
    pub last: f64,
    /// How many sessions ran in it, the open ones included.
    #[serde(default)]
    pub count: u32,
    /// What its sessions cost over the last 7 days.
    #[serde(default)]
    pub spend: f64,
    /// Its sessions that are not open now, newest first.
    #[serde(default)]
    pub sessions: Vec<Session>,
}

/// What a line of the list is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A typed folder path, to start a session in.
    Path,
    /// A folder Claude ran in.
    Folder,
    /// One of a folder's sessions.
    Session,
}

/// A line of the list: a typed path, a folder, or one of a folder's sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// What the row is.
    pub kind: RowKind,
    /// The typed path, or the folder's path.
    pub path: String,
    /// The folder's index in the folders, for folder and session rows.
    pub folder: Option<usize>,
    /// The session's index in its folder's sessions, for session rows.
    pub session: Option<usize>,
    /// Whether a session is the last of its folder on show, drawn with the closing branch.
    pub end: bool,
}

/// A key the list takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Moves up, or scrolls a folder's sessions back.
    Up,
    /// Moves down, or scrolls a folder's sessions on.
    Down,
    /// Closes a folder, from it or from one of its sessions.
    Left,
    /// Opens a folder's sessions.
    Right,
    /// Opens the selected row.
    Enter,
    /// Deletes the search's last character.
    Backspace,
    /// Types a character into the search.
    Char(char),
}

/// What the user did in the list: the search, the selected row and the open folders.
pub struct Picker {
    /// The folders, last used first.
    pub folders: Vec<Folder>,
    /// What the user typed.
    pub query: String,
    /// The index of the selected row among those shown.
    pub cursor: usize,
    /// The paths of the folders showing their sessions.
    pub expanded: BTreeSet<String>,
    /// By folder path, how many of its sessions are scrolled past.
    pub scroll: HashMap<String, usize>,
    /// Tells whether a typed path (with `~` expanded) is a folder.
    pub is_dir: Box<dyn Fn(&str) -> bool>,
}

impl Default for Picker {
    /// An empty picker that looks on disk, `~` expanded, for typed folders.
    fn default() -> Self {
        Self::new(Box::new(|path| Path::new(&expand_user(path)).is_dir()))
    }
}

impl Picker {
    /// An empty picker that asks `is_dir` whether a typed path is a folder.
    pub fn new(is_dir: Box<dyn Fn(&str) -> bool>) -> Self {
        Self {
            folders: Vec::new(),
            query: String::new(),
            cursor: 0,
            expanded: BTreeSet::new(),
            scroll: HashMap::new(),
            is_dir,
        }
    }

    /// The rows on show, with the cursor kept on one of them.
    pub fn shown(&mut self) -> Vec<Row> {
        let out = rows(
            &self.folders,
            &self.query,
            &self.expanded,
            self.is_dir.as_ref(),
            &self.scroll,
        );
        self.cursor = self.cursor.min(out.len().saturating_sub(1));
        out
    }

    /// The selected row, or `None` when nothing is listed.
    pub fn selected(&mut self) -> Option<Row> {
        let shown = self.shown();
        shown.into_iter().nth(self.cursor)
    }

    /// Applies a key; returns the row to open on Enter, else `None`.
    pub fn key(&mut self, key: Key) -> Option<Row> {
        let row = self.selected();
        match key {
            Key::Enter => return row,
            Key::Up | Key::Down => {
                if !self.scrolls(row.as_ref(), key) {
                    self.cursor = if key == Key::Down {
                        self.cursor + 1
                    } else {
                        self.cursor.saturating_sub(1)
                    };
                }
            }
            Key::Right => {
                if let Some(row) = row.filter(|r| r.kind == RowKind::Folder) {
                    self.expanded.insert(row.path);
                }
            }
            Key::Left => {
                // From a session, back to its folder, which then closes
                if let Some(row) = row.filter(|r| r.kind != RowKind::Path) {
                    self.expanded.remove(&row.path);
                    self.scroll.remove(&row.path);
                    let shown = self.shown();
                    if let Some(i) = shown
                        .iter()
                        .position(|r| r.kind == RowKind::Folder && r.path == row.path)
                    {
                        self.cursor = i;
                    }
                }
            }
            Key::Backspace => {
                self.query.pop();
                self.cursor = 0;
            }
            Key::Char(c) => {
                self.query.push(c);
                self.cursor = 0;
            }
        }
        self.shown();
        None
    }

    /// Scrolls a folder's sessions when the cursor would leave them with more that way.
    ///
    /// Returns whether it scrolled, in which case the cursor stays on its line.
    fn scrolls(&mut self, row: Option<&Row>, key: Key) -> bool {
        let Some(row) = row.filter(|r| r.kind == RowKind::Session) else {
            return false;
        };
        let (Some(folder), Some(session)) = (row.folder, row.session) else {
            return false;
        };
        let count = self.folders.get(folder).map_or(0, |f| f.sessions.len());
        let start = self.scroll.get(&row.path).copied().unwrap_or(0);
        if key == Key::Down && row.end && start + SHOWN < count {
            self.scroll.insert(row.path.clone(), start + 1);
            return true;
        }
        if key == Key::Up && start > 0 && session == start {
            self.scroll.insert(row.path.clone(), start - 1);
            return true;
        }
        false
    }
}

/// Tells whether a folder's path holds every word of the query, ignoring case.
fn matches(folder: &Folder, query: &str) -> bool {
    let path = folder.path.to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| path.contains(word))
}

/// The list's rows: a typed path first, then the folders that match, with sessions below.
///
/// Args:
/// * `folders`: the folders, last used first.
/// * `query`: what the user typed.
/// * `expanded`: the paths of the folders showing their sessions.
/// * `is_dir`: tells whether a typed path (with `~` expanded) is a folder.
/// * `scroll`: by folder path, how many of its sessions are scrolled past.
pub fn rows(
    folders: &[Folder],
    query: &str,
    expanded: &BTreeSet<String>,
    is_dir: &dyn Fn(&str) -> bool,
    scroll: &HashMap<String, usize>,
) -> Vec<Row> {
    let mut out = Vec::new();
    let typed = query.trim();
    if (typed.starts_with('/') || typed.starts_with('~')) && is_dir(typed) {
        out.push(Row {
            kind: RowKind::Path,
            path: typed.to_owned(),
            folder: None,
            session: None,
            end: false,
        });
    }
    for (index, folder) in folders.iter().enumerate() {
        if !matches(folder, query) {
            continue;
        }
        out.push(Row {
            kind: RowKind::Folder,
            path: folder.path.clone(),
            folder: Some(index),
            session: None,
            end: false,
        });
        if expanded.contains(&folder.path) {
            let count = folder.sessions.len();
            let start = scroll.get(&folder.path).copied().unwrap_or(0).min(count);
            let stop = (start + SHOWN).min(count);
            out.extend((start..stop).map(|session| Row {
                kind: RowKind::Session,
                path: folder.path.clone(),
                folder: Some(index),
                session: Some(session),
                end: session + 1 == stop,
            }));
        }
    }
    out
}

/// The rule between two folders.
fn rule(width: usize) -> Line<'static> {
    let text = format!(" {}", "─".repeat(width.saturating_sub(1)));
    styled(&[(&text, RULE)])
}

/// A folder's two lines: name and last use, then its sessions and the week's spend.
fn folder_lines(
    folder: &Folder,
    expanded: &BTreeSet<String>,
    now: f64,
    width: usize,
) -> [Line<'static>; 2] {
    let arrow = if expanded.contains(&folder.path) {
        " ▾ "
    } else {
        " ▸ "
    };
    let age = format!("{} ago", words::age(folder.last, now));
    let age_cells = width_of(&age);
    let mut first = styled(&[(arrow, SOFT)]);
    let name = words::fit(&folder.name, width.saturating_sub(4 + age_cells));
    push(&mut first, name, STRONG);
    let gap = width.saturating_sub(cell_len(&first) + age_cells);
    push(&mut first, " ".repeat(gap), INK);
    push(&mut first, age, SOFT);
    let plural = if folder.count == 1 { "" } else { "s" };
    let spend = if folder.spend == 0.0 {
        String::new()
    } else {
        format!(" · {} 7d", words::money(Some(folder.spend)))
    };
    let second = format!("   {} session{plural}{spend}", folder.count);
    [first, styled(&[(&second, SOFT)])]
}

/// One of a folder's last sessions: its title, when it was used and what it cost.
///
/// A row that is not a session of `folders` draws as an empty line.
fn session_line(row: &Row, folders: &[Folder], now: f64, width: usize) -> Line<'static> {
    let folder = row.folder.and_then(|f| folders.get(f));
    let Some((folder, (index, session))) = folder.and_then(|folder| {
        let index = row.session?;
        Some((folder, (index, folder.sessions.get(index)?)))
    }) else {
        return Line::default();
    };
    // The closing branch only under the folder's very last session: more may scroll into view
    let branch = if index + 1 == folder.sessions.len() {
        "└ ".to_owned()
    } else if row.end {
        format!("{MORE} ")
    } else {
        "├ ".to_owned()
    };
    let mut line = styled(&[("     ", INK), (&branch, FAINT)]);
    let tail = ljust(&words::age(session.last, now), AGE_WIDTH)
        + &rjust(&words::money(session.cost), SPEND_WIDTH);
    let title = width.saturating_sub(cell_len(&line) + width_of(&tail) + 1);
    push(
        &mut line,
        ljust(&words::fit(&session.title, title), title + 1),
        INK,
    );
    push(&mut line, tail, SOFT);
    line
}

/// Every row's lines, with a rule between folders, and the row each line belongs to.
fn row_lines(
    shown: &[Row],
    folders: &[Folder],
    expanded: &BTreeSet<String>,
    now: f64,
    width: usize,
) -> (Vec<Line<'static>>, Vec<Option<usize>>) {
    let mut lines = Vec::new();
    let mut owners = Vec::new();
    for (i, row) in shown.iter().enumerate() {
        let drawn: Vec<Line<'static>> = match row.kind {
            RowKind::Path => {
                let open = words::fit(&format!("open {}", row.path), width.saturating_sub(5));
                vec![styled(&[("   + ", GREEN), (&open, INK)])]
            }
            RowKind::Session => vec![session_line(row, folders, now, width)],
            RowKind::Folder => {
                if !lines.is_empty() {
                    lines.push(rule(width));
                    owners.push(None);
                }
                match row.folder.and_then(|f| folders.get(f)) {
                    Some(folder) => folder_lines(folder, expanded, now, width).into(),
                    None => Vec::new(),
                }
            }
        };
        owners.extend(std::iter::repeat_n(Some(i), drawn.len()));
        lines.extend(drawn);
    }
    (lines, owners)
}

/// Drops a line's first character, keeping the styles of the rest.
fn drop_first(line: &mut Line<'static>) {
    if let Some(span) = line.spans.iter_mut().find(|s| !s.content.is_empty()) {
        let mut chars = span.content.chars();
        chars.next();
        span.content = chars.as_str().to_owned().into();
    }
}

/// The rows around the selected one, in `room` lines at most, the selection highlighted.
///
/// Args:
/// * `shown`: the rows from [`rows`].
/// * `folders`: the folders the rows point into.
/// * `cursor`: the index of the selected row.
/// * `expanded`: the paths of the folders showing their sessions.
/// * `now`: current time as a Unix timestamp.
/// * `room`: lines available.
/// * `width`: columns available.
pub fn list_lines(
    shown: &[Row],
    folders: &[Folder],
    cursor: usize,
    expanded: &BTreeSet<String>,
    now: f64,
    room: usize,
    width: usize,
) -> Vec<Line<'static>> {
    if shown.is_empty() {
        return vec![styled(&[("   nothing matches", FAINT)])];
    }
    let (lines, owners) = row_lines(shown, folders, expanded, now, width);
    let total = lines.len();
    let picked: Vec<usize> = owners
        .iter()
        .enumerate()
        .filter(|(_, owner)| **owner == Some(cursor))
        .map(|(n, _)| n)
        .collect();
    // Scrolls just enough to keep every line of the selected row in view
    let top = match (picked.first(), picked.last()) {
        (Some(&first), Some(&last)) => (last + 1)
            .saturating_sub(room)
            .min(total.saturating_sub(room))
            .min(first),
        _ => 0,
    };
    let end = total.min(top + room);
    let mut out = Vec::with_capacity(end.saturating_sub(top));
    for (n, mut line) in lines.into_iter().enumerate().take(end).skip(top) {
        // A session line's branch already marks its folder's own more below
        if n + 1 == end && end < total && plain(&line).chars().nth(5) != Some(MORE) {
            // Its first column is always blank: room for the mark that more lies below
            drop_first(&mut line);
            line.spans.insert(0, Span::styled(MORE.to_string(), FAINT));
        }
        if picked.contains(&n) {
            pad_right(&mut line, width);
            for span in &mut line.spans {
                span.style = span.style.patch(CURRENT);
            }
        }
        out.push(line);
    }
    out
}

/// The keys a screen takes, each in Claude's orange before what it does, if anything.
///
/// A line too long for `width` is cut with an ellipsis.
pub fn keys_line(keys: &[(&str, &str)], width: usize) -> Line<'static> {
    let mut line = Line::from(Span::raw(" "));
    for (i, (key, does)) in keys.iter().enumerate() {
        if i > 0 {
            push(&mut line, " · ", FAINT);
        }
        push(&mut line, *key, CLAUDE);
        if !does.is_empty() {
            push(&mut line, format!(" {does}"), INK);
        }
    }
    if cell_len(&line) > width {
        truncate(&mut line, width.saturating_sub(1));
        push(&mut line, "…", FAINT);
    }
    line
}

/// The Alt+n panel as `height` lines: the search line, the list and the keys.
pub fn screen(picker: &mut Picker, now: f64, height: u16) -> Vec<Line<'static>> {
    let shown = picker.shown();
    let height = usize::from(height);
    let prompt = styled(&[(" ❯ ", BLUE), (&picker.query, INK), ("▌", BLUE)]);
    let mut lines = vec![prompt, Line::default()];
    lines.extend(list_lines(
        &shown,
        &picker.folders,
        picker.cursor,
        &picker.expanded,
        now,
        height.saturating_sub(4),
        WIDTH,
    ));
    while lines.len() < height.saturating_sub(1) {
        lines.push(Line::default());
    }
    lines.push(keys_line(&FOOTER, WIDTH));
    lines
        .into_iter()
        .map(|line| {
            if plain(&line).is_empty() {
                Line::raw(" ")
            } else {
                line
            }
        })
        .take(height)
        .collect()
}

/// A path with a leading `~` or `~/` turned into the home folder, as `os.path.expanduser`
/// does; `~user` forms are left as they are.
fn expand_user(path: &str) -> String {
    let rest = match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest,
        _ => return path.to_owned(),
    };
    let home = paths::home(&paths::process_env);
    let expanded = format!("{}{rest}", home.to_string_lossy().trim_end_matches('/'));
    if expanded.is_empty() {
        "/".to_owned()
    } else {
        expanded
    }
}

/// The tmux `new-window` arguments that open a row: resume its session, or start one.
pub fn command(row: &Row, folders: &[Folder]) -> Vec<String> {
    let session = row
        .folder
        .and_then(|f| folders.get(f))
        .zip(row.session)
        .and_then(|(folder, s)| folder.sessions.get(s));
    match session {
        Some(session) if row.kind == RowKind::Session => vec![
            "-c".to_owned(),
            row.path.clone(),
            format!("claude --resume {}", shell::quote(&session.id)),
        ],
        _ => vec!["-c".to_owned(), expand_user(&row.path), "claude".to_owned()],
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use ratatui::style::Color;
    use ratatui::text::Line;

    use crate::ui::line::cells;
    use crate::ui::picker::{
        command, keys_line, list_lines, rows, screen, session_line, Folder, Key, Picker, Row,
        RowKind, Session, WIDTH,
    };

    const NOW: f64 = 1_700_000_000.0;

    fn sessions() -> Vec<Session> {
        (0..7)
            .map(|i| Session {
                id: format!("s{i}"),
                title: format!("task {i}"),
                last: NOW - 3600.0 * f64::from(i),
                cost: (i == 0).then_some(1.5),
            })
            .collect()
    }

    fn folders() -> Vec<Folder> {
        vec![
            Folder {
                path: "/w/api".into(),
                name: "api".into(),
                last: NOW - 60.0,
                count: 41,
                spend: 12.5,
                sessions: sessions(),
            },
            Folder {
                path: "/w/web".into(),
                name: "web".into(),
                last: NOW - 7200.0,
                count: 1,
                spend: 0.0,
                sessions: sessions()[..2].to_vec(),
            },
        ]
    }

    fn picker(is_dir: bool) -> Picker {
        let mut p = Picker::new(Box::new(move |_| is_dir));
        p.folders = folders();
        p
    }

    fn set(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    fn plain(line: &Line) -> String {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        text.trim_end().to_owned()
    }

    fn no_dirs(_: &str) -> bool {
        false
    }

    fn session_ids(shown: &[Row], folders: &[Folder]) -> Vec<String> {
        shown
            .iter()
            .filter(|r| r.kind == RowKind::Session)
            .map(|r| {
                let (f, s) = (r.folder.unwrap(), r.session.unwrap());
                folders[f].sessions[s].id.clone()
            })
            .collect()
    }

    #[test]
    fn rows_list_folders_and_open_a_folders_last_five_sessions() {
        let fs = folders();
        let shown = rows(&fs, "", &set(&["/w/api"]), &no_dirs, &HashMap::new());
        let kinds: Vec<_> = shown.iter().map(|r| (r.kind, r.path.as_str())).collect();
        let mut want = vec![(RowKind::Folder, "/w/api")];
        want.extend([(RowKind::Session, "/w/api"); 5]);
        want.push((RowKind::Folder, "/w/web"));
        assert_eq!(kinds, want);
        assert_eq!(session_ids(&shown, &fs), ["s0", "s1", "s2", "s3", "s4"]);
        let ends: Vec<_> = shown
            .iter()
            .filter(|r| r.kind == RowKind::Session)
            .map(|r| r.end)
            .collect();
        assert_eq!(ends, [false, false, false, false, true]);
    }

    #[test]
    fn a_folders_sessions_scroll_past_the_last_and_back() {
        let mut p = picker(false);
        p.key(Key::Right);
        for _ in 0..7 {
            p.key(Key::Down);
        }
        // Two steps past the fifth session scroll by two; the cursor stays on the last line
        assert_eq!(p.cursor, 5);
        assert_eq!(p.scroll, HashMap::from([("/w/api".to_owned(), 2)]));
        let shown = p.shown();
        assert_eq!(
            session_ids(&shown, &p.folders),
            ["s2", "s3", "s4", "s5", "s6"]
        );
        let selected = p.selected().unwrap();
        let last = plain(&session_line(&selected, &p.folders, NOW, WIDTH));
        assert!(last.starts_with("     └ task 6"), "{last}");
        // With nothing older left, the cursor moves on to the next folder
        p.key(Key::Down);
        assert_eq!(p.selected().unwrap().path, "/w/web");
        // Back up to the first session on show, then one more scrolls back by one
        for _ in 0..6 {
            p.key(Key::Up);
        }
        assert_eq!(p.cursor, 1);
        assert_eq!(p.scroll, HashMap::from([("/w/api".to_owned(), 1)]));
        p.key(Key::Left);
        assert!(p.scroll.is_empty() && p.expanded.is_empty());
    }

    #[test]
    fn a_folder_stays_closed_until_expanded() {
        let shown = rows(&folders(), "", &set(&[]), &no_dirs, &HashMap::new());
        let kinds: Vec<_> = shown.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [RowKind::Folder, RowKind::Folder]);
    }

    #[test]
    fn typing_filters_by_every_word_and_a_typed_folder_comes_first() {
        let fs = folders();
        let none = HashMap::new();
        let paths: Vec<_> = rows(&fs, "W E", &set(&[]), &no_dirs, &none)
            .into_iter()
            .map(|r| r.path)
            .collect();
        assert_eq!(paths, ["/w/web"]);
        let shown = rows(&fs, "~/x", &set(&[]), &|_| true, &none);
        assert_eq!(
            (shown[0].kind, shown[0].path.as_str()),
            (RowKind::Path, "~/x")
        );
        assert!(rows(&fs, "/nope", &set(&[]), &no_dirs, &none).is_empty());
    }

    #[test]
    fn screen_draws_two_lines_per_folder_and_the_open_sessions() {
        let mut p = picker(false);
        // Python's test draws rows for "" under the query "ap"; here the rows follow the
        // query, and "w" keeps both folders
        p.query = "w".into();
        p.expanded = set(&["/w/api"]);
        let lines = screen(&mut p, NOW, 16);
        assert_eq!(lines.len(), 16);
        let text: Vec<String> = lines.iter().map(plain).collect();
        for t in &text {
            assert!(t.chars().map(cells).sum::<usize>() <= WIDTH, "{t}");
        }
        assert_eq!(text[0], " ❯ w▌");
        assert!(text[2].starts_with(" ▾ api") && text[2].ends_with("1m ago"));
        assert_eq!(text[3], "   41 sessions · $12.50 7d");
        assert!(text[4].starts_with("     ├ task 0") && text[4].ends_with("$1.50"));
        assert!(text[6].starts_with("     ├ task 2") && text[6].contains("2h"));
        // More sessions scroll into view below: a mark, no closing branch yet
        assert!(text[8].starts_with("     ▼ task 4"), "{}", text[8]);
        assert_eq!(text[9].trim(), "─".repeat(WIDTH - 1));
        // No spend this week: no spend part
        assert!(text[10].starts_with(" ▸ web"));
        assert_eq!(text[11], "   1 session");
        assert!(text[15].trim().starts_with("↑↓ move"));
        // Both lines of the selected folder have the highlight
        let lit = |line: &Line| {
            line.spans
                .iter()
                .any(|s| s.style.bg == Some(Color::Indexed(235)))
        };
        assert!(lit(&lines[2]) && lit(&lines[3]));
        assert!(!lit(&lines[10]));
    }

    #[test]
    fn the_list_scrolls_to_keep_the_selected_row_in_view() {
        let many: Vec<Folder> = (0..30)
            .map(|i| Folder {
                path: format!("/w/f{i}"),
                name: format!("f{i}"),
                last: NOW - f64::from(i),
                count: 1,
                spend: 0.0,
                sessions: Vec::new(),
            })
            .collect();
        let none = set(&[]);
        let shown = rows(&many, "", &none, &no_dirs, &HashMap::new());
        let lines: Vec<String> = list_lines(&shown, &many, 25, &none, NOW, 8, WIDTH)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(lines.len(), 8);
        assert!(lines.iter().any(|x| x.contains(" f25")));
        assert_eq!(lines[7], "▼  1 session");
        assert!(!lines.iter().any(|x| x.contains(" f0")));
        // At the bottom of the list nothing more lies below
        let end = list_lines(&shown, &many, 29, &none, NOW, 8, WIDTH);
        assert_eq!(plain(&end[7]), "   1 session");
    }

    #[test]
    fn nothing_matching_says_so() {
        let lines = list_lines(&[], &[], 0, &set(&[]), NOW, 8, WIDTH);
        assert_eq!(plain(&lines[0]), "   nothing matches");
    }

    #[test]
    fn the_picker_moves_opens_and_types() {
        let mut p = picker(false);
        assert_eq!(p.key(Key::Right), None);
        assert!(p.expanded.contains("/w/api"));
        p.key(Key::Down);
        let opened = p.key(Key::Enter).unwrap();
        assert_eq!(opened.session, Some(0));
        assert_eq!(p.folders[0].sessions[0].id, "s0");
        p.key(Key::Left);
        assert_eq!(p.cursor, 0);
        assert!(p.expanded.is_empty());
        p.key(Key::Char('w'));
        assert_eq!((p.query.as_str(), p.cursor), ("w", 0));
        let selected = p.selected().unwrap();
        assert_eq!(command(&selected, &p.folders), ["-c", "/w/api", "claude"]);
        p.key(Key::Backspace);
        assert_eq!(p.query, "");
    }

    #[test]
    fn left_returns_to_the_folder_even_when_a_typed_path_matches_it() {
        let mut p = picker(true);
        for c in "/w/api".chars() {
            p.key(Key::Char(c));
        }
        for key in [Key::Down, Key::Right, Key::Down, Key::Left] {
            p.key(key);
        }
        assert_eq!(p.selected().unwrap().kind, RowKind::Folder);
        assert!(p.expanded.is_empty());
    }

    #[test]
    fn command_quotes_session_ids() {
        let mut fs = folders();
        fs[0].sessions[0].id = "a b".into();
        let row = Row {
            kind: RowKind::Session,
            path: "/w/api".into(),
            folder: Some(0),
            session: Some(0),
            end: false,
        };
        assert_eq!(
            command(&row, &fs),
            ["-c", "/w/api", "claude --resume 'a b'"]
        );
    }

    #[test]
    fn command_expands_the_home_of_a_typed_path() {
        let row = Row {
            kind: RowKind::Path,
            path: "~/x".into(),
            folder: None,
            session: None,
            end: false,
        };
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            command(&row, &[]),
            ["-c".to_owned(), format!("{home}/x"), "claude".to_owned()]
        );
    }

    #[test]
    fn keys_line_cuts_a_long_line_with_an_ellipsis() {
        let keys = [("↑↓", "move"), ("Enter", "new / resume"), ("Esc", "")];
        assert_eq!(
            plain(&keys_line(&keys, 40)),
            " ↑↓ move · Enter new / resume · Esc"
        );
        assert_eq!(plain(&keys_line(&keys, 12)), " ↑↓ move · …");
    }
}
