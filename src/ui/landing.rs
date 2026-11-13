//! The landing page's text: the folder list on the left, the statistics on the right.

use chrono::{DateTime, Datelike, Local};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::ui::line::{ljust, pad_right, push, styled};
use crate::ui::picker::{self, hints_line, list_lines, search_box, Picker, Row};
use crate::ui::sidebar::{self, panel_lines, stats_lines, Stats, PANEL_CONTENT, PANEL_WIDTH};
use crate::ui::words::{MASCOT, MASCOT_ROWS, MONTHS, SOFT, STRONG, WEEKDAYS};

/// The columns between the list and the statistics, which take the sidebar's width on the right.
pub const GAP: usize = 4;
/// The folder list grows with the terminal up to this, so ages stay near the names.
pub const LIST_MAX: usize = 72;

/// The cells each mascot row takes beside the name and the date.
const MASCOT_CELLS: usize = 10;
/// The header, a blank line, the search box and a blank line, above the list.
const TOP: usize = 8;

/// The mascot beside the name, today's date and how many sessions are open.
///
/// The date is in local time, as `"Wed 4 Nov"`; it is left empty for a time out of range.
pub fn header(opened: usize, now: f64) -> Vec<Line<'static>> {
    // As Python's `time.localtime`, which floors the seconds
    let date = DateTime::from_timestamp(now.floor() as i64, 0)
        .map(|t| {
            let t = t.with_timezone(&Local);
            let weekday = WEEKDAYS[t.weekday().num_days_from_monday() as usize];
            format!("{weekday} {} {}", t.day(), MONTHS[t.month0() as usize])
        })
        .unwrap_or_default();
    let plural = if opened == 1 { "" } else { "s" };
    let count = format!("{opened} session{plural} open");
    let mascot = MASCOT_ROWS.map(|row| format!(" {}", ljust(row, MASCOT_CELLS)));
    vec![
        styled(&[(&mascot[0], MASCOT), ("claude-sessions", STRONG)]),
        styled(&[(&mascot[1], MASCOT), (&format!("{date} · {count}"), SOFT)]),
        styled(&[(&mascot[2], MASCOT)]),
    ]
}

/// What the keys do on the landing page: those of the selected row, as in the Alt+n panel,
/// then `/` and `q`, which only this page takes, then the keys that open the entries, last as
/// a line too long loses them first. Esc here only clears a search, so the panel's
/// `Esc to cancel` is left out.
///
/// # Arguments
/// * `row`: the selected row, if any.
/// * `picker`: what is open, to tell whether → opens or ← closes.
/// * `searching`: whether characters type into the search, so the entries take Ctrl+n and
///   Ctrl+o rather than `n` and `o`.
pub fn footer(
    row: Option<&Row>,
    picker: &Picker,
    searching: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut keys = picker::hints(row, picker);
    keys.retain(|&(key, _)| key != "Esc");
    keys.extend([("/", "search"), ("q", "quit")]);
    let entries = if searching {
        picker::ENTRY_KEYS
    } else {
        ["n", "o"]
    };
    keys.extend(picker::entry_hints(entries, picker));
    keys
}

/// What the right of the page shows, and its width: the wide statistics, the sidebar's, or
/// nothing.
///
/// The wide statistics also need the height: cut short, they would lose whole blocks, so the
/// spend by model, the last, goes first.
fn right_side(stats: &Stats, now: f64, width: usize, height: usize) -> (Vec<Line<'static>>, usize) {
    // The list keeps at least the Alt+n panel's width beside the statistics
    let fits = |stats_width: usize| width >= picker::WIDTH + GAP + stats_width;
    let room = height.saturating_sub(2);
    if fits(PANEL_WIDTH) {
        let mut panel = panel_lines(stats, now, PANEL_CONTENT, true);
        if panel.len() > room {
            panel = panel_lines(stats, now, PANEL_CONTENT, false);
        }
        if panel.len() <= room {
            return (panel, PANEL_WIDTH);
        }
    }
    if fits(sidebar::WIDTH) {
        return (stats_lines(stats, now, true), sidebar::WIDTH);
    }
    (Vec::new(), 0)
}

/// Appends `tail` to `line`, its own line style laid under its spans, as rich's
/// `Text.append_text` does.
fn append(line: &mut Line<'static>, tail: Line<'static>) {
    let base = tail.style;
    line.spans.extend(
        tail.spans
            .into_iter()
            .map(|span| Span::styled(span.content, base.patch(span.style))),
    );
}

/// The page as `height` lines of `width` columns.
///
/// The header, the search box and the folder list fill the left, up to [`LIST_MAX`] columns;
/// the wide statistics sit right after it, the two centred together. A terminal too narrow or
/// short for them gets the sidebar's statistics, and one too narrow for those the list alone.
///
/// The lines are those Python draws, even when one is wider than the screen (the header on a
/// narrow terminal): the terminal crops them. A screen under 2 lines keeps its first lines.
///
/// # Arguments
/// * `picker`: the folder list and what the user did in it; its cursor is kept in range.
/// * `searching`: whether the user is typing a search.
/// * `opened`: how many sessions are open.
/// * `stats`: the statistics to show.
/// * `now`: current time as a Unix timestamp.
/// * `width`, `height`: the terminal's size.
pub fn screen(
    picker: &mut Picker,
    searching: bool,
    opened: usize,
    stats: &Stats,
    now: f64,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let (width, height) = (usize::from(width), usize::from(height));
    let (right, right_width) = right_side(stats, now, width, height);
    let list_width = LIST_MAX.min(if right.is_empty() {
        width.saturating_sub(1)
    } else {
        width.saturating_sub(right_width + GAP)
    });
    let mut left = header(opened, now);
    left.push(Line::default());
    // Out of a search, the box tells how to start one
    let hint = if searching {
        picker::placeholder(picker)
    } else {
        "/ to search"
    };
    left.extend(search_box(&picker.query, hint, searching, list_width));
    left.push(Line::default());
    let room = height.saturating_sub(TOP + 2).max(1);
    let shown = picker.shown();
    left.extend(list_lines(picker, &shown, now, room, list_width));
    let rows = height.saturating_sub(2);
    left.resize(rows.max(left.len()), Line::default());
    // The list and the statistics sit together in the middle of the terminal
    let used = list_width
        + if right.is_empty() {
            0
        } else {
            GAP + right_width
        };
    let margin = " ".repeat(width.saturating_sub(used) / 2);
    let indented = |line: Line<'static>| {
        let mut out = Line::default();
        push(&mut out, margin.clone(), Style::new());
        append(&mut out, line);
        out
    };
    let mut right = right.into_iter();
    let mut lines: Vec<Line<'static>> = left
        .into_iter()
        .take(rows)
        .map(|mut line| {
            if let Some(stats_line) = right.next() {
                pad_right(&mut line, list_width + GAP);
                append(&mut line, stats_line);
            }
            indented(line)
        })
        .collect();
    lines.push(Line::default());
    let keys_width = width.saturating_sub(1 + margin.len());
    let keys = footer(shown.get(picker.cursor), picker, searching);
    lines.push(indented(hints_line(&keys, keys_width)));
    lines.truncate(height);
    lines
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{Local, TimeZone};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::text::Line;
    use ratatui::widgets::{Paragraph, Widget};

    use crate::claude::statusline::Quota;
    use crate::ui::landing::{footer, header, screen, GAP, LIST_MAX};
    use crate::ui::line::{pad_right, plain, row_text, truncate, width_of};
    use crate::ui::picker::{self, hints_line, Folder, Key, Picker, Root, Session};
    use crate::ui::sidebar::{self, panel_lines, Stats, PANEL_CONTENT, PANEL_WIDTH};
    use crate::ui::usage::{Day, Totals};

    /// 2026-11-04 14:07 local time, a Wednesday.
    fn now() -> f64 {
        Local
            .with_ymd_and_hms(2026, 11, 4, 14, 7, 0)
            .single()
            .unwrap()
            .timestamp() as f64
    }

    fn folder(title: &str) -> Folder {
        let now = now();
        Folder {
            path: "/w/api".into(),
            name: "api".into(),
            last: now - 60.0,
            count: 3,
            spend: 12.5,
            sessions: vec![Session {
                id: "s1".into(),
                title: title.into(),
                last: now - 60.0,
                cost: Some(2.0),
            }],
        }
    }

    fn limits() -> BTreeMap<String, Quota> {
        let quota = Quota {
            pct: 11.0,
            resets_at: now() + 3600.0,
        };
        BTreeMap::from([("five_hour".to_owned(), quota)])
    }

    fn stats() -> Stats {
        Stats {
            limits: limits(),
            today: 9.5,
            day: Some(Day {
                sessions: 2,
                ..Day::default()
            }),
            ..Stats::default()
        }
    }

    fn picker() -> Picker {
        let mut p = Picker::new(Box::new(|_| false));
        p.folders = vec![folder("fix it")];
        p
    }

    /// Each line's text without its trailing spaces.
    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| plain(line).trim_end().to_owned())
            .collect()
    }

    /// A text from its `skip`-th character on, as Python's `text[skip:]`.
    fn from(text: &str, skip: usize) -> String {
        text.chars().skip(skip).collect()
    }

    #[test]
    fn header_names_the_day_and_the_open_sessions() {
        let lines = texts(&header(1, now()));
        assert!(lines[0].ends_with("claude-sessions"), "{}", lines[0]);
        assert!(
            lines[1].ends_with("Wed 4 Nov · 1 session open"),
            "{}",
            lines[1]
        );
        assert!(texts(&header(3, now()))[1].contains("3 sessions open"));
    }

    #[test]
    fn header_puts_the_mascot_in_ten_columns_after_a_space() {
        let lines = texts(&header(0, now()));
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "  ▐▛███▜▌  claude-sessions");
        assert_eq!(lines[1], " ▝▜█████▛▘ Wed 4 Nov · 0 sessions open");
        assert_eq!(lines[2], "   ▘▘ ▝▝");
    }

    #[test]
    fn the_landing_uses_the_search_box() {
        let box_of = |p: &mut Picker, searching| {
            let lines = texts(&screen(p, searching, 0, &stats(), now(), 60, 20));
            lines[3..8].to_vec()
        };
        let mut p = picker();
        let idle = box_of(&mut p, false);
        let border = "─".repeat(57);
        assert_eq!(idle[0], "");
        assert_eq!(idle[1], format!("╭{border}╮"));
        assert_eq!(idle[2], format!("│ ❯ / to search{}│", " ".repeat(43)));
        assert_eq!(idle[3], format!("╰{border}╯"));
        assert_eq!(idle[4], "");
        // While searching, nothing typed yet: the panel's placeholder after the cursor
        assert!(box_of(&mut p, true)[2].starts_with("│ ❯ ▌Search folders…"));
        p.query = "ap".into();
        assert!(box_of(&mut p, true)[2].starts_with("│ ❯ ap▌ "));
        // A search ended keeps its text, with no cursor
        assert!(box_of(&mut p, false)[2].starts_with("│ ❯ ap "));
    }

    #[test]
    fn the_landing_footer_adds_search_and_quit() {
        let mut p = picker();
        // The footer sits under the list, which the statistics' width pushes right
        let last = |p: &mut Picker| {
            let lines = texts(&screen(p, false, 0, &stats(), now(), 120, 20));
            lines[19].trim_start().to_owned()
        };
        assert_eq!(
            last(&mut p),
            "Enter to new session · → to sessions · / to search · q to quit · n to new folder · o \
             to open folder"
        );
        let row = p.selected();
        assert_eq!(
            footer(row.as_ref(), &p, false),
            [
                ("Enter", "new session"),
                ("→", "sessions"),
                ("/", "search"),
                ("q", "quit"),
                ("n", "new folder"),
                ("o", "open folder")
            ]
        );
        // While searching, n and o are typed: the entries take their Ctrl keys
        let keys = footer(row.as_ref(), &p, true);
        assert_eq!(
            keys[4..],
            [("Ctrl+n", "new folder"), ("Ctrl+o", "open folder")]
        );
        // On an entry, its own keys only
        p.key(Key::Entry(Root::New));
        assert_eq!(
            last(&mut p),
            "Enter/← to close · / to search · q to quit · n to close new folder · o to open folder"
        );
    }

    #[test]
    fn screen_has_the_list_on_the_left_and_the_stats_on_the_right() {
        let long_title = "a session title long enough to need a wide terminal to show it whole";
        let mut p = picker();
        p.folders = vec![Folder {
            sessions: vec![Session {
                last: now(),
                ..folder(long_title).sessions[0].clone()
            }],
            ..folder(long_title)
        }];
        p.key(Key::Right);
        let width = 140;
        // The list and the statistics together, centred
        let margin = (usize::from(width) - LIST_MAX - GAP - PANEL_WIDTH) / 2;
        let lines: Vec<String> = texts(&screen(&mut p, false, 2, &stats(), now(), width, 30))
            .iter()
            .map(|line| from(line, margin))
            .collect();
        assert_eq!(lines.len(), 30);
        assert!(lines[5].starts_with("│ ❯ / to search"), "{}", lines[5]);
        assert!(lines[8].starts_with("❯ ▾ api"), "{}", lines[8]);
        assert!(
            lines[9].starts_with("    3 sessions · $12.50 7d"),
            "{}",
            lines[9]
        );
        // The list stops at its widest, the statistics right after it
        assert_eq!(lines[10].chars().count(), LIST_MAX, "{}", lines[10]);
        assert!(lines[10].contains("a session title long enough"));
        let stats_at = LIST_MAX + GAP;
        assert!(from(&lines[0], stats_at).trim().starts_with("• Usage"));
        assert!(
            lines[29].starts_with(" Enter to new session · ← to close"),
            "{}",
            lines[29]
        );
    }

    #[test]
    fn a_narrow_screen_shows_the_list_alone() {
        let lines = texts(&screen(&mut picker(), false, 2, &stats(), now(), 60, 30));
        assert!(!lines.iter().any(|line| line.contains("Today")));
        assert!(lines[8].starts_with("❯ ▸ api"), "{}", lines[8]);
    }

    #[test]
    fn a_middling_screen_falls_back_to_the_sidebar_stats() {
        let width = picker::WIDTH + GAP + sidebar::WIDTH;
        let lines = texts(&screen(
            &mut picker(),
            false,
            2,
            &stats(),
            now(),
            width as u16,
            30,
        ));
        let right: Vec<String> = (lines.iter())
            .map(|l| from(l, picker::WIDTH + GAP).trim().to_owned())
            .collect();
        assert_eq!(right[1], "• Usage");
        assert!(right.contains(&"• Today               $9.50".to_owned()));
    }

    #[test]
    fn a_short_screen_falls_back_to_the_sidebar_stats() {
        let lines = texts(&screen(&mut picker(), false, 2, &stats(), now(), 160, 8));
        assert_eq!(lines.len(), 8);
        assert!(lines[1].ends_with("• Usage"), "{}", lines[1]);
    }

    #[test]
    fn a_footer_too_long_is_cut_with_an_ellipsis() {
        let line = hints_line(&footer(None, &picker(), false), 20);
        assert_eq!(plain(&line), " / to search · q to…");
    }

    #[test]
    fn a_screen_short_of_room_drops_the_spend_by_model_first() {
        let totals = Totals {
            by_project: vec![("api".into(), 2.0)],
            by_model: vec![("Opus".into(), 2.0), ("Haiku".into(), 1.0)],
            ..Totals::default()
        };
        let stats = Stats {
            totals: Some(totals),
            ..stats()
        };
        let full = panel_lines(&stats, now(), PANEL_CONTENT, true).len() as u16;
        let lines = texts(&screen(
            &mut picker(),
            false,
            2,
            &stats,
            now(),
            160,
            full + 1,
        ));
        assert!(!lines.iter().any(|line| line.contains("By model")));
        assert!(lines.iter().any(|line| line.contains("By folder")));
        let roomy = texts(&screen(
            &mut picker(),
            false,
            2,
            &stats,
            now(),
            160,
            full + 2,
        ));
        assert!(roomy.iter().any(|line| line.contains("By model")));
    }

    #[test]
    fn a_searching_screen_with_no_match_says_so() {
        let mut p = picker();
        p.query = "zzz".into();
        let lines = texts(&screen(&mut p, true, 0, &stats(), now(), 60, 14));
        assert!(lines[5].starts_with("│ ❯ zzz▌ "), "{}", lines[5]);
        assert_eq!(lines[8], "    nothing matches");
    }

    fn arb_folders() -> impl proptest::strategy::Strategy<Value = Vec<Folder>> {
        use proptest::prelude::*;
        let session = (
            "[a-z日本 ]{0,40}",
            0.0..90_000.0f64,
            proptest::option::of(0.0..50.0f64),
        );
        let folder = (
            "[a-z日本]{1,30}",
            0.0..900_000.0f64,
            0u32..60,
            0.0..200.0f64,
            proptest::collection::vec(session, 0..9),
        );
        proptest::collection::vec(folder, 0..8).prop_map(|folders| {
            let now = now();
            folders
                .into_iter()
                .enumerate()
                .map(|(i, (name, ago, count, spend, sessions))| Folder {
                    path: format!("/w/{i}/{name}"),
                    name,
                    last: now - ago,
                    count,
                    spend,
                    sessions: sessions
                        .into_iter()
                        .enumerate()
                        .map(|(j, (title, ago, cost))| Session {
                            id: format!("s{j}"),
                            title,
                            last: now - ago,
                            cost,
                        })
                        .collect(),
                })
                .collect()
        })
    }

    proptest::proptest! {
        #[test]
        fn screen_fits_any_size(
            width in 10u16..=250,
            height in 3u16..=80,
            folders in arb_folders(),
            query in "[a-z ]{0,3}",
            cursor in 0usize..30,
            open in proptest::collection::vec(proptest::bool::ANY, 8),
            searching in proptest::bool::ANY,
            opened in 0usize..30,
            with_day in proptest::bool::ANY,
            with_totals in proptest::bool::ANY,
        ) {
            let mut p = Picker::new(Box::new(|_| false));
            p.expanded = folders
                .iter()
                .zip(&open)
                .filter(|(_, open)| **open)
                .map(|(f, _)| f.path.clone())
                .collect();
            p.folders = folders;
            p.query = query;
            p.cursor = cursor;
            let totals = Totals {
                by_project: vec![("api".into(), 2.0), ("web".into(), 1.0)],
                by_model: vec![("Opus".into(), 2.0)],
                added: 1200,
                removed: 300,
                sessions: 4,
                ..Totals::default()
            };
            let stats = Stats {
                day: if with_day { stats().day } else { None },
                totals: with_totals.then_some(totals),
                hours: vec![600.0; 24],
                ..stats()
            };
            let lines = screen(&mut p, searching, opened, &stats, now(), width, height);
            proptest::prop_assert_eq!(lines.len(), usize::from(height));
            let area = Rect::new(0, 0, width, height);
            let mut buf = Buffer::empty(area);
            Paragraph::new(lines.clone()).render(area, &mut buf);
            // The terminal crops: each row shows its line cut, or padded, to the width
            for (y, line) in lines.iter().enumerate() {
                let row = row_text(&buf, y as u16);
                proptest::prop_assert!(width_of(&row) <= usize::from(width));
                let mut want = line.clone();
                truncate(&mut want, usize::from(width));
                pad_right(&mut want, usize::from(width));
                proptest::prop_assert_eq!(row, plain(&want));
            }
        }
    }
}
