//! How the picker draws: its list, the search box and what the keys do.

use std::collections::BTreeSet;

use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};

use crate::ui::line::{cell_len, cells, ljust, push, rjust, styled, truncate, width_of};
use crate::ui::picker::{Folder, Picker, Root, Row};
use crate::ui::search::{Hit, Searchable};
use crate::ui::words::{
    self, CLAUDE, FAINT, GREEN, INACTIVE, INK, PERMISSION, RULE, SOFT, STRONG, SUBTLE,
};

/// The cells a session's age takes.
const AGE_WIDTH: usize = 4;
/// The cells a session's cost takes.
const SPEND_WIDTH: usize = 8;
/// Marks the last line on show when the list goes on below it.
const MORE: char = '▼';

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
/// # Arguments
/// * `folder`: the session's folder.
/// * `index`: the session's index in its folder's sessions; one past them draws an empty line.
/// * `end`: whether it is the last of its folder on show.
/// * `now`: current time as a Unix timestamp.
/// * `width`: columns available.
fn session_line(folder: &Folder, index: usize, end: bool, now: f64, width: usize) -> Line<'static> {
    let Some(session) = folder.sessions.get(index) else {
        return Line::default();
    };
    // The closing branch only under the folder's very last session: more may scroll into view
    let branch = if index + 1 == folder.sessions.len() {
        "└ ".to_owned()
    } else if end {
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

/// A session found by the search: its title, its folder's name in grey, `(deleted)` when the
/// folder is gone, then how often the words occur and its age. The last on show, with more
/// found below, starts with the mark that more lie below.
///
/// # Arguments
/// * `session`: the session found.
/// * `more_below`: whether it is the last on show with more found below.
/// * `count`: how often the typed words occur in the session.
/// * `now`: current time as a Unix timestamp.
/// * `width`: columns available.
fn hit_line(
    session: &Searchable,
    more_below: bool,
    count: usize,
    now: f64,
    width: usize,
) -> Line<'static> {
    let lead = if more_below {
        format!("{MORE}  ")
    } else {
        "   ".to_owned()
    };
    let deleted = if session.gone { " (deleted)" } else { "" };
    let tail = format!(
        "{count}×  {}",
        ljust(&words::age(session.last, now), AGE_WIDTH)
    );
    // The lead, the two spaces after the title and one before the tail
    let room = width.saturating_sub(3 + 2 + 1 + width_of(&tail));
    let name = words::fit(&session.name, room / 3);
    let title = words::fit(
        &session.title,
        room.saturating_sub(width_of(&name) + width_of(deleted)),
    );
    let mut line = styled(&[(&lead, FAINT), (&title, INK), ("  ", INK)]);
    push(&mut line, format!("{name}{deleted}"), INACTIVE);
    let gap = width.saturating_sub(cell_len(&line) + width_of(&tail));
    push(&mut line, " ".repeat(gap), INK);
    push(&mut line, tail, SOFT);
    line
}

/// Cuts a line longer than `width` with a grey ellipsis.
fn cut(mut line: Line<'static>, width: usize) -> Line<'static> {
    if cell_len(&line) > width {
        truncate(&mut line, width.saturating_sub(1));
        push(&mut line, "…", FAINT);
    }
    line
}

/// An open entry to the disk: a grey arrow, a green `+` for a new folder, its name in bold and
/// its hint in grey.
fn root_line(root: Root, hint: &str, width: usize) -> Line<'static> {
    let mut line = styled(&[(" ▾ ", SOFT)]);
    let name = match root {
        Root::New => {
            push(&mut line, "+ ", GREEN);
            "New folder…"
        }
        Root::Open => "Open another folder…",
    };
    push(&mut line, name, STRONG);
    push(&mut line, "  ", INK);
    push(&mut line, hint, INACTIVE);
    cut(line, width)
}

/// A folder of an entry's tree, or a match of a search: its tree lines, then its name, a match's
/// parent folders in grey and the letters the search matched in Claude's orange.
///
/// # Arguments
/// * `branch`: the tree lines before the name.
/// * `label`: the folder's name, or a match's path below the folder searched.
/// * `hits`: the byte offsets in `label` of the characters the search matched.
/// * `width`: columns available.
fn dir_line(branch: &str, label: &str, hits: &[usize], width: usize) -> Line<'static> {
    let mut line = styled(&[("   ", INK), (branch, SUBTLE)]);
    // A match's label is its path below the folder searched: the name is after the last `/`
    let name_at = label.trim_end_matches('/').rfind('/').map_or(0, |i| i + 1);
    let room = width.saturating_sub(cell_len(&line));
    let fits = width_of(label) <= room;
    let mut used = 0;
    let mut spans: Vec<Span<'static>> = Vec::new();
    // Marks by byte offset, as the search gives them
    for (at, c) in label.char_indices() {
        if !fits && used + cells(c) > room.saturating_sub(1) {
            break;
        }
        used += cells(c);
        let style = if hits.contains(&at) {
            CLAUDE.add_modifier(Modifier::BOLD)
        } else if at < name_at || (c == '/' && at + 1 == label.len()) {
            INACTIVE
        } else {
            INK
        };
        match spans.last_mut() {
            Some(span) if span.style == style => span.content.to_mut().push(c),
            _ => spans.push(Span::styled(c.to_string(), style)),
        }
    }
    line.spans.extend(spans);
    if !fits {
        push(&mut line, "…", FAINT);
    }
    line
}

/// A folder to create, then start a session in, `label` its path from home.
fn create_line(label: &str, width: usize) -> Line<'static> {
    let text = format!("create {label} and start");
    cut(styled(&[("   + ", GREEN), (&text, INK)]), width)
}

/// The folder's index in the folders, for a folder row or one of its sessions.
fn folder_of(row: &Row) -> Option<usize> {
    match row {
        Row::Folder { index, .. } | Row::Session { folder: index, .. } => Some(*index),
        _ => None,
    }
}

/// Every row's lines, with a rule under the entries and between folders, a header over the
/// sessions found, and the row each line belongs to. With `nothing` set, a last line says
/// nothing matches.
///
/// # Arguments
/// * `shown`: the rows from [`Picker::shown`].
/// * `picker`: the list: its folders, open folders and the sessions searched.
/// * `found`: the sessions found, from [`Picker::found`].
/// * `nothing`: whether to say nothing matches.
/// * `now`: current time as a Unix timestamp.
/// * `width`: columns available.
fn row_lines(
    shown: &[Row],
    picker: &Picker,
    found: &[Hit],
    nothing: bool,
    now: f64,
    width: usize,
) -> (Vec<Line<'static>>, Vec<Option<usize>>) {
    let folders = &picker.folders;
    let mut lines = Vec::new();
    let mut owners = Vec::new();
    for (i, row) in shown.iter().enumerate() {
        let after_entries = i > 0 && shown[i - 1].root().is_some() && row.root().is_none();
        if !lines.is_empty() && (matches!(row, Row::Folder { .. }) || after_entries) {
            lines.push(rule(width));
            owners.push(None);
        }
        let hit = |r: &Row| matches!(r, Row::Hit { .. });
        if hit(row) && (i == 0 || !hit(&shown[i - 1])) {
            let count = format!("  {} found", found.len());
            lines.push(styled(&[("   Sessions", STRONG), (&count, INACTIVE)]));
            owners.push(None);
        }
        let drawn: Vec<Line<'static>> = match row {
            Row::Path { path } => {
                let open = words::fit(&format!("open {path}"), width.saturating_sub(5));
                vec![styled(&[("   + ", GREEN), (&open, INK)])]
            }
            Row::Root { root, hint } => vec![root_line(*root, hint, width)],
            Row::Dir {
                branch,
                label,
                hits,
                ..
            } => vec![dir_line(branch, label, hits, width)],
            Row::Create { label, .. } => vec![create_line(label, width)],
            Row::Session {
                folder, index, end, ..
            } => vec![folders.get(*folder).map_or_else(Line::default, |folder| {
                session_line(folder, *index, *end, now, width)
            })],
            Row::Hit {
                index, more_below, ..
            } => {
                let count = found
                    .iter()
                    .find(|h| h.index == *index)
                    .map_or(0, |h| h.count);
                let session = picker.sessions.get(*index);
                vec![session.map_or_else(Line::default, |s| {
                    hit_line(s, *more_below, count, now, width)
                })]
            }
            Row::Folder { index, .. } => match folders.get(*index) {
                Some(folder) => folder_lines(folder, &picker.expanded, now, width).into(),
                None => Vec::new(),
            },
        };
        owners.extend(std::iter::repeat_n(Some(i), drawn.len()));
        lines.extend(drawn);
    }
    if nothing {
        if !lines.is_empty() {
            lines.push(rule(width));
            owners.push(None);
        }
        lines.push(styled(&[("   nothing matches", FAINT)]));
        owners.push(None);
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

/// The rows around the selected one, in `room` lines at most, after a gutter column that holds
/// a lavender pointer on the selected row's first line; the selected row's text is lavender too.
///
/// # Arguments
/// * `picker`: the list: its folders, selected row, open folders and search.
/// * `shown`: the rows from [`Picker::shown`].
/// * `now`: current time as a Unix timestamp.
/// * `room`: lines available.
/// * `width`: columns available.
pub fn list_lines(
    picker: &Picker,
    shown: &[Row],
    now: f64,
    room: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let cursor = picker.cursor;
    let typed = !picker.query.trim().is_empty();
    // A match: a history row, or a folder or create row of a searched tree
    let found = shown.iter().any(|r| match r {
        Row::Root { .. } => false,
        Row::Dir { root, .. } => picker.searched(*root).is_some(),
        _ => true,
    });
    // The gutter takes a column of the width
    let (lines, owners) = row_lines(
        shown,
        picker,
        &picker.found(),
        typed && !found,
        now,
        width.saturating_sub(1),
    );
    let total = lines.len();
    let picked: Vec<usize> = owners
        .iter()
        .enumerate()
        .filter(|(_, owner)| **owner == Some(cursor))
        .map(|(n, _)| n)
        .collect();
    // An open folder brings its sessions on show along with it
    let folder = shown.get(cursor).and_then(folder_of);
    let block_end = owners
        .iter()
        .rposition(|owner| {
            owner
                .and_then(|i| folder_of(&shown[i]))
                .is_some_and(|f| Some(f) == folder)
        })
        .or(picked.last().copied());
    // Scrolls just enough to keep every line of the selected row, and of its folder's open
    // sessions where they fit, in view
    let top = match (picked.first(), block_end) {
        (Some(&first), Some(last)) => (last + 1)
            .saturating_sub(room)
            .min(total.saturating_sub(room))
            .min(first),
        _ => 0,
    };
    let end = total.min(top + room);
    let mut out = Vec::with_capacity(end.saturating_sub(top));
    for (n, mut line) in lines.into_iter().enumerate().take(end).skip(top) {
        // A session line's branch already marks its folder's own more below
        let sixth = || line.spans.iter().flat_map(|s| s.content.chars()).nth(5);
        if n + 1 == end && end < total && sixth() != Some(MORE) {
            // Its first column is always blank: room for the mark that more lies below
            drop_first(&mut line);
            line.spans.insert(0, Span::styled(MORE.to_string(), FAINT));
        }
        // Claude Code's menus: the text in lavender, no bar; grey parts stay grey
        if picked.contains(&n) {
            for span in &mut line.spans {
                let text = !span.content.trim().is_empty();
                if text && matches!(span.style.fg, None | Some(Color::White)) {
                    span.style.fg = PERMISSION.fg;
                }
            }
        }
        let gutter = if picked.first() == Some(&n) {
            Span::styled("❯", PERMISSION.add_modifier(Modifier::BOLD))
        } else {
            Span::raw(" ")
        };
        line.spans.insert(0, gutter);
        out.push(line);
    }
    out
}

/// What the search box shows while nothing is typed: with a tree open, it also offers to name a
/// new folder.
pub fn placeholder(picker: &Picker) -> &'static str {
    if picker.open.is_empty() {
        "Search folders…"
    } else {
        "Search folders, or type a new folder's name…"
    }
}

/// The search in a rounded box, as Claude Code's prompt: three lines of `width` columns.
///
/// # Arguments
/// * `query`: what the user typed; text too long for the box keeps its end.
/// * `placeholder`: what the box shows in grey while `query` is empty.
/// * `focused`: whether keys type into it, shown by a cursor.
/// * `width`: columns the box takes, its border included.
pub fn search_box(
    query: &str,
    placeholder: &str,
    focused: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    let cursor = if focused { "▌" } else { "" };
    let mut middle = styled(&[("│", SOFT), (" ❯ ", CLAUDE)]);
    // What the text may take: the borders, the prompt, the cursor and a space before the right
    // border take the rest
    let room = inner.saturating_sub(4 + width_of(cursor));
    if query.is_empty() {
        push(&mut middle, cursor, INK);
        push(&mut middle, words::fit(placeholder, room), INACTIVE);
    } else {
        let mut used = 0;
        let kept: Vec<char> = query
            .chars()
            .rev()
            .take_while(|&c| {
                used += cells(c);
                used <= room
            })
            .collect();
        push(&mut middle, kept.into_iter().rev().collect::<String>(), INK);
        push(&mut middle, cursor, INK);
    }
    let gap = (inner + 1).saturating_sub(cell_len(&middle));
    push(&mut middle, " ".repeat(gap), INK);
    push(&mut middle, "│", SOFT);
    let border = "─".repeat(inner);
    vec![
        styled(&[(&format!("╭{border}╮"), SOFT)]),
        middle,
        styled(&[(&format!("╰{border}╯"), SOFT)]),
    ]
}

/// The keys that open the entries wherever characters are typed into the search, New folder's
/// first.
pub const ENTRY_KEYS: [&str; 2] = ["Ctrl+n", "Ctrl+o"];

/// What the keys of New folder and Open another folder do, as `(key, what it does)`: open the
/// entry, or close it while it is open.
///
/// # Arguments
/// * `keys`: the keys of New folder and Open another folder, in that order.
/// * `picker`: what is open.
pub fn entry_hints(keys: [&'static str; 2], picker: &Picker) -> [(&'static str, &'static str); 2] {
    let does = |root, open, close| {
        if picker.open.contains(&root) {
            close
        } else {
            open
        }
    };
    [
        (keys[0], does(Root::New, "new folder", "close new folder")),
        (
            keys[1],
            does(Root::Open, "open folder", "close open folder"),
        ),
    ]
}

/// What the keys do on the selected row, as `(key, what it does)`, ending with Esc. ← in a
/// tree is offered only while nothing is typed, as it does nothing then.
///
/// # Arguments
/// * `row`: the selected row, if any.
/// * `picker`: what is open and typed, to tell whether → opens or ← closes.
pub fn hints(row: Option<&Row>, picker: &Picker) -> Vec<(&'static str, &'static str)> {
    let left = picker.query.is_empty();
    let mut keys = match row {
        Some(r) => match r {
            Row::Root { .. } => vec![(if left { "Enter/←" } else { "Enter" }, "close")],
            Row::Dir {
                root: Root::New, ..
            } if left => vec![("Enter", "pick as parent"), ("←", "back")],
            Row::Dir {
                root: Root::New, ..
            } => vec![("Enter", "pick as parent")],
            Row::Dir { root, path, .. } if picker.dirs.contains(&(*root, path.clone())) => {
                let mut keys = vec![("Enter", "start here")];
                keys.extend(left.then_some(("←", "close")));
                keys
            }
            Row::Dir { .. } => vec![("Enter", "start here"), ("→", "open")],
            Row::Create { .. } => vec![("Enter", "create & start")],
            Row::Folder { path, .. } if picker.expanded.contains(path) => {
                vec![("Enter", "new session"), ("←", "close")]
            }
            Row::Folder { .. } => vec![("Enter", "new session"), ("→", "sessions")],
            Row::Session { .. } | Row::Hit { .. } => vec![("Enter", "resume")],
            Row::Path { .. } => vec![("Enter", "start here")],
        },
        None => Vec::new(),
    };
    keys.push(("Esc", "cancel"));
    keys
}

/// Hints in Claude Code's form, `{key} to {does}` in grey joined by ` · `.
///
/// A line too long for `width` is cut with an ellipsis.
pub fn hints_line(keys: &[(&str, &str)], width: usize) -> Line<'static> {
    let mut line = Line::from(Span::raw(" "));
    for (i, (key, does)) in keys.iter().enumerate() {
        if i > 0 {
            push(&mut line, " · ", SUBTLE);
        }
        push(&mut line, format!("{key} to {does}"), INACTIVE);
    }
    cut(line, width)
}

/// The Alt+n panel as `height` lines of `width` columns: the search box, a blank line, the list
/// and what the keys do: on the selected row, Esc, then the entries' keys.
pub fn screen(picker: &mut Picker, now: f64, width: u16, height: u16) -> Vec<Line<'static>> {
    let shown = picker.shown();
    let height = usize::from(height);
    // A cell to spare on the right, as the landing page leaves
    let width = usize::from(width).saturating_sub(1);
    let mut lines = search_box(&picker.query, placeholder(picker), true, width);
    lines.push(Line::default());
    lines.extend(list_lines(
        picker,
        &shown,
        now,
        height.saturating_sub(6),
        width,
    ));
    while lines.len() < height.saturating_sub(1) {
        lines.push(Line::default());
    }
    // The entries' keys last: a line too long loses them first
    let mut keys = hints(shown.get(picker.cursor), picker);
    keys.extend(entry_hints(ENTRY_KEYS, picker));
    lines.push(hints_line(&keys, width));
    lines
        .into_iter()
        .map(|line| {
            if line.spans.iter().all(|s| s.content.is_empty()) {
                Line::raw(" ")
            } else {
                line
            }
        })
        .take(height)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ratatui::style::Modifier;
    use ratatui::text::Line;

    use crate::ui::line::{cell_len, cells};
    use crate::ui::picker::draw::{
        dir_line, entry_hints, hints, hints_line, hit_line, list_lines, placeholder, screen,
        search_box, session_line, ENTRY_KEYS,
    };
    use crate::ui::picker::tests::{
        creates, hit_id, home_picker, label, linkedin_picker, list, no_dirs, open_documents,
        picker, searchable, session_ids, sessions, set, type_text, NOW,
    };
    use crate::ui::picker::{Folder, Key, Picker, Root, Row, WIDTH};
    use crate::ui::words::{CLAUDE, GREEN, INACTIVE, INK, PERMISSION, SOFT, SUBTLE};

    fn plain(line: &Line) -> String {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        text.trim_end().to_owned()
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
        let Some(Row::Session {
            folder, index, end, ..
        }) = p.selected()
        else {
            panic!("not on a session");
        };
        let last = plain(&session_line(&p.folders[folder], index, end, NOW, WIDTH));
        assert!(last.starts_with("     └ task 6"), "{last}");
        // With nothing older left, the cursor moves on to the next folder
        p.key(Key::Down);
        assert_eq!(p.selected().unwrap().path(), "/w/web");
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
    fn screen_draws_two_lines_per_folder_and_the_open_sessions() {
        let mut p = picker(false);
        // Python's test draws rows for "" under the query "ap"; here the rows follow the
        // query, and "w" keeps both folders
        p.query = "w".into();
        p.expanded = set(&["/w/api"]);
        let lines = screen(&mut p, NOW, WIDTH as u16 + 1, 19);
        assert_eq!(lines.len(), 19);
        let text: Vec<String> = lines.iter().map(plain).collect();
        for t in &text {
            assert!(t.chars().map(cells).sum::<usize>() <= WIDTH, "{t}");
        }
        // The search box, then a blank line, then the list in a gutter for the pointer
        assert!(text[0].starts_with('╭') && text[2].starts_with('╰'));
        assert!(text[1].starts_with("│ ❯ w▌"), "{}", text[1]);
        assert_eq!(text[3], "");
        // The closed entries are not listed: the history comes first
        assert!(text[4].starts_with("❯ ▾ api") && text[4].ends_with("1m ago"));
        assert_eq!(text[5], "    41 sessions · $12.50 7d");
        assert!(text[6].starts_with("      ├ task 0") && text[6].ends_with("$1.50"));
        assert!(text[8].starts_with("      ├ task 2") && text[8].contains("2h"));
        // More sessions scroll into view below: a mark, no closing branch yet
        assert!(text[10].starts_with("      ▼ task 4"), "{}", text[10]);
        assert_eq!(text[11].trim(), "─".repeat(WIDTH - 2));
        // No spend this week: no spend part
        assert!(text[12].starts_with("  ▸ web"));
        assert_eq!(text[13], "    1 session");
        // The hints of the open folder, cut at this narrow width
        assert_eq!(text[18], " Enter to new session · ← to close · Esc to can…");
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
        let shown = list(&many, 0, &none).history_rows();
        let lines: Vec<String> = list_lines(&list(&many, 25, &none), &shown, NOW, 8, WIDTH)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(lines.len(), 8);
        assert!(lines.iter().any(|x| x.contains(" f25")));
        assert_eq!(lines[7], " ▼  1 session");
        assert!(!lines.iter().any(|x| x.contains(" f0")));
        // At the bottom of the list nothing more lies below
        let end = list_lines(&list(&many, 29, &none), &shown, NOW, 8, WIDTH);
        assert_eq!(plain(&end[7]), "    1 session");
    }

    #[test]
    fn opening_a_folder_low_in_the_list_brings_its_sessions_into_view() {
        let mut many: Vec<Folder> = (0..8)
            .map(|i| Folder {
                path: format!("/w/f{i}"),
                name: format!("f{i}"),
                last: NOW - f64::from(i),
                count: 1,
                spend: 0.0,
                sessions: Vec::new(),
            })
            .collect();
        many[5].sessions = sessions();
        let open = set(&["/w/f5"]);
        let shown = list(&many, 0, &open).history_rows();
        let lines: Vec<String> = list_lines(&list(&many, 5, &open), &shown, NOW, 16, WIDTH)
            .iter()
            .map(plain)
            .collect();
        assert!(lines.iter().any(|x| x.starts_with("❯ ▾ f5")), "{lines:?}");
        let tasks: Vec<_> = lines.iter().filter(|x| x.contains("task ")).collect();
        assert_eq!(tasks.len(), 5, "{lines:?}");
    }

    #[test]
    fn nothing_matching_says_so() {
        let mut p = list(&[], 0, &set(&[]));
        p.query = "zz".into();
        let lines = list_lines(&p, &[], NOW, 8, WIDTH);
        assert_eq!(plain(&lines[0]), "    nothing matches");
    }

    #[test]
    fn nothing_matches_shows_only_once_something_is_typed() {
        let mut p = Picker::new(Box::new(no_dirs));
        let says = |p: &mut Picker| {
            screen(p, NOW, WIDTH as u16 + 1, 19)
                .iter()
                .any(|l| plain(l).contains("nothing matches"))
        };
        assert!(!says(&mut p));
        p.query = " ".into();
        assert!(!says(&mut p));
        p.query = "zz".into();
        assert!(says(&mut p));
    }

    /// Whether the panel says nothing matches.
    fn says_nothing_matches(p: &mut Picker) -> bool {
        screen(p, NOW, WIDTH as u16 + 1, 40)
            .iter()
            .any(|l| plain(l).contains("nothing matches"))
    }

    #[test]
    fn nothing_matches_only_when_no_row_at_all_matches() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::Open));
        // A folder on disk matches, no history folder does
        type_text(&mut p, "Music");
        assert!(!says_nothing_matches(&mut p));
        // Neither a folder on disk nor a history folder, and no create row
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::Open));
        type_text(&mut p, "~zz");
        assert!(!creates(&mut p, Root::Open));
        assert!(says_nothing_matches(&mut p));
        // A tree open but not searched: its folders match nothing
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::Open));
        p.key(Key::Down);
        p.key(Key::Down);
        p.key(Key::Down);
        p.key(Key::Down);
        assert!(matches!(p.selected(), Some(Row::Folder { .. })));
        type_text(&mut p, "zz");
        assert_eq!(p.anchor, None);
        assert!(says_nothing_matches(&mut p));
    }

    #[test]
    fn the_selected_row_gets_a_lavender_pointer_not_a_bar() {
        let mut p = picker(false);
        let shown = p.shown();
        let lines = list_lines(&p, &shown, NOW, 20, WIDTH);
        let at = lines.iter().position(|l| plain(l).contains("api")).unwrap();
        let first = &lines[at].spans[0];
        assert_eq!(first.content, "❯");
        assert_eq!(first.style, PERMISSION.add_modifier(Modifier::BOLD));
        // The name takes the pointer's colour; the other rows keep an empty gutter
        let name = lines[at].spans.iter().find(|s| s.content == "api").unwrap();
        assert_eq!(name.style.fg, PERMISSION.fg);
        assert!(lines
            .iter()
            .enumerate()
            .all(|(n, l)| n == at || plain(l).starts_with(' ')));
        let bars = lines
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.style.bg.is_some());
        assert!(!bars, "{lines:?}");
        assert!(lines.iter().all(|l| cell_len(l) <= WIDTH));
    }

    #[test]
    fn an_open_entry_shows_its_hint() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::Open));
        p.key(Key::Entry(Root::New));
        p.key(Key::Down);
        p.key(Key::Down);
        p.key(Key::Down);
        p.key(Key::Down);
        let shown = p.shown();
        let lines = list_lines(&p, &shown, NOW, 20, 80);
        assert_eq!(
            plain(&lines[0]),
            "  ▾ + New folder…  pick the parent, then type the name"
        );
        // Below New folder's tree, with no rule between the entries
        assert_eq!(
            plain(&lines[4]),
            "❯ ▾ Open another folder…  type to search below"
        );
        let hint = lines[4].spans.last().unwrap();
        assert_eq!(
            (hint.content.as_ref(), hint.style),
            ("type to search below", INACTIVE)
        );
        let plus = lines[0].spans.iter().find(|s| s.content == "+ ").unwrap();
        assert_eq!(plus.style, GREEN);
    }

    #[test]
    fn a_match_greys_its_parent_and_marks_its_letters() {
        let line = dir_line("├ ", "lta/vtp-sdk", &[8, 9, 10], WIDTH);
        assert_eq!(plain(&line), "   ├ lta/vtp-sdk");
        let style_of = |text: &str| {
            let span = line.spans.iter().find(|s| s.content == text);
            span.unwrap_or_else(|| panic!("{text} in {line:?}")).style
        };
        assert_eq!(style_of("├ "), SUBTLE);
        assert_eq!(style_of("lta/"), INACTIVE);
        assert_eq!(style_of("vtp-"), INK);
        assert_eq!(style_of("sdk"), CLAUDE.add_modifier(Modifier::BOLD));
    }

    #[test]
    fn a_long_match_is_cut_and_keeps_its_marks() {
        let line = dir_line("└ ", &format!("{}/ábc", "x".repeat(60)), &[61], 20);
        assert_eq!(cell_len(&line), 20);
        assert!(plain(&line).ends_with('…'), "{line:?}");
        // A mark on a character past the cut is dropped, never misplaced
        assert!(line
            .spans
            .iter()
            .all(|s| s.style != CLAUDE.add_modifier(Modifier::BOLD)));
    }

    #[test]
    fn a_create_row_names_the_folder_from_home() {
        let (_t, mut p, _) = home_picker();
        open_documents(&mut p, Root::New);
        type_text(&mut p, "Meu-Projeto");
        let shown = p.shown();
        let lines = list_lines(&p, &shown, NOW, 20, WIDTH);
        let create = lines
            .iter()
            .map(plain)
            .find(|l| l.contains("create"))
            .unwrap();
        assert_eq!(create, "❯   + create ~/Documents/Meu-Projeto and start");
    }

    #[test]
    fn the_search_box_is_rounded_and_holds_the_placeholder() {
        let lines = search_box("", "Search folders…", true, WIDTH);
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(text.len(), 3);
        assert!(
            text[0].starts_with('╭') && text[0].ends_with('╮'),
            "{}",
            text[0]
        );
        assert!(text[1].starts_with("│ ❯ ▌Search folders…"), "{}", text[1]);
        assert!(text[1].ends_with('│'));
        assert!(
            text[2].starts_with('╰') && text[2].ends_with('╯'),
            "{}",
            text[2]
        );
        assert!(lines.iter().all(|l| cell_len(l) == WIDTH));
        assert_eq!(lines[0].spans[0].style, SOFT);
        assert_eq!(lines[1].spans[1].style, CLAUDE);
        // Not focused: no cursor; typed text replaces the placeholder
        assert!(plain(&search_box("", "x", false, WIDTH)[1]).starts_with("│ ❯ x "));
        assert!(plain(&search_box("ap", "x", true, WIDTH)[1]).starts_with("│ ❯ ap▌ "));
        // Text too long for the box keeps its end, where the cursor is
        let long = search_box(&"y".repeat(80), "x", true, 20);
        assert!(long.iter().all(|l| cell_len(l) == 20));
        assert!(plain(&long[1]).ends_with("yy▌ │"), "{}", plain(&long[1]));
    }

    #[test]
    fn the_placeholder_offers_a_name_once_a_tree_is_open() {
        let (_t, mut p, _) = home_picker();
        assert_eq!(placeholder(&p), "Search folders…");
        p.key(Key::Entry(Root::New));
        assert_eq!(
            placeholder(&p),
            "Search folders, or type a new folder's name…"
        );
    }

    /// The hints of the selected row, each as `"{key} to {does}"`.
    fn hint_texts(p: &mut Picker) -> Vec<String> {
        let row = p.selected();
        hints(row.as_ref(), p)
            .iter()
            .map(|(key, does)| format!("{key} to {does}"))
            .collect()
    }

    #[test]
    fn hints_follow_the_selected_row() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::Open));
        p.key(Key::Down);
        assert_eq!(
            hint_texts(&mut p),
            ["Enter to start here", "→ to open", "Esc to cancel"]
        );
        let joined = |p: &mut Picker| plain(&hints_line(&hints(p.selected().as_ref(), p), 80));
        assert_eq!(
            joined(&mut p),
            " Enter to start here · → to open · Esc to cancel"
        );
        p.key(Key::Up);
        assert_eq!(joined(&mut p), " Enter/← to close · Esc to cancel");
        p.key(Key::Down);
        type_text(&mut p, "zz-new");
        assert!(matches!(p.selected(), Some(Row::Create { .. })));
        assert!(joined(&mut p).contains("Enter to create & start"));
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::New));
        p.key(Key::Down);
        assert!(joined(&mut p).contains("Enter to pick as parent · ← to back"));
    }

    /// The panel's last line, `width` columns wide.
    fn panel_footer(p: &mut Picker, width: u16) -> String {
        plain(screen(p, NOW, width, 20).last().unwrap())
    }

    #[test]
    fn the_panel_footer_ends_with_esc_then_the_entry_keys() {
        // As wide as the Alt+n popup: a folder's keys and Esc whole, the entries' cut
        let mut p = picker(false);
        assert_eq!(
            panel_footer(&mut p, 75),
            " Enter to new session · → to sessions · Esc to cancel · Ctrl+n to new fol…"
        );
        assert_eq!(
            panel_footer(&mut p, 120),
            " Enter to new session · → to sessions · Esc to cancel · Ctrl+n to new folder · \
             Ctrl+o to open folder"
        );
        // On an entry and in its tree too
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::New));
        assert_eq!(
            panel_footer(&mut p, 120),
            " Enter/← to close · Esc to cancel · Ctrl+n to close new folder · Ctrl+o to open \
             folder"
        );
        p.key(Key::Down);
        assert!(panel_footer(&mut p, 120).ends_with(
            "← to back · Esc to cancel · Ctrl+n to close new folder · Ctrl+o to open folder"
        ));
    }

    #[test]
    fn an_open_entrys_key_says_it_closes_it() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::New));
        assert_eq!(
            entry_hints(ENTRY_KEYS, &p),
            [("Ctrl+n", "close new folder"), ("Ctrl+o", "open folder")]
        );
        // On a history row too
        p.key(Key::Entry(Root::Open));
        p.cursor = p.shown().iter().position(|r| r.path() == "/w/api").unwrap();
        assert!(panel_footer(&mut p, 160)
            .ends_with("Esc to cancel · Ctrl+n to close new folder · Ctrl+o to close open folder"));
        p.key(Key::Entry(Root::New));
        assert_eq!(
            entry_hints(["n", "o"], &p),
            [("n", "new folder"), ("o", "close open folder")]
        );
    }

    #[test]
    fn left_is_hinted_only_while_nothing_is_typed() {
        let (_t, mut p, _) = home_picker();
        p.key(Key::Entry(Root::New));
        type_text(&mut p, "Doc");
        assert_eq!(label(&p.selected().unwrap()), "Documents");
        assert_eq!(
            hint_texts(&mut p),
            ["Enter to pick as parent", "Esc to cancel"]
        );
        p.key(Key::Up);
        p.key(Key::Up);
        assert!(matches!(p.selected(), Some(Row::Root { .. })));
        assert_eq!(hint_texts(&mut p), ["Enter to close", "Esc to cancel"]);
        // An open folder of the tree, the text filtering the history only
        let (_t, mut p, _) = home_picker();
        for key in [Key::Entry(Root::New), Key::Down, Key::Right] {
            p.key(key);
        }
        p.query = "x".into();
        assert_eq!(
            hint_texts(&mut p),
            ["Enter to pick as parent", "Esc to cancel"]
        );
        p.query.clear();
        assert_eq!(
            hint_texts(&mut p),
            ["Enter to pick as parent", "← to back", "Esc to cancel"]
        );
    }

    #[test]
    fn hints_line_is_grey_and_cut_with_an_ellipsis() {
        let keys = [("Enter", "start here"), ("Esc", "cancel")];
        let line = hints_line(&keys, 80);
        assert_eq!(plain(&line), " Enter to start here · Esc to cancel");
        let style_of = |text: &str| line.spans.iter().find(|s| s.content == text).unwrap().style;
        assert_eq!(style_of("Enter to start here"), INACTIVE);
        assert_eq!(style_of(" · "), SUBTLE);
        let cut = hints_line(&keys, 12);
        assert_eq!(plain(&cut), " Enter to s…");
    }

    #[test]
    fn typed_text_lists_the_sessions_found_under_a_header() {
        let mut p = linkedin_picker();
        type_text(&mut p, "linkedin");
        let shown = p.shown();
        let text: Vec<String> = list_lines(&p, &shown, NOW, 40, WIDTH)
            .iter()
            .map(plain)
            .collect();
        let header = text.iter().position(|t| t.contains("Sessions  2 found"));
        let header = header.unwrap_or_else(|| panic!("{text:?}"));
        // The title holding the word first, though older
        assert!(text[header + 1].contains("Post on LinkedIn"), "{text:?}");
        assert!(text[header + 2].contains("Banner options"), "{text:?}");
        for line in &text[header + 1..=header + 2] {
            assert!(line.contains("  cv") && line.contains("1×"), "{line}");
        }
        assert_eq!(p.selected().and_then(hit_id).as_deref(), Some("a"));
    }

    #[test]
    fn a_deleted_folder_reads_deleted() {
        let mut session = searchable("a", "Banner", "/w/cv", 60.0, &[]);
        session.gone = true;
        let line = plain(&hit_line(&session, false, 2, NOW, WIDTH));
        assert!(line.contains("cv (deleted)"), "{line}");
    }

    #[test]
    fn a_hit_line_fits_the_narrow_panel() {
        let title = "A very long title about posting on LinkedIn in English, with more words";
        let path = "/w/a-folder-with-a-really-long-name-indeed";
        let session = searchable("a", title, path, 7200.0, &[]);
        let line = hit_line(&session, false, 12, NOW, WIDTH);
        assert!(cell_len(&line) <= WIDTH, "{}", plain(&line));
        let text = plain(&line);
        assert!(text.ends_with("12×  2h"), "{text}");
        assert!(text.starts_with("   A very"), "{text}");
        assert!(text.contains("  a-folder"), "{text}");
    }
}
