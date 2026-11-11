//! The sidebar column: mascot, sessions by urgency, background runs, and the statistics.
//!
//! It reads like Claude Code's own transcript: each session is a line with its status's mark,
//! and a `⎿` line under it. The landing page's wide statistics ([`panel_lines`]) are built from
//! the same blocks.

use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, NaiveDate};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;

use crate::claude::statusline::Quota;
use crate::core::state::{self, Record, Status, NEEDS_YOU, TIMED};
use crate::term::tmux::{self, Pane};
use crate::ui::line::{cell_len, cells, ljust, pad_right, push, rjust, truncate, width_of};
use crate::ui::usage::{Day, Totals};
use crate::ui::words::{
    self, BAR, CLAUDE, CURRENT, ERROR, INACTIVE, INK, MAGENTA, MASCOT, MASCOT_ROWS, PERMISSION,
    STRONG, SUBTLE, SUCCESS, WARNING,
};

/// The sidebar pane's width.
pub const WIDTH: usize = 29;
/// The room inside the one-cell padding on each side.
pub const CONTENT: usize = 27;
/// The landing page's wide statistics: their width.
pub const PANEL_WIDTH: usize = 56;
/// The room inside the landing page's padding.
pub const PANEL_CONTENT: usize = PANEL_WIDTH - 2;

/// What a `⎿` line puts before its text.
const ELBOW: &str = "  ⎿  ";
/// What the lines after a `⎿` line put before their text.
const INDENT: &str = "     ";
/// The cells [`ELBOW`] and [`INDENT`] take.
const ELBOW_CELLS: usize = 5;
/// The room a `⎿` line leaves after the elbow.
const DETAIL_ROOM: usize = CONTENT - ELBOW_CELLS;
/// A blank line, the mascot (3) and a blank line: the mascot level with Claude Code's own.
const HEAD: i64 = 5;
/// Lines of a session entry whose name fits one line, not counting the blank line between
/// entries; a longer name adds one (see [`entry_height`]).
const ENTRY: usize = 2;
/// The cells kept for the waiting time beside a name: `59m` or `12h`.
const LABEL_CELLS: usize = 3;
/// The cells the cost takes on a `⎿` line, right-aligned: `$123.45`.
const COST_CELLS: usize = 7;
/// The cells Claude's time takes on a `⎿` line, right-aligned: ` 12h05`.
const TIME_CELLS: usize = 6;
/// Two-letter day names, for the narrow chart.
const DAY_NAMES: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
/// Active hours: a cell per hour, from no work to the busiest hour.
const HEAT: [&str; 5] = ["·", "░", "▒", "▓", "█"];
/// The hours ruled under the active hours.
const HOURS_RULER: &str = "0     6     12    18";
/// From this many columns the chart names days with three letters and the quota says
/// "resets in".
const WIDE_CHART: usize = 40;
/// How many names the spend by folder and by model list.
const TOP_RANKED: usize = 4;
/// The cells a ranked name takes.
const RANKED_NAME: usize = 16;
/// `⏺` shows as an emoji button in some fonts.
const BULLET: &str = "•";
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

/// What the bottom of the sidebar shows: today, the quota and the week's spend.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Stats {
    /// The usage windows by name (`five_hour`, `seven_day`).
    pub limits: BTreeMap<String, Quota>,
    /// Dollars spent today.
    pub today: f64,
    /// `(date, dollars)` for each of the last 7 days, oldest first, today last.
    pub week: Vec<(NaiveDate, f64)>,
    /// What today's journal adds up to; `None` leaves the Today block out.
    pub day: Option<Day>,
    /// The week's totals, for the landing page's Last 7 days block.
    pub totals: Option<Totals>,
    /// Seconds sessions worked in each hour of the day over the last 7 days, for the landing
    /// page's week block.
    pub hours: Vec<f64>,
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
            let usage = window.head().usage.clone().unwrap_or_default();
            Some(Entry {
                status: window.lead.status,
                since: state::since(&window.lead),
                cost: usage.cost,
                duration: usage.api_duration,
                model: usage.model,
                window_id,
                name,
            })
        })
        .collect()
}

/// Where a status is listed: what needs the user first, then what runs, then the rest.
///
/// Finished, idle and unknown statuses share the last place, so looking at a finished session
/// (which makes it idle) never moves it.
fn urgency(status: Status) -> u8 {
    if NEEDS_YOU.contains(&status) {
        0
    } else if matches!(status, Status::Working | Status::Shell) {
        1
    } else {
        2
    }
}

/// The entries in the order the sidebar lists them: by urgency, then by window.
pub fn ordered(entries: &[Entry]) -> Vec<&Entry> {
    let mut out: Vec<&Entry> = entries.iter().collect();
    // A stable sort keeps the window order within each place
    out.sort_by_key(|e| urgency(e.status));
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

/// A line starting with the left padding column, over a `base` style.
fn start(base: Style) -> Line<'static> {
    Line::from(Span::raw(" ")).style(base)
}

/// Pads a line to `content` and the two padding columns, so a background reaches both edges.
fn finish(mut line: Line<'static>, content: usize) -> Line<'static> {
    pad_right(&mut line, content + 2);
    line
}

/// Ends a line with `value` against the right edge.
fn right(line: &mut Line<'static>, value: &str, style: Style, content: usize) {
    let used = cell_len(line).saturating_sub(1) + width_of(value);
    push(line, " ".repeat(content.saturating_sub(used)), INK);
    push(line, value, style);
}

/// A `•` line: the mark, a title cut to fit, and a value on the right.
struct Bullet<'a> {
    mark: &'a str,
    mark_style: Style,
    title: &'a str,
    title_style: Style,
    value: &'a str,
    value_style: Style,
    base: Style,
    content: usize,
}

impl Bullet<'_> {
    fn line(&self) -> Line<'static> {
        let mut line = start(self.base);
        push(&mut line, format!("{} ", self.mark), self.mark_style);
        let taken = if self.value.is_empty() {
            0
        } else {
            width_of(self.value) + 1
        };
        let room = self.content.saturating_sub(2 + taken);
        push(&mut line, words::fit(self.title, room), self.title_style);
        if !self.value.is_empty() {
            right(&mut line, self.value, self.value_style, self.content);
        }
        line
    }
}

/// The start of a `⎿` line, or of the lines after it, which only indent.
fn elbow(first: bool, base: Style) -> Line<'static> {
    let mut line = start(base);
    push(&mut line, if first { ELBOW } else { INDENT }, SUBTLE);
    line
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
fn mark(status: Status, now: f64) -> String {
    match status {
        Status::Permission => "!".to_owned(),
        Status::Input => "?".to_owned(),
        Status::Shell => "◐".to_owned(),
        Status::Done => "✓".to_owned(),
        Status::Working => spinner(now).to_string(),
        Status::Idle | Status::Unknown => "○".to_owned(),
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

/// The lines of a session entry: two, or three when its name goes on to a second line.
fn entry_height(entry: &Entry) -> usize {
    ENTRY + usize::from(!split_name(entry).1.is_empty())
}

/// A `⎿` field, or `–` for an unknown one.
fn or_dash(text: String) -> String {
    if text.is_empty() {
        "–".to_owned()
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
fn entry_lines(entry: &Entry, current: bool, now: f64) -> Vec<Line<'static>> {
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
    let (head, rest) = split_name(entry);
    let mut lines = vec![Bullet {
        mark: &mark,
        mark_style: mark_style.add_modifier(Modifier::BOLD),
        title: &head,
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
        push(&mut more, words::fit(&rest, CONTENT - 2), name_style);
        lines.push(more);
    }

    let cost = rjust(&or_dash(words::money(entry.cost)), COST_CELLS);
    let time = rjust(&or_dash(words::span(entry.duration)), TIME_CELLS);
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
    push(
        &mut detail,
        format!("  {}", or_dash(model.to_owned())),
        INACTIVE,
    );
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

/// A statistics block's `•` line: its name, and its total on the right.
fn block(title: &str, value: &str, content: usize) -> Line<'static> {
    Bullet {
        mark: BULLET,
        mark_style: CLAUDE,
        title,
        title_style: STRONG,
        value,
        value_style: STRONG,
        base: Style::new(),
        content,
    }
    .line()
}

/// `value` as a horizontal bar scaled so `top` fills `cells`.
///
/// Any value above zero shows at least one cell; the scaled length rounds half to even.
fn hbar(value: f64, top: f64, cells: usize) -> String {
    let filled = if top != 0.0 {
        (value / top * cells as f64).round_ties_even()
    } else {
        0.0
    };
    let least = if value > 0.0 { 1.0 } else { 0.0 };
    BAR.repeat(filled.min(cells as f64).max(least) as usize)
}

/// Appends a chart bar for `value` scaled to `top`, padded so `value_text` ends the line at
/// `content` cells.
fn push_bar(line: &mut Line<'static>, value: f64, top: f64, value_text: &str, content: usize) {
    let used = cell_len(line).saturating_sub(1) + 1 + width_of(value_text);
    let cells = content.saturating_sub(used);
    push(line, ljust(&hbar(value, top, cells), cells + 1), CLAUDE);
}

/// An unsigned count with a comma between thousands.
fn thousands(n: u64) -> String {
    words::thousands(i64::try_from(n).unwrap_or(i64::MAX))
}

/// A `⎿` line with the lines added and removed, cut to the content.
fn changed(added: u64, removed: u64, content: usize) -> Line<'static> {
    let mut line = elbow(true, Style::new());
    push(&mut line, format!("+{}", thousands(added)), SUCCESS);
    push(&mut line, format!(" −{}", thousands(removed)), ERROR);
    push(&mut line, " lines", INACTIVE);
    truncate(&mut line, 1 + content);
    line
}

/// A day's sessions and the time they waited on you, as `4 sessions · 12m wait`.
fn sessions_waited(day: &Day) -> String {
    let plural = if day.sessions == 1 { "" } else { "s" };
    let waited = words::span(Some(day.waited));
    format!("{} session{plural} · {waited} wait", day.sessions)
}

/// Today: its spend, the lines changed, the sessions and the time spent waiting.
///
/// The counts share the lines' row when they fit, else go on a row of their own.
fn today_lines(day: &Day, spent: f64, content: usize) -> Vec<Line<'static>> {
    let mut lines_changed = changed(day.added, day.removed, content);
    let said = sessions_waited(day);
    let head = block("Today", &words::money(Some(spent)), content);
    if cell_len(&lines_changed) - 1 + 3 + width_of(&said) <= content {
        push(&mut lines_changed, format!(" · {said}"), INACTIVE);
        return vec![head, lines_changed];
    }
    let mut counts = elbow(false, Style::new());
    let room = content.saturating_sub(ELBOW_CELLS);
    push(&mut counts, words::fit(&said, room), INACTIVE);
    vec![head, lines_changed, counts]
}

/// Usage: the 5-hour and 7-day quota bars, with when each resets; none without quota.
fn usage_lines(limits: &BTreeMap<String, Quota>, now: f64, content: usize) -> Vec<Line<'static>> {
    let wide = content >= WIDE_CHART;
    let mut lines = Vec::new();
    for (label, key) in [("5h", "five_hour"), ("7d", "seven_day")] {
        let Some(quota) = limits.get(key) else {
            continue;
        };
        let mut line = elbow(lines.is_empty(), Style::new());
        push(&mut line, format!("{label} "), INACTIVE);
        let left = words::remaining(quota.resets_at - now);
        let reset = if wide {
            format!("resets in {left}")
        } else {
            left
        };
        // The room the longest reset takes, so both bars end in the same column
        let room = if wide { "resets in ".len() } else { 0 } + 5;
        let cells = content.saturating_sub(ELBOW_CELLS + 3 + 5 + 1 + room);
        line.spans.extend(words::bar(quota.pct, cells));
        let pct = quota.pct.round_ties_even() as i64;
        push(&mut line, rjust(&format!("{pct}%"), 5), Style::new());
        push(&mut line, format!(" {reset}"), INACTIVE);
        lines.push(line);
    }
    if !lines.is_empty() {
        lines.insert(0, block("Usage", "", content));
    }
    lines
}

/// The week's spend; Python's sum starts from the integer 0, so an empty week is $0.00, never
/// -0.00.
fn week_total(week: &[(NaiveDate, f64)]) -> f64 {
    week.iter().fold(0.0, |sum, (_, v)| sum + v)
}

/// A row per day with a bar scaled to the most expensive one.
///
/// A wide block names each day with three letters, a narrow one with two.
fn day_rows(week: &[(NaiveDate, f64)], content: usize) -> Vec<Line<'static>> {
    let top = week.iter().map(|(_, v)| *v).fold(0.0, f64::max);
    let wide = content >= WIDE_CHART;
    let values: Vec<String> = week
        .iter()
        .map(|(_, v)| {
            if wide {
                format!("${v:.2}")
            } else {
                format!("${v:.1}")
            }
        })
        .collect();
    let value_cells = values.iter().map(|v| width_of(v)).max().unwrap_or(0);
    let mut lines = Vec::new();
    for ((day, dollars), value) in week.iter().zip(&values) {
        let weekday = day.weekday().num_days_from_monday() as usize;
        let name = if wide {
            words::WEEKDAYS[weekday]
        } else {
            DAY_NAMES[weekday]
        };
        let mut line = elbow(false, Style::new());
        push(&mut line, format!("{name} "), INACTIVE);
        let value = rjust(value, value_cells);
        push_bar(&mut line, *dollars, top, &value, content);
        push(&mut line, value, INACTIVE);
        lines.push(line);
    }
    lines
}

/// Turns the first of a block's indented lines into its `⎿` line.
fn hook_first(lines: &mut [Line<'static>]) {
    if let Some(span) = lines.first_mut().and_then(|l| l.spans.get_mut(1)) {
        if span.content == INDENT {
            *span = Span::styled(ELBOW, SUBTLE);
        }
    }
}

/// Spend 7d: the total, then a row per day.
fn chart_lines(week: &[(NaiveDate, f64)], content: usize) -> Vec<Line<'static>> {
    let mut rows = day_rows(week, content);
    hook_first(&mut rows);
    let head = block("Spend 7d", &words::money(Some(week_total(week))), content);
    std::iter::once(head).chain(rows).collect()
}

/// What cost most this week, by name, with a bar scaled to the first.
///
/// # Arguments
/// * `ranked`: `(name, dollars)` pairs, most expensive first; the first [`TOP_RANKED`] show.
/// * `content`: columns available.
fn ranked_rows(ranked: &[(String, f64)], content: usize) -> Vec<Line<'static>> {
    let shown = &ranked[..ranked.len().min(TOP_RANKED)];
    let Some((_, top)) = shown.first() else {
        return Vec::new();
    };
    let values: Vec<String> = shown.iter().map(|(_, v)| words::money(Some(*v))).collect();
    let value_cells = values.iter().map(|v| width_of(v)).max().unwrap_or(0);
    let mut lines = Vec::new();
    for ((name, dollars), value) in shown.iter().zip(&values) {
        let mut line = elbow(false, Style::new());
        let name = ljust(&words::fit(name, RANKED_NAME), RANKED_NAME + 1);
        push(&mut line, name, INK);
        let value = rjust(value, value_cells);
        push_bar(&mut line, *dollars, *top, &value, content);
        push(&mut line, value, INACTIVE);
        lines.push(line);
    }
    lines
}

/// A cell per hour of the day, darker the longer sessions worked in it, ruled under it.
///
/// # Arguments
/// * `hours`: seconds sessions worked in each hour of the day, midnight first; none drawn
///   without any work.
fn hours_lines(hours: &[f64]) -> Vec<Line<'static>> {
    let top = hours.iter().copied().fold(0.0, f64::max);
    if top == 0.0 {
        return Vec::new();
    }
    let darkest = (HEAT.len() - 1) as f64;
    let mut line = elbow(false, Style::new());
    for &seconds in hours {
        if seconds == 0.0 {
            push(&mut line, HEAT[0], SUBTLE);
            continue;
        }
        // Rounded up, so any work shows
        let level = -words::floor_div(-seconds * darkest, top);
        push(&mut line, HEAT[level.clamp(0.0, darkest) as usize], CLAUDE);
    }
    let mut ruler = elbow(false, Style::new());
    push(&mut ruler, HOURS_RULER, INACTIVE);
    vec![line, ruler]
}

/// A heading inside a block: an indented rule to the right edge with `title` in it, as
/// `── Hours ──────`.
fn heading(title: &str, content: usize) -> Line<'static> {
    let mut line = elbow(false, Style::new());
    push(&mut line, "── ", SUBTLE);
    push(&mut line, title, CLAUDE);
    let rest = content.saturating_sub(ELBOW_CELLS + 4 + width_of(title));
    push(&mut line, format!(" {}", "─".repeat(rest)), SUBTLE);
    line
}

/// The landing page's last 7 days in one block: the spend, the lines and sessions, then the
/// hours worked, the spend per day, by folder and, with `models`, by model.
///
/// A part with nothing to show is left out with its heading.
fn week_lines(stats: &Stats, content: usize, models: bool) -> Vec<Line<'static>> {
    let total = words::money(Some(week_total(&stats.week)));
    let mut rows = Vec::new();
    if let Some(totals) = &stats.totals {
        let mut summary = changed(totals.added, totals.removed, content);
        let plural = if totals.sessions == 1 { "" } else { "s" };
        push(
            &mut summary,
            format!(" · {} session{plural}", totals.sessions),
            INACTIVE,
        );
        truncate(&mut summary, 1 + content);
        rows.push(summary);
    }
    let ranked = |pick: fn(&Totals) -> &Vec<(String, f64)>| {
        stats
            .totals
            .as_ref()
            .map_or_else(Vec::new, |t| ranked_rows(pick(t), content))
    };
    let mut parts = vec![
        ("Hours", hours_lines(&stats.hours)),
        ("Per day", day_rows(&stats.week, content)),
        ("By folder", ranked(|t| &t.by_project)),
    ];
    if models {
        parts.push(("By model", ranked(|t| &t.by_model)));
    }
    for (title, lines) in parts.into_iter().filter(|(_, l)| !l.is_empty()) {
        rows.push(heading(title, content));
        rows.extend(lines);
    }
    hook_first(&mut rows);
    std::iter::once(block("Last 7 days", &total, content))
        .chain(rows)
        .collect()
}

/// The lines of the blocks that have any, a blank line between them.
fn spaced(blocks: Vec<Vec<Line<'static>>>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for block in blocks.into_iter().filter(|b| !b.is_empty()) {
        if !lines.is_empty() {
            lines.push(start(Style::new()));
        }
        lines.extend(block);
    }
    lines
}

/// The statistics at the bottom, under a blank line and with one between blocks: Usage,
/// Today and Spend 7d.
///
/// # Arguments
/// * `stats`: what to show.
/// * `now`: current time as a Unix timestamp.
/// * `today`: whether to show the Today block; it goes first when room is short.
pub fn stats_lines(stats: &Stats, now: f64, today: bool) -> Vec<Line<'static>> {
    let mut blocks = vec![usage_lines(&stats.limits, now, CONTENT)];
    if let Some(day) = stats.day.as_ref().filter(|_| today) {
        blocks.push(today_lines(day, stats.today, CONTENT));
    }
    blocks.push(chart_lines(&stats.week, CONTENT));
    std::iter::once(start(Style::new()))
        .chain(spaced(blocks))
        .map(|l| finish(l, CONTENT))
        .collect()
}

/// The landing page's wide statistics, a blank line between blocks: Usage, Today, and the
/// last 7 days in one block, by model only with `models`. Each line is padded to
/// `content + 2` cells.
pub fn panel_lines(stats: &Stats, now: f64, content: usize, models: bool) -> Vec<Line<'static>> {
    let mut blocks = vec![usage_lines(&stats.limits, now, content)];
    if let Some(day) = &stats.day {
        blocks.push(today_lines(day, stats.today, content));
    }
    blocks.push(week_lines(stats, content, models));
    spaced(blocks)
        .into_iter()
        .map(|l| finish(l, content))
        .collect()
}

/// A row of the session list.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row<'a> {
    /// The blank line between two entries.
    Blank,
    /// The first line of an entry, with its mark.
    First(&'a Entry),
    /// The other lines of an entry: the rest of its name, and the `⎿` line.
    Rest(&'a Entry),
}

/// Lays the sessions out in `room` lines at most.
///
/// An entry that does not fit, and all after it, are left out.
fn plan(entries: &[Entry], room: i64) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    for entry in ordered(entries) {
        let before = usize::from(!rows.is_empty());
        let height = entry_height(entry);
        if (rows.len() + before + height) as i64 > room {
            break;
        }
        if before > 0 {
            rows.push(Row::Blank);
        }
        rows.push(Row::First(entry));
        rows.extend(std::iter::repeat_n(Row::Rest(entry), height - 1));
    }
    rows
}

/// The statistics that fit under the sessions: Today goes when the sessions need its room.
fn bottom_lines(entries: &[Entry], stats: &Stats, now: f64, height: i64) -> Vec<Line<'static>> {
    let full = stats_lines(stats, now, true);
    let shown = |room| plan(entries, room).len();
    if shown(height - HEAD - full.len() as i64) == shown(height) {
        return full;
    }
    stats_lines(stats, now, false)
}

/// The entry drawn on row `y` of a `height`-line column, or `None`.
///
/// The blank lines between entries, and entries a short column leaves out, belong to none.
pub fn entry_at<'a>(entries: &'a [Entry], y: u16, height: u16, stats: &Stats) -> Option<&'a Entry> {
    let height = i64::from(height);
    let bottom = bottom_lines(entries, stats, 0.0, height).len() as i64;
    let rows = plan(entries, height - HEAD - bottom);
    let row = usize::try_from(i64::from(y) - HEAD).ok()?;
    match rows.get(row)? {
        Row::First(entry) | Row::Rest(entry) => Some(entry),
        Row::Blank => None,
    }
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
    let bottom = bottom_lines(entries, stats, now, height);
    let room = height - HEAD - bottom.len() as i64;
    let mut body = Vec::new();
    for row in plan(entries, room) {
        match row {
            Row::First(entry) => body.extend(entry_lines(entry, entry.window_id == current, now)),
            Row::Blank => body.push(blank()),
            Row::Rest(_) => {}
        }
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
    use chrono::{Days, NaiveDate};
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    use crate::claude::statusline::Quota;
    use crate::core::state::{Mux, Record, Status, Usage};
    use crate::term::tmux::Pane;
    use crate::ui::line::{cell_len, plain, row_text};
    use crate::ui::sidebar::{
        column, entry_at, entry_lines, hbar, hours_lines, load_entries, ordered, panel_lines, plan,
        runs_lines, spinner, stats_lines, step, today_lines, Entry, Row, Run, Stats, CONTENT,
        DETAIL_ROOM, PANEL_WIDTH, WIDTH,
    };
    use crate::ui::usage::{Day, Totals};
    use crate::ui::words::{
        CURRENT, ERROR, INACTIVE, MAGENTA, PERMISSION, STRONG, SUCCESS, WARNING,
    };

    /// 2026-11-03 14:07:00 UTC, a Tuesday; a multiple of 5 s, so the spinner is at frame 0.
    const NOW: f64 = 1_793_714_820.0;
    const ELBOW: &str = "  ⎿  ";

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 3).unwrap()
    }

    fn limits() -> std::collections::BTreeMap<String, Quota> {
        [
            ("five_hour", 58.0, NOW + 2.0 * 3600.0 + 33.0 * 60.0),
            // 2026-11-06 09:00
            ("seven_day", 31.0, NOW + 240_780.0),
        ]
        .into_iter()
        .map(|(k, pct, resets_at)| (k.to_owned(), Quota { pct, resets_at }))
        .collect()
    }

    fn week() -> Vec<(NaiveDate, f64)> {
        [0.0, 2.0, 4.0, 6.0, 8.0, 1.0, 3.0]
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                (
                    today().checked_sub_days(Days::new(6 - i as u64)).unwrap(),
                    v,
                )
            })
            .collect()
    }

    fn stats() -> Stats {
        Stats {
            limits: limits(),
            today: 9.59,
            week: week(),
            ..Stats::default()
        }
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

    /// The content columns of each line, without the padding or trailing spaces.
    fn body(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                let text: String = plain(l).chars().skip(1).take(CONTENT).collect();
                text.trim_end().to_owned()
            })
            .collect()
    }

    /// A `⎿` line: the cost and Claude's time right-aligned, then the model.
    fn detail(cost: &str, time: &str, model: &str) -> String {
        format!("{ELBOW}{cost:>7}{time:>6}  {model}")
    }

    fn entry(status: Status, waited: f64) -> Entry {
        eu("@1", "web", status, NOW - waited, 3.10, 2400.0)
    }

    fn day(added: u64, removed: u64, sessions: u32, waited: f64) -> Day {
        Day {
            added,
            removed,
            sessions,
            waited,
        }
    }

    /// A heading of the landing page's week block, after `lead`.
    fn rule(lead: &str, title: &str) -> String {
        let rest = PANEL_WIDTH - 2 - 9 - title.len();
        format!("{lead}── {title} {}", "─".repeat(rest))
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
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
    fn a_week_with_no_spend_draws_empty_rows() {
        let empty = Stats {
            week: week().into_iter().map(|(d, _)| (d, 0.0)).collect(),
            ..Stats::default()
        };
        let b = body(&stats_lines(&empty, NOW, true));
        assert_eq!(b.last().unwrap(), "     Tu                $0.0");
    }

    #[test]
    fn an_empty_journal_shows_a_positive_zero_spend() {
        let dir = tempfile::tempdir().unwrap();
        let mut reader = crate::ui::usage::Reader::in_dir(dir.path().to_path_buf());
        let totals = reader.totals(&[], 0, NOW).clone();
        let stats = Stats {
            today: totals.today,
            week: totals.days,
            day: Some(Day::default()),
            ..Stats::default()
        };
        let b = body(&stats_lines(&stats, NOW, true));
        assert_eq!(b[1], "• Today               $0.00");
        assert!(b.iter().any(|l| l.ends_with(" $0.0")), "{b:#?}");
        assert!(!b.iter().any(|l| l.contains('-')), "{b:#?}");
    }

    #[test]
    fn hbar_draws_at_least_one_cell_for_any_spend() {
        assert_eq!(hbar(1.0, 2.0, 4), "━━");
        assert_eq!(hbar(0.001, 100.0, 4), "━");
        assert_eq!(hbar(0.0, 0.0, 4), "");
    }

    #[test]
    fn hbar_rounds_half_to_even() {
        // round(2.5) == 2 in Python
        assert_eq!(hbar(5.0, 8.0, 4), "━━");
    }

    #[test]
    fn the_panel_puts_the_week_in_one_block() {
        let by = [
            ("claude-sessions", 18.4),
            ("api", 9.3),
            ("dotfiles", 2.1),
            ("notes", 2.0),
            ("x", 1.0),
        ];
        let mut hours = vec![0.0; 24];
        hours[9] = 3600.0;
        let totals = Totals {
            by_project: by.iter().map(|(n, v)| ((*n).into(), *v)).collect(),
            by_model: vec![("Opus 5.5".into(), 20.0), ("Sonnet 5.5".into(), 4.0)],
            added: 8940,
            removed: 2115,
            sessions: 38,
            ..Totals::default()
        };
        let mut stats = Stats {
            day: Some(day(1376, 58, 4, 720.0)),
            totals: Some(totals),
            hours,
            ..stats()
        };
        let rendered = panel_lines(&stats, NOW, PANEL_WIDTH - 2, true);
        assert!(rendered.iter().all(|l| cell_len(l) == PANEL_WIDTH));
        let lines: Vec<String> = rendered
            .iter()
            .map(|l| plain(l).trim_end().to_owned())
            .collect();
        assert_eq!(
            lines[..13],
            strs(&[
                " • Usage",
                &lines[1],
                &lines[2],
                "",
                " • Today                                          $9.59",
                "   ⎿  +1,376 −58 lines · 4 sessions · 12m wait",
                "",
                " • Last 7 days                                   $24.00",
                "   ⎿  +8,940 −2,115 lines · 38 sessions",
                &rule("      ", "Hours"),
                &format!("      {}█{}", "·".repeat(9), "·".repeat(14)),
                "      0     6     12    18",
                &rule("      ", "Per day"),
            ])[..]
        );
        assert!(lines[1].ends_with("58% resets in 2h33"));
        let column_of = |l: &str| l.chars().position(|c| c == '%');
        assert_eq!(column_of(&lines[1]), column_of(&lines[2]));
        assert_eq!(
            lines[13],
            "      Wed                                         $0.00"
        );
        assert!(lines[17].ends_with("━ $8.00"));
        assert_eq!(lines[20], rule("      ", "By folder"));
        assert!(lines[21].starts_with("      claude-sessions  ") && lines[21].ends_with("$18.40"));
        assert!(!lines.iter().any(|l| l.contains(" x ")));
        let n = lines.len();
        assert_eq!(lines[n - 3], rule("      ", "By model"));
        assert!(
            lines[n - 2].starts_with("      Opus 5.5         ") && lines[n - 2].ends_with("$20.00")
        );
        assert!(
            lines[n - 1].starts_with("      Sonnet 5.5       ") && lines[n - 1].ends_with("$4.00")
        );
        // No work recorded this week: no hours, nor their heading
        stats.hours = vec![0.0; 24];
        assert_eq!(panel_lines(&stats, NOW, PANEL_WIDTH - 2, true).len(), n - 3);
        // Without models their part goes
        assert_eq!(
            panel_lines(&stats, NOW, PANEL_WIDTH - 2, false).len(),
            n - 3 - 3
        );
        // Without the week's totals the per-day rows start the block
        stats.totals = None;
        let bare: Vec<String> = panel_lines(&stats, NOW, PANEL_WIDTH - 2, true)
            .iter()
            .map(|l| plain(l).trim_end().to_owned())
            .collect();
        assert_eq!(bare[bare.len() - 8], rule("   ⎿  ", "Per day"));
    }

    #[test]
    fn active_hours_shade_each_hour_by_the_busiest() {
        let mut hours = vec![0.0; 24];
        hours[9] = 3600.0;
        hours[10] = 1800.0;
        hours[14] = 100.0;
        let lines: Vec<String> = hours_lines(&hours)
            .iter()
            .map(|l| {
                plain(l)
                    .chars()
                    .skip(1)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            lines[0],
            format!("     {}█▒···░{}", "·".repeat(9), "·".repeat(9))
        );
        assert_eq!(lines[1], "     0     6     12    18");
        assert!(hours_lines(&[0.0; 24]).is_empty());
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
        assert!(plan(std::slice::from_ref(&long), 2).is_empty());
        assert_eq!(plan(std::slice::from_ref(&long), 3).len(), 3);
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
        assert_eq!(
            plan(std::slice::from_ref(&odd), 10),
            vec![Row::First(&odd), Row::Rest(&odd)]
        );
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
    fn today_lines_fit_the_column_with_big_numbers() {
        let big = day(1_234_567, 7_654_321, 123, 99.0 * 3600.0);
        let lines = today_lines(&big, 1234.5, CONTENT);
        assert!(lines.iter().all(|l| cell_len(l) <= 1 + CONTENT));
        assert_eq!(lines.len(), 3);
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
