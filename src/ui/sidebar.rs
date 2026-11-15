//! The sidebar column: mascot, sessions by urgency, background runs, and the statistics.
//!
//! It reads like Claude Code's own transcript: each session is a line with its status's mark,
//! and a `⎿` line under it. The statistics at the bottom are [`crate::ui::stats`]'s blocks.

use std::collections::HashMap;

use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use serde::Deserialize;

use crate::core::state::{self, Record, Status, TIMED};
use crate::term::tmux::{self, Pane};
use crate::ui::line::{cells, finish, push, rjust, start, truncate, width_of, BULLET, ELBOW_CELLS};
use crate::ui::stats::{stats_lines, Stats};
use crate::ui::words::{
    self, elbow, right, Bullet, CLAUDE, CURRENT, ERROR, INACTIVE, INK, MAGENTA, MASCOT,
    MASCOT_ROWS, PERMISSION, STRONG, SUCCESS, WARNING,
};

/// The sidebar pane's width.
pub const WIDTH: usize = 29;
/// The room inside the one-cell padding on each side.
pub(crate) const CONTENT: usize = 27;

/// The room a `⎿` line leaves after the elbow.
const DETAIL_ROOM: usize = CONTENT - ELBOW_CELLS;
/// A blank line, the mascot (3) and a blank line: the mascot level with Claude Code's own.
const HEAD: i64 = 5;
/// Lines of a session entry whose name fits one line, not counting the blank line between
/// entries; a longer name adds one (see [`Laid::height`]).
const ENTRY: usize = 2;
/// The cells kept for the waiting time beside a name: `59m` or `12h`.
const LABEL_CELLS: usize = 3;
/// The cells the cost takes on a `⎿` line, right-aligned: `$123.45`.
const COST_CELLS: usize = 7;
/// The cells Claude's time takes on a `⎿` line, right-aligned: ` 12h05`.
const TIME_CELLS: usize = 6;
/// Claude Code's spinner, played forwards and back, one frame per redraw.
const SPINNER: [char; 10] = ['·', '✢', '✳', '✶', '✻', '✽', '✻', '✶', '✳', '✢'];

/// One session window as the sidebar lists it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// The tmux window's id.
    pub window_id: String,
    /// The window's name.
    pub name: String,
    /// The status of the window's most urgent session.
    pub status: Status,
    /// When that session entered its status.
    pub since: f64,
    /// Dollars spent so far.
    pub cost: Option<f64>,
    /// How long Claude has spent answering, in seconds.
    pub duration: Option<f64>,
    /// The model's display name.
    pub model: Option<String>,
}

/// A Claude run without a terminal (a start hook's `claude -p`), listed while it runs.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Run {
    /// What the run is called.
    pub name: String,
    /// When it started, as a Unix timestamp.
    pub started: f64,
}

/// The session windows of a tmux server, in window order, from the live records.
///
/// A window with no live session (the landing page) is left out. The most urgent session of a
/// window sets its status; the session in its first Claude pane gives the cost and the model.
///
/// # Arguments
/// * `records`: the live records, most urgent first.
/// * `panes`: the server's panes.
/// * `socket`: the server's socket; records on other servers are left out.
pub fn load_entries(records: &[Record], panes: &[Pane], socket: &str) -> Vec<Entry> {
    let windows = tmux::windows_of(records, panes, socket);
    let heads: HashMap<&str, &tmux::Window> =
        windows.iter().map(|w| (w.window_id.as_str(), w)).collect();
    tmux::window_entries(panes)
        .into_iter()
        .filter_map(|(window_id, _, name)| {
            let window = heads.get(window_id.as_str())?;
            let usage = window.head().usage.as_ref();
            Some(Entry {
                status: window.lead.status,
                since: state::since(&window.lead),
                cost: usage.and_then(|u| u.cost),
                duration: usage.and_then(|u| u.api_duration),
                model: usage.and_then(|u| u.model.clone()),
                window_id,
                name,
            })
        })
        .collect()
}

/// The entries in the order the sidebar lists them: by urgency, then by window.
fn ordered(entries: &[Entry]) -> Vec<&Entry> {
    let mut out: Vec<&Entry> = entries.iter().collect();
    // A stable sort keeps the window order within each place
    out.sort_by_key(|e| state::urgency(e.status));
    out
}

/// The window `offset` places from `current` in the order the sidebar lists them.
///
/// Wraps around at either end; from a window it does not list (the landing page), down goes to
/// the first and up to the last. `None` when it lists nothing.
pub fn step(entries: &[Entry], current: &str, offset: i32) -> Option<String> {
    let order = ordered(entries);
    let index = match order.iter().position(|e| e.window_id == current) {
        Some(at) => (at as i64 + i64::from(offset)).rem_euclid(order.len() as i64) as usize,
        None if offset > 0 => 0,
        None => order.len().checked_sub(1)?,
    };
    order.get(index).map(|e| e.window_id.clone())
}

/// The spinner's frame at a moment; the frame number truncates, as Python's `int()` does.
fn spinner(now: f64) -> char {
    let frame = (now * f64::from(words::FPS)) as i64;
    SPINNER[frame.rem_euclid(SPINNER.len() as i64) as usize]
}

/// The colour of a status, its mark and its waiting time; `None` for an idle one.
fn colour(status: Status) -> Option<Style> {
    match status {
        Status::Permission => Some(ERROR),
        Status::Input => Some(WARNING),
        Status::Done => Some(SUCCESS),
        Status::Working => Some(CLAUDE),
        Status::Shell => Some(PERMISSION),
        Status::Idle | Status::Unknown => None,
    }
}

/// The mark of a status at `now`: its shape tells the status apart without the colour, and a
/// working session's spins.
fn mark(status: Status, now: f64) -> char {
    match status {
        Status::Permission => '!',
        Status::Input => '?',
        Status::Shell => '◐',
        Status::Done => '✓',
        Status::Working => spinner(now),
        Status::Idle | Status::Unknown => '○',
    }
}

/// The waiting time shown right of an entry's name; empty for untimed statuses.
///
/// From 100 hours on it counts days (`4d`), so it never outgrows the [`LABEL_CELLS`] kept for it.
fn waiting(entry: &Entry, now: f64) -> String {
    if !TIMED.contains(&entry.status) {
        return String::new();
    }
    let waited = now - entry.since;
    let label = words::waiting_label(waited);
    if width_of(&label) <= LABEL_CELLS {
        label
    } else {
        format!("{}d", words::floor_div(waited, 86400.0) as i64)
    }
}

/// An entry's name split into its first line, beside the mark and the waiting time, and the
/// rest for the line under it; the rest is empty when the name fits.
///
/// A status that shows a waiting time always keeps [`LABEL_CELLS`] for it, so the entry's
/// height does not change as the time appears and grows. The split falls on the last space
/// that fits, unless that leaves the first line less than half full; then the name is cut
/// inside a word.
fn split_name(entry: &Entry) -> (String, String) {
    let taken = if TIMED.contains(&entry.status) {
        LABEL_CELLS + 1
    } else {
        0
    };
    let room = CONTENT.saturating_sub(2 + taken);
    let name = entry.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if width_of(&name) <= room {
        return (name, String::new());
    }
    let mut used = 0;
    let mut cut = 0;
    let mut space = None;
    for (at, c) in name.char_indices() {
        if used + cells(c) > room {
            // A word that ends right at the edge still fits
            if c == ' ' {
                space = Some(at);
            }
            break;
        }
        if c == ' ' {
            space = Some(at);
        }
        used += cells(c);
        cut = at + c.len_utf8();
    }
    // A space early in the line would leave it mostly empty: cut the word there instead
    let at = space
        .filter(|&at| 2 * width_of(&name[..at]) >= room)
        .unwrap_or(cut);
    (name[..at].to_owned(), name[at..].trim_start().to_owned())
}

/// A `⎿` field, or `–` for an unknown one.
fn or_dash(text: &str) -> &str {
    if text.is_empty() {
        "–"
    } else {
        text
    }
}

/// The lines of a session entry, [`WIDTH`] cells each: two, or three for a long name.
///
/// The first has the status's mark (the spinner while Claude works), the name and the waiting
/// time; a name too long for it goes on under it, cut there with `…`. The `⎿` line has the
/// cost, Claude's time and the model's family in columns, so they line up across entries.
/// The current session's lines have a lighter background.
fn entry_lines(laid: &Laid, current: bool, now: f64) -> Vec<Line<'static>> {
    let Laid { entry, head, rest } = laid;
    let base = if current { CURRENT } else { Style::new() };
    let status_colour = colour(entry.status);
    let mark_style = status_colour.unwrap_or(INACTIVE);
    let mark = mark(entry.status, now);
    let name_style = match (current, status_colour) {
        (true, _) => STRONG,
        (false, None) => INACTIVE,
        (false, Some(_)) => INK,
    };
    let label = waiting(entry, now);
    let mut lines = vec![Bullet {
        mark,
        mark_style: mark_style.add_modifier(Modifier::BOLD),
        title: head,
        title_style: name_style,
        value: &label,
        value_style: mark_style.add_modifier(Modifier::BOLD),
        base,
        content: CONTENT,
    }
    .line()];
    if !rest.is_empty() {
        let mut more = start(base);
        push(&mut more, "  ", INK);
        push(&mut more, words::fit(rest, CONTENT - 2), name_style);
        lines.push(more);
    }

    let cost = rjust(or_dash(&words::money(entry.cost)), COST_CELLS);
    let time = rjust(or_dash(&words::span(entry.duration)), TIME_CELLS);
    // A cost or time wider than its column leaves the model less room
    let room = DETAIL_ROOM.saturating_sub(width_of(&cost) + width_of(&time) + 2);
    // Only the family ("Opus" of "Opus 5.5"), and only whole: a cut name says nothing
    let model = entry
        .model
        .as_deref()
        .and_then(|m| m.split_whitespace().next())
        .filter(|m| width_of(m) <= room)
        .unwrap_or_default();
    let mut detail = elbow(true, base);
    push(&mut detail, cost, INACTIVE);
    push(&mut detail, time, INACTIVE);
    push(&mut detail, format!("  {}", or_dash(model)), INACTIVE);
    truncate(&mut detail, 1 + CONTENT);
    lines.push(detail);
    lines.into_iter().map(|l| finish(l, CONTENT)).collect()
}

/// A blank line, the `Background` bullet and one line per run, in `room` lines at most.
///
/// Runs that do not fit are left out; with no room for a run there is no section at all.
fn runs_lines(runs: &[Run], now: f64, room: i64) -> Vec<Line<'static>> {
    let fits = usize::try_from(room - 2).unwrap_or(0);
    let shown = &runs[..runs.len().min(fits)];
    if shown.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        start(Style::new()),
        Bullet {
            mark: BULLET,
            mark_style: MAGENTA,
            title: "Background",
            title_style: STRONG,
            value: "",
            value_style: Style::new(),
            base: Style::new(),
            content: CONTENT,
        }
        .line(),
    ];
    for (i, run) in shown.iter().enumerate() {
        let label = words::waiting_label(now - run.started);
        let mut line = elbow(i == 0, Style::new());
        let taken = if label.is_empty() {
            0
        } else {
            width_of(&label) + 1
        };
        push(
            &mut line,
            words::fit(&run.name, DETAIL_ROOM.saturating_sub(taken)),
            INACTIVE,
        );
        if !label.is_empty() {
            right(&mut line, &label, MAGENTA, CONTENT);
        }
        lines.push(line);
    }
    lines.into_iter().map(|l| finish(l, CONTENT)).collect()
}

/// A session entry with its name split for its lines (see [`split_name`]), so a frame splits
/// each name once.
struct Laid<'a> {
    entry: &'a Entry,
    head: String,
    rest: String,
}

impl<'a> Laid<'a> {
    fn new(entry: &'a Entry) -> Self {
        let (head, rest) = split_name(entry);
        Self { entry, head, rest }
    }

    /// The entry's lines: two, or three when its name goes on to a second line.
    fn height(&self) -> usize {
        ENTRY + usize::from(!self.rest.is_empty())
    }
}

/// The entries laid out in the order the sidebar lists them.
fn lay(entries: &[Entry]) -> Vec<Laid<'_>> {
    ordered(entries).into_iter().map(Laid::new).collect()
}

/// How many entries fit in `room` lines, a blank line between two.
///
/// An entry that does not fit, and all after it, are left out.
fn fitting(laid: &[Laid], room: i64) -> usize {
    let mut used = 0;
    let mut shown = 0;
    for entry in laid {
        let next = used + usize::from(shown > 0) + entry.height();
        if next as i64 > room {
            break;
        }
        used = next;
        shown += 1;
    }
    shown
}

/// The statistics that fit under the sessions: Today goes when the sessions need its room.
fn bottom_lines(laid: &[Laid], stats: &Stats, now: f64, height: i64) -> Vec<Line<'static>> {
    let full = stats_lines(stats, now, CONTENT, true);
    // Without a Today block there is nothing to leave out
    if stats.day.is_none()
        || fitting(laid, height - HEAD - full.len() as i64) == fitting(laid, height)
    {
        return full;
    }
    stats_lines(stats, now, CONTENT, false)
}

/// The entry drawn on row `y` of a `height`-line column, or `None`.
///
/// The blank lines between entries, and entries a short column leaves out, belong to none.
pub fn entry_at<'a>(entries: &'a [Entry], y: u16, height: u16, stats: &Stats) -> Option<&'a Entry> {
    let height = i64::from(height);
    let laid = lay(entries);
    let bottom = bottom_lines(&laid, stats, 0.0, height).len() as i64;
    let shown = fitting(&laid, height - HEAD - bottom);
    let mut row = usize::try_from(i64::from(y) - HEAD).ok()?;
    for entry in &laid[..shown] {
        if row < entry.height() {
            return Some(entry.entry);
        }
        // Past this entry and the blank line after it
        row = row.checked_sub(entry.height() + 1)?;
    }
    None
}

/// The whole column: exactly `height` lines of [`WIDTH`] cells.
///
/// Sessions come first, most urgent first, then background runs; the statistics sit at the
/// bottom. A terminal shorter than the statistics keeps the bottom of them.
pub fn column(
    entries: &[Entry],
    current: &str,
    stats: &Stats,
    now: f64,
    height: u16,
    runs: &[Run],
) -> Vec<Line<'static>> {
    let blank = || finish(start(Style::new()), CONTENT);
    let pad = " ".repeat((CONTENT - width_of(MASCOT_ROWS[0])) / 2);
    let mut lines = vec![blank()];
    for row in MASCOT_ROWS {
        let mut line = start(Style::new());
        push(&mut line, format!("{pad}{row}"), MASCOT);
        lines.push(finish(line, CONTENT));
    }
    lines.push(blank());

    let height = i64::from(height);
    let laid = lay(entries);
    let bottom = bottom_lines(&laid, stats, now, height);
    let room = height - HEAD - bottom.len() as i64;
    let mut body = Vec::new();
    for (i, entry) in laid[..fitting(&laid, room)].iter().enumerate() {
        if i > 0 {
            body.push(blank());
        }
        body.extend(entry_lines(entry, entry.entry.window_id == current, now));
    }
    body.extend(runs_lines(runs, now, room - body.len() as i64));
    let filler = usize::try_from(room - body.len() as i64).unwrap_or(0);
    lines.extend(body);
    lines.extend(std::iter::repeat_with(blank).take(filler));
    lines.extend(bottom);
    let cut = lines.len().saturating_sub(height as usize);
    lines.split_off(cut)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    use crate::core::state::{Mux, Record, Status, Usage};
    use crate::term::tmux::Pane;
    use crate::ui::line::{cell_len, plain, row_text};
    use crate::ui::sidebar::{
        self, column, entry_at, fitting, lay, load_entries, ordered, runs_lines, spinner, step,
        Entry, Laid, Run, DETAIL_ROOM, WIDTH,
    };
    use crate::ui::stats::tests::{body, day, limits, stats, strs, week, NOW};
    use crate::ui::stats::Stats;
    use crate::ui::words::{
        CURRENT, ERROR, INACTIVE, MAGENTA, PERMISSION, STRONG, SUCCESS, WARNING,
    };

    const ELBOW: &str = "  ⎿  ";

    fn entry_lines(entry: &Entry, current: bool, now: f64) -> Vec<Line<'static>> {
        sidebar::entry_lines(&Laid::new(entry), current, now)
    }

    fn e(id: &str, name: &str, status: Status, since: f64) -> Entry {
        Entry {
            window_id: id.into(),
            name: name.into(),
            status,
            since,
            ..Entry::default()
        }
    }

    fn eu(id: &str, name: &str, status: Status, since: f64, cost: f64, dur: f64) -> Entry {
        Entry {
            cost: Some(cost),
            duration: Some(dur),
            ..e(id, name, status, since)
        }
    }

    fn entries() -> Vec<Entry> {
        vec![
            eu("@1", "web", Status::Permission, NOW - 180.0, 3.10, 2400.0),
            eu(
                "@2",
                "vehicle.k265",
                Status::Working,
                NOW - 5.0,
                0.90,
                4320.0,
            ),
            eu("@3", "api", Status::Shell, NOW - 360.0, 5.42, 7500.0),
            eu("@4", "docs", Status::Done, NOW - 9.0, 0.12, 900.0),
            eu("@5", "notes", Status::Idle, NOW - 9.0, 0.05, 10800.0),
        ]
    }

    /// A `⎿` line: the cost and Claude's time right-aligned, then the model.
    fn detail(cost: &str, time: &str, model: &str) -> String {
        format!("{ELBOW}{cost:>7}{time:>6}  {model}")
    }

    fn entry(status: Status, waited: f64) -> Entry {
        eu("@1", "web", status, NOW - waited, 3.10, 2400.0)
    }

    #[test]
    fn column_matches_the_spec_text() {
        let stats = Stats {
            day: Some(day(1376, 58, 4, 720.0)),
            ..stats()
        };
        let lines = column(&entries(), "@3", &stats, NOW, 50, &[]);
        assert_eq!(lines.len(), 50);
        assert!(lines.iter().all(|l| cell_len(l) == WIDTH));
        let spin = spinner(NOW).to_string();
        let pad = " ".repeat(9);
        let b = body(&lines);
        assert_eq!(
            b[..20],
            strs(&[
                "",
                &format!("{pad} ▐▛███▜▌"),
                &format!("{pad}▝▜█████▛▘"),
                &format!("{pad}  ▘▘ ▝▝"),
                "",
                "! web                    3m",
                &detail("$3.10", "40m", "–"),
                "",
                &format!("{spin} vehicle.k265"),
                &detail("$0.90", "1h12", "–"),
                "",
                "◐ api                    6m",
                &detail("$5.42", "2h05", "–"),
                "",
                "✓ docs",
                &detail("$0.12", "15m", "–"),
                "",
                "○ notes",
                &detail("$0.05", "3h00", "–"),
                "",
            ])[..]
        );
        assert_eq!(
            b[b.len() - 17..],
            strs(&[
                "",
                "• Usage",
                &format!("{ELBOW}5h ━━━━━━━━  58% 2h33"),
                "     7d ━━━━━━━━  31% 2d18h",
                "",
                "• Today               $9.59",
                &format!("{ELBOW}+1,376 −58 lines"),
                "     4 sessions · 12m wait",
                "",
                "• Spend 7d           $24.00",
                &format!("{ELBOW}We                $0.0"),
                "     Th ━━━━           $2.0",
                "     Fr ━━━━━━━        $4.0",
                "     Sa ━━━━━━━━━━     $6.0",
                "     Su ━━━━━━━━━━━━━━ $8.0",
                "     Mo ━━             $1.0",
                "     Tu ━━━━━          $3.0",
            ])[..]
        );
        // Today's row is no different from the others
        assert!(!lines[49].spans.iter().any(|s| s.style == STRONG));
    }

    #[test]
    fn a_long_name_goes_on_under_the_first_line() {
        let name = "Revisar PR do módulo de pagamentos";
        let first = eu("@1", name, Status::Permission, NOW - 180.0, 1.0, 60.0);
        let b = body(&entry_lines(&first, false, NOW));
        assert_eq!(
            b[..2],
            strs(&["! Revisar PR do módulo   3m", "  de pagamentos"])[..]
        );
        assert_eq!(b[2], detail("$1.00", "1m", "–"));
        // Too long for two lines: the second is cut
        let longer = e(
            "@1",
            &format!("{name} e do checkout novo"),
            Status::Done,
            NOW,
        );
        let b = body(&entry_lines(&longer, false, NOW));
        assert_eq!(
            b[..2],
            strs(&["✓ Revisar PR do módulo de", "  pagamentos e do checkout…"])[..]
        );
        // A name that fits keeps one line
        assert_eq!(entry_lines(&entries()[0], false, NOW).len(), 2);
    }

    #[test]
    fn the_waiting_time_never_changes_an_entry_height() {
        // 21 cells: one line beside the room kept for the waiting time, at any time
        let name = "aaaaaaaaaa bbbbbbbbbb";
        let lines = |waited| entry_lines(&entry_named(name, waited), false, NOW).len();
        assert_eq!([lines(0.0), lines(120.0), lines(36_000.0)], [2, 2, 2]);
        let longer = "aaaaaaaaaa bbbbbbbbbbb";
        let lines = |waited| entry_lines(&entry_named(longer, waited), false, NOW).len();
        assert_eq!([lines(0.0), lines(120.0), lines(36_000.0)], [3, 3, 3]);
    }

    fn entry_named(name: &str, waited: f64) -> Entry {
        e("@1", name, Status::Permission, NOW - waited)
    }

    #[test]
    fn a_wait_of_days_fits_the_room_kept_for_it() {
        let name = "Revisar PR do módulo de pagamentos";
        let long = entry_named(name, 100.0 * 3600.0);
        let b = body(&entry_lines(&long, false, NOW));
        assert_eq!(
            b[..2],
            strs(&["! Revisar PR do módulo   4d", "  de pagamentos"])[..]
        );
    }

    #[test]
    fn a_name_without_spaces_is_cut_inside_it() {
        let long = "claude-sessions-and-more-of-it";
        let first = eu("@1", long, Status::Permission, NOW - 180.0, 1.0, 60.0);
        let b = body(&entry_lines(&first, false, NOW));
        assert_eq!(
            b[..2],
            strs(&["! claude-sessions-and-m  3m", "  ore-of-it"])[..]
        );
        let calm = eu("@1", long, Status::Working, NOW, 1.0, 60.0);
        let b = body(&entry_lines(&calm, false, NOW));
        assert_eq!(b[1], "  of-it");
        // A word that ends right at the edge stays on the first line
        let edge = e("@1", "aaaa bbbbbbbbbbbbbbbbbbbb xyz", Status::Idle, NOW);
        let b = body(&entry_lines(&edge, false, NOW));
        assert_eq!(b[..2], strs(&["○ aaaa bbbbbbbbbbbbbbbbbbbb", "  xyz"])[..]);
        // A space that would leave the first line mostly empty is passed over
        let early = e("@1", &format!("v2 {long}"), Status::Idle, NOW);
        let b = body(&entry_lines(&early, false, NOW));
        assert_eq!(
            b[..2],
            strs(&["○ v2 claude-sessions-and-mo", "  re-of-it"])[..]
        );
    }

    #[test]
    fn the_model_shows_its_family_only_when_it_fits_whole() {
        let model = |name: &str| {
            let entry = Entry {
                model: Some(name.into()),
                ..eu("@1", "web", Status::Idle, NOW, 67.05, 600.0)
            };
            body(&entry_lines(&entry, false, NOW))[1].clone()
        };
        assert_eq!(model("Opus 5.5"), detail("$67.05", "10m", "Opus"));
        assert_eq!(
            model("Sonnet 5.5 (1M context)"),
            detail("$67.05", "10m", "Sonnet")
        );
        assert_eq!(model("claude-opus-4-1"), detail("$67.05", "10m", "–"));
        // A cost wider than its column leaves no room for a whole family
        let rich = Entry {
            model: Some("Sonnet 5.5".into()),
            ..eu("@1", "web", Status::Idle, NOW, 10_000.0, 600.0)
        };
        let b = body(&entry_lines(&rich, false, NOW));
        assert_eq!(b[1], format!("{ELBOW}$10000.00   10m  –"));
    }

    #[test]
    fn the_detail_columns_line_up_across_entries() {
        let lines: Vec<String> = entries()
            .iter()
            .map(|e| body(&entry_lines(e, false, NOW))[1].clone())
            .collect();
        let at = |l: &String, c: char| l.chars().position(|x| x == c);
        assert!(
            lines.iter().all(|l| at(l, '–') == at(&lines[0], '–')),
            "{lines:#?}"
        );
        let big = eu(
            "@1",
            "web",
            Status::Idle,
            NOW,
            123.45,
            12.0 * 3600.0 + 300.0,
        );
        let b = body(&entry_lines(&big, false, NOW));
        assert_eq!(b[1], detail("$123.45", "12h05", "–"));
    }

    #[test]
    fn the_current_entry_has_a_strong_name_and_a_lighter_background() {
        let long = eu(
            "@3",
            "a name long enough for two lines",
            Status::Shell,
            NOW,
            1.0,
            1.0,
        );
        let lines = entry_lines(&long, true, NOW);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].spans.iter().any(|s| s.style == STRONG));
        assert!(lines[1].spans.iter().any(|s| s.style == STRONG));
        for line in &lines {
            assert_eq!(cell_len(line), WIDTH);
            assert_eq!(line.style, CURRENT);
        }
        for e in entries() {
            assert_eq!(entry_lines(&e, false, NOW)[0].style, Default::default());
        }
    }

    #[test]
    fn the_mark_has_the_status_shape_and_colour_and_spins_while_working() {
        let marks = [
            (Status::Permission, '!', ERROR),
            (Status::Input, '?', WARNING),
            (Status::Done, '✓', SUCCESS),
            (Status::Shell, '◐', PERMISSION),
            (Status::Idle, '○', INACTIVE),
            (Status::Unknown, '○', INACTIVE),
        ];
        for (status, mark, colour) in marks {
            let first = &entry_lines(&entry(status, 0.0), false, NOW)[0];
            assert_eq!(plain(first).chars().nth(1), Some(mark));
            // spans[0] is the padding column
            assert_eq!(
                first.spans[1].style,
                colour.add_modifier(Modifier::BOLD),
                "{status:?}"
            );
        }
        let working = entry(Status::Working, 0.0);
        let marks: std::collections::HashSet<char> = (0..6)
            .map(|t| {
                let lines = entry_lines(&working, false, NOW + f64::from(t) / 8.0);
                plain(&lines[0]).chars().nth(1).unwrap()
            })
            .collect();
        assert_eq!(marks.len(), 6);
        assert!(!marks.contains(&'•'));
    }

    #[test]
    fn spinner_truncates_the_frame_like_python_int() {
        assert_eq!(spinner(NOW), '·');
        assert_eq!(spinner(NOW + 0.124), '·');
        assert_eq!(spinner(NOW + 0.125), '✢');
        assert_eq!(spinner(NOW + 5.0 / 8.0), '✽');
    }

    #[test]
    fn waiting_time_shows_only_for_timed_statuses_from_one_minute() {
        let done = &entry_lines(&entry(Status::Done, 600.0), false, NOW)[0];
        assert!(!plain(done).contains("10m"));
        let fresh = &entry_lines(&entry(Status::Permission, 30.0), false, NOW)[0];
        assert_eq!(plain(fresh).trim(), "! web");
    }

    #[test]
    fn unknown_usage_shows_dashes() {
        let lines = entry_lines(&e("@1", "new", Status::Working, NOW), false, NOW);
        assert_eq!(body(&lines)[1], detail("–", "–", "–"));
        let only_cost = Entry {
            cost: Some(0.5),
            ..e("@1", "new", Status::Idle, NOW)
        };
        assert_eq!(
            body(&entry_lines(&only_cost, false, NOW))[1],
            detail("$0.50", "–", "–")
        );
    }

    #[test]
    fn short_terminal_keeps_stats_and_cuts_sessions() {
        let lines = column(&entries(), "@3", &stats(), NOW, 20, &[]);
        assert_eq!(lines.len(), 20);
        assert!(plain(&lines[19]).contains("Tu"));
        assert!(plain(&lines[5]).contains("web"));
        assert!(!lines.iter().any(|l| plain(l).contains("vehicle")));
    }

    #[test]
    fn no_limits_leaves_quota_out() {
        let stats = Stats {
            week: week(),
            ..Stats::default()
        };
        let lines = body(&column(&entries(), "@3", &stats, NOW, 30, &[]));
        assert!(lines.contains(&"• Spend 7d           $24.00".to_owned()));
        assert!(!lines.contains(&"• Usage".to_owned()));
    }

    fn name_at(y: u16, height: u16, stats: &Stats) -> Option<String> {
        entry_at(&entries(), y, height, stats).map(|e| e.name.clone())
    }

    #[test]
    fn entry_at_maps_rows_to_entries() {
        let s = stats();
        // The blank lines between entries belong to none
        assert_eq!(name_at(4, 50, &s), None);
        assert_eq!(name_at(5, 50, &s).as_deref(), Some("web"));
        assert_eq!(name_at(6, 50, &s).as_deref(), Some("web"));
        assert_eq!(name_at(7, 50, &s), None);
        assert_eq!(name_at(8, 50, &s).as_deref(), Some("vehicle.k265"));
        assert_eq!(name_at(10, 50, &s), None);
        assert_eq!(name_at(11, 50, &s).as_deref(), Some("api"));
        assert_eq!(name_at(0, 50, &s), None);
        assert_eq!(name_at(40, 50, &s), None);
    }

    #[test]
    fn entry_at_counts_the_third_line_of_a_long_name() {
        let mut list = entries();
        list[0].name = "a name long enough for two lines".into();
        let at = |y| entry_at(&list, y, 50, &stats()).map(|e| e.window_id.clone());
        assert_eq!(at(7).as_deref(), Some("@1"));
        assert_eq!(at(8), None);
        assert_eq!(at(9).as_deref(), Some("@2"));
    }

    #[test]
    fn a_long_name_that_does_not_fit_leaves_its_entry_out() {
        let long = e("@1", "a name long enough for two lines", Status::Idle, NOW);
        let laid = lay(std::slice::from_ref(&long));
        assert_eq!(fitting(&laid, 2), 0);
        assert_eq!(fitting(&laid, 3), 1);
    }

    #[test]
    fn entry_at_ignores_entries_cut_by_a_short_terminal() {
        let s = stats();
        assert_eq!(name_at(5, 20, &s).as_deref(), Some("web"));
        // the 2nd entry, not drawn
        assert_eq!(name_at(8, 20, &s), None);
        let no_quota = Stats {
            week: week(),
            ..Stats::default()
        };
        // more room
        assert_eq!(name_at(8, 20, &no_quota).as_deref(), Some("vehicle.k265"));
    }

    fn runs() -> Vec<Run> {
        vec![
            Run {
                name: "daily-recap".into(),
                started: NOW - 150.0,
            },
            Run {
                name: "trending-repos".into(),
                started: NOW - 70.0,
            },
        ]
    }

    #[test]
    fn background_runs_show_under_the_sessions() {
        let lines = column(&entries()[..2], "@1", &stats(), NOW, 30, &runs());
        assert_eq!(
            body(&lines)[10..14],
            strs(&[
                "",
                "• Background",
                &format!("{:<25}2m", format!("{ELBOW}daily-recap")),
                &format!("{:<25}1m", "     trending-repos"),
            ])[..]
        );
        assert!(lines[12].spans.iter().any(|s| s.style == MAGENTA));
    }

    #[test]
    fn no_runs_no_background_section() {
        let lines = column(&entries()[..2], "@1", &stats(), NOW, 30, &[]);
        assert!(!lines.iter().any(|l| plain(l).contains("Background")));
    }

    #[test]
    fn short_terminal_drops_runs_before_sessions() {
        let lines = column(&entries()[..2], "@1", &stats(), NOW, 24, &runs());
        assert_eq!(lines.len(), 24);
        assert!(lines.iter().any(|l| plain(l).contains("vehicle.k265")));
        assert!(!lines.iter().any(|l| plain(l).contains("daily-recap")));
    }

    #[test]
    fn today_goes_first_when_the_sessions_need_its_room() {
        let with_day = Stats {
            today: 1.0,
            day: Some(day(0, 0, 2, 0.0)),
            ..stats()
        };
        let roomy = body(&column(&entries()[..2], "@1", &with_day, NOW, 40, &[]));
        assert!(roomy.contains(&"• Today               $1.00".to_owned()));
        assert!(roomy.contains(&"     2 sessions · 0m wait".to_owned()));
        let tight = body(&column(&entries()[..2], "@1", &with_day, NOW, 23, &[]));
        assert!(!tight.concat().contains("Today"));
        assert!(tight.iter().any(|l| l.contains("vehicle.k265")));
        let without = body(&column(&entries()[..2], "@1", &stats(), NOW, 40, &[]));
        assert!(!without.concat().contains("Today"));
    }

    #[test]
    fn a_new_run_keeps_its_whole_name() {
        let name = "x".repeat(DETAIL_ROOM);
        let run = Run {
            name: name.clone(),
            started: NOW - 30.0,
        };
        assert_eq!(
            body(&runs_lines(&[run], NOW, 5))[2],
            format!("{ELBOW}{name}")
        );
    }

    #[test]
    fn no_room_for_a_run_means_no_section() {
        assert!(runs_lines(&runs(), NOW, 2).is_empty());
        assert!(runs_lines(&runs(), NOW, -4).is_empty());
        assert_eq!(runs_lines(&runs(), NOW, 3).len(), 3);
    }

    #[test]
    fn the_waiting_time_keeps_a_space_before_it() {
        let wide = eu(
            "@1",
            "名前名前名前名前名前名前",
            Status::Permission,
            NOW - 720.0,
            1.0,
            60.0,
        );
        let first = &entry_lines(&wide, false, NOW)[0];
        assert_eq!(cell_len(first), WIDTH);
        let text = plain(first);
        assert!(text.trim_end().ends_with(" 12m"));
    }

    #[test]
    fn an_unknown_status_is_listed_with_the_idle_sessions() {
        let odd = e("@9", "odd", Status::Unknown, NOW);
        let notes = e("@5", "notes", Status::Idle, NOW);
        let first = entries()[0].clone();
        let list = [odd.clone(), notes.clone(), first.clone()];
        assert_eq!(ordered(&list), vec![&first, &odd, &notes]);
        let laid = lay(std::slice::from_ref(&odd));
        assert_eq!((fitting(&laid, 10), laid[0].height()), (1, 2));
    }

    #[test]
    fn step_follows_the_listed_order_and_wraps() {
        // In window order: docs, web, notes, api, vehicle.k265. Listed: web (needs you), api
        // and vehicle.k265 (running, in window order), docs (done), notes (idle)
        let all = entries();
        let shuffled: Vec<Entry> = [3, 0, 4, 2, 1].iter().map(|&i| all[i].clone()).collect();
        let order = ["@1", "@3", "@2", "@4", "@5"];
        for (i, window) in order.iter().enumerate() {
            assert_eq!(
                step(&shuffled, window, 1).as_deref(),
                Some(order[(i + 1) % 5])
            );
            assert_eq!(
                step(&shuffled, window, -1).as_deref(),
                Some(order[(i + 4) % 5])
            );
        }
        // From the landing page: down to the first, up to the last
        assert_eq!(step(&shuffled, "@0", 1).as_deref(), Some("@1"));
        assert_eq!(step(&shuffled, "@0", -1).as_deref(), Some("@5"));
        assert_eq!(step(&[], "@0", 1), None);
    }

    #[test]
    fn looking_at_a_finished_session_does_not_move_it() {
        let done = [
            e("@1", "a", Status::Done, NOW),
            e("@2", "b", Status::Idle, NOW),
            e("@3", "c", Status::Done, NOW),
        ];
        let seen = [
            done[0].clone(),
            e("@2", "b", Status::Idle, NOW),
            e("@3", "c", Status::Idle, NOW),
        ];
        let ids = |l: &[Entry]| -> Vec<String> {
            ordered(l).iter().map(|e| e.window_id.clone()).collect()
        };
        assert_eq!(ids(&done), ["@1", "@2", "@3"]);
        assert_eq!(ids(&seen), ["@1", "@2", "@3"]);
    }

    fn pane(pane_id: &str, window_id: &str, index: u32, window_index: u32, name: &str) -> Pane {
        Pane {
            pane_id: pane_id.into(),
            window_id: window_id.into(),
            index,
            window_index,
            sidebar: false,
            landing: false,
            window_name: name.into(),
        }
    }

    fn record(id: &str, pane: &str, status: Status, since: f64, cost: f64) -> Record {
        Record {
            id: id.into(),
            status,
            status_since: since,
            mux: Some(Mux {
                socket: "claude".into(),
                pane: pane.into(),
            }),
            usage: Some(Usage {
                cost: Some(cost),
                api_duration: Some(60.0),
                model: Some(format!("m-{id}")),
                ..Usage::default()
            }),
            ..Record::default()
        }
    }

    #[test]
    fn load_entries_lists_session_windows_in_window_order() {
        let panes = [
            pane("%0", "@0", 0, 0, "home"),
            pane("%1", "@1", 0, 1, "one"),
            pane("%2", "@1", 1, 1, "one"),
            pane("%3", "@2", 0, 2, "two"),
        ];
        // Most urgent first: the window's status is s2's, its details s1's (the first pane)
        let records = [
            record("s2", "%2", Status::Permission, 50.0, 2.0),
            record("s1", "%1", Status::Working, 40.0, 1.0),
            record("s3", "%3", Status::Done, 30.0, 3.0),
        ];
        let entries = load_entries(&records, &panes, "claude");
        assert_eq!(
            entries,
            vec![
                Entry {
                    window_id: "@1".into(),
                    name: "one".into(),
                    status: Status::Permission,
                    since: 50.0,
                    cost: Some(1.0),
                    duration: Some(60.0),
                    model: Some("m-s1".into()),
                },
                Entry {
                    window_id: "@2".into(),
                    name: "two".into(),
                    status: Status::Done,
                    since: 30.0,
                    cost: Some(3.0),
                    duration: Some(60.0),
                    model: Some("m-s3".into()),
                },
            ]
        );
        assert!(load_entries(&records, &panes, "other").is_empty());
    }

    const STATUSES: [Status; 7] = [
        Status::Permission,
        Status::Input,
        Status::Done,
        Status::Working,
        Status::Shell,
        Status::Idle,
        Status::Unknown,
    ];

    fn arb_entry() -> impl proptest::strategy::Strategy<Value = Entry> {
        use proptest::prelude::*;
        (
            "[a-z日本é名前 ]{0,40}",
            0..STATUSES.len(),
            0.0..200_000.0f64,
            proptest::option::of(0.0..5000.0f64),
            proptest::option::of(0.0..400_000.0f64),
            proptest::option::of("[a-zA-Z0-9 .\\[\\]-]{0,30}"),
        )
            .prop_map(|(name, status, ago, cost, duration, model)| Entry {
                window_id: String::new(),
                name,
                status: STATUSES[status],
                since: NOW - ago,
                cost,
                duration,
                model,
            })
    }

    proptest::proptest! {
        #[test]
        fn column_never_exceeds_width_or_height(
            height in 1u16..=60,
            mut list in proptest::collection::vec(arb_entry(), 0..8),
            current in 0usize..9,
            runs in proptest::collection::vec(("[a-z日本 -]{0,40}", 0.0..9000.0f64), 0..5),
            with_day in proptest::bool::ANY,
            with_limits in proptest::bool::ANY,
            spend in proptest::collection::vec(0.0..500.0f64, 7),
        ) {
            use ratatui::widgets::{Paragraph, Widget};
            for (i, entry) in list.iter_mut().enumerate() {
                entry.window_id = format!("@{i}");
            }
            let current = format!("@{current}");
            let runs: Vec<Run> = runs
                .into_iter()
                .map(|(name, ago)| Run { name, started: NOW - ago })
                .collect();
            let stats = Stats {
                limits: if with_limits { limits() } else { Default::default() },
                today: spend[6],
                week: week().into_iter().zip(spend).map(|((d, _), v)| (d, v)).collect(),
                day: with_day.then(|| day(123_456, 7_890, 12, 4000.0)),
                ..Stats::default()
            };
            let lines = column(&list, &current, &stats, NOW, height, &runs);
            proptest::prop_assert_eq!(lines.len(), usize::from(height));
            for line in &lines {
                proptest::prop_assert_eq!(cell_len(line), WIDTH, "{:?}", plain(line));
            }
            let area = ratatui::layout::Rect::new(0, 0, WIDTH as u16, height);
            let mut buf = ratatui::buffer::Buffer::empty(area);
            Paragraph::new(lines.clone()).render(area, &mut buf);
            // Nothing cropped: each row shows its whole line, and the line ends at the edge
            for (y, line) in lines.iter().enumerate() {
                proptest::prop_assert_eq!(row_text(&buf, y as u16), plain(line));
            }
        }
    }
}
