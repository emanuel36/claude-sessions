//! The statistics blocks the sidebar and the landing page share: usage, today and the week's
//! spend, narrow at the bottom of the sidebar and wide on the landing page.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde::Deserialize;

use crate::claude::statusline::Quota;
use crate::ui::line::{
    cell_len, finish, ljust, push, rjust, start, truncate, width_of, BULLET, ELBOW, ELBOW_CELLS,
    INDENT,
};
use crate::ui::usage::{Day, Totals};
use crate::ui::words::{
    self, elbow, Bullet, BAR, CLAUDE, ERROR, INACTIVE, INK, STRONG, SUBTLE, SUCCESS,
};

/// The landing page's wide statistics: their width.
pub const PANEL_WIDTH: usize = 56;
/// The room inside the landing page's padding.
pub const PANEL_CONTENT: usize = PANEL_WIDTH - 2;
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

/// A `⎿` line with the lines added and removed, cut to the content.
fn changed(added: u64, removed: u64, content: usize) -> Line<'static> {
    let mut line = elbow(true, Style::new());
    push(&mut line, format!("+{}", words::thousands(added)), SUCCESS);
    push(&mut line, format!(" −{}", words::thousands(removed)), ERROR);
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

/// Usage, Today (with `today`) and `last`, a blank line between blocks; each line is padded to
/// `content + 2` cells.
fn blocks(
    stats: &Stats,
    now: f64,
    content: usize,
    today: bool,
    last: Vec<Line<'static>>,
) -> Vec<Line<'static>> {
    let mut blocks = vec![usage_lines(&stats.limits, now, content)];
    if let Some(day) = stats.day.as_ref().filter(|_| today) {
        blocks.push(today_lines(day, stats.today, content));
    }
    blocks.push(last);
    spaced(blocks)
        .into_iter()
        .map(|l| finish(l, content))
        .collect()
}

/// The statistics at the bottom, under a blank line and with one between blocks: Usage,
/// Today and Spend 7d.
///
/// # Arguments
/// * `stats`: what to show.
/// * `now`: current time as a Unix timestamp.
/// * `content`: columns available; each line is padded to `content + 2` cells.
/// * `today`: whether to show the Today block; it goes first when room is short.
pub fn stats_lines(stats: &Stats, now: f64, content: usize, today: bool) -> Vec<Line<'static>> {
    let blank = finish(start(Style::new()), content);
    let chart = chart_lines(&stats.week, content);
    std::iter::once(blank)
        .chain(blocks(stats, now, content, today, chart))
        .collect()
}

/// The landing page's wide statistics, a blank line between blocks: Usage, Today, and the
/// last 7 days in one block, by model only with `models`. Each line is padded to
/// `content + 2` cells.
pub fn panel_lines(stats: &Stats, now: f64, content: usize, models: bool) -> Vec<Line<'static>> {
    blocks(
        stats,
        now,
        content,
        true,
        week_lines(stats, content, models),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use chrono::{Days, NaiveDate};
    use ratatui::text::Line;

    use crate::claude::statusline::Quota;
    use crate::ui::line::{cell_len, plain};
    use crate::ui::stats::{
        hbar, hours_lines, panel_lines, stats_lines, today_lines, Stats, PANEL_WIDTH,
    };
    use crate::ui::usage::{Day, Totals};

    /// 2026-11-03 14:07:00 UTC, a Tuesday; a multiple of 5 s, so the spinner is at frame 0.
    pub(crate) const NOW: f64 = 1_793_714_820.0;
    /// The sidebar's room inside its padding.
    use crate::ui::sidebar::CONTENT;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 3).unwrap()
    }

    pub(crate) fn limits() -> std::collections::BTreeMap<String, Quota> {
        [
            ("five_hour", 58.0, NOW + 2.0 * 3600.0 + 33.0 * 60.0),
            // 2026-11-06 09:00
            ("seven_day", 31.0, NOW + 240_780.0),
        ]
        .into_iter()
        .map(|(k, pct, resets_at)| (k.to_owned(), Quota { pct, resets_at }))
        .collect()
    }

    pub(crate) fn week() -> Vec<(NaiveDate, f64)> {
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

    pub(crate) fn stats() -> Stats {
        Stats {
            limits: limits(),
            today: 9.59,
            week: week(),
            ..Stats::default()
        }
    }

    /// The content columns of each line, without the padding or trailing spaces.
    pub(crate) fn body(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                let text: String = plain(l).chars().skip(1).take(CONTENT).collect();
                text.trim_end().to_owned()
            })
            .collect()
    }

    pub(crate) fn day(added: u64, removed: u64, sessions: u32, waited: f64) -> Day {
        Day {
            added,
            removed,
            sessions,
            waited,
        }
    }

    pub(crate) fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_week_with_no_spend_draws_empty_rows() {
        let empty = Stats {
            week: week().into_iter().map(|(d, _)| (d, 0.0)).collect(),
            ..Stats::default()
        };
        let b = body(&stats_lines(&empty, NOW, CONTENT, true));
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
        let b = body(&stats_lines(&stats, NOW, CONTENT, true));
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
    fn today_lines_fit_the_column_with_big_numbers() {
        let big = day(1_234_567, 7_654_321, 123, 99.0 * 3600.0);
        let lines = today_lines(&big, 1234.5, CONTENT);
        assert!(lines.iter().all(|l| cell_len(l) <= 1 + CONTENT));
        assert_eq!(lines.len(), 3);
    }

    /// A heading of the landing page's week block, after `lead`.
    fn rule(lead: &str, title: &str) -> String {
        let rest = PANEL_WIDTH - 2 - 9 - title.len();
        format!("{lead}── {title} {}", "─".repeat(rest))
    }
}
