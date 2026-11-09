//! Words, numbers, bars and styles the sidebar and the landing page share.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::ui::line::cells;

/// The shortest delay a redraw timer is given (see [`next_tick`]).
pub const MIN_TICK: f64 = 0.05;
/// While a session works the screen redraws `FPS` times a second, a spinner frame each time.
pub const FPS: u32 = 8;
/// Fixed English names, not the locale's, so the screens read the same everywhere.
pub const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
/// Fixed English month names.
pub const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
/// The three rows of Claude's mascot.
pub const MASCOT_ROWS: [&str; 3] = [" ▐▛███▜▌", "▝▜█████▛▘", "  ▘▘ ▝▝  "];

/// Quota bars turn yellow from this percentage.
const AMBER: f64 = 70.0;
/// Quota bars turn red from this percentage.
const RED: f64 = 85.0;

// Colours by name: ANSI numbers, so they follow the terminal palette.

/// Plain text in the terminal's own colours.
pub const INK: Style = Style::new();
/// ANSI green.
pub const GREEN: Style = Style::new().fg(Color::Green);
/// ANSI blue.
pub const BLUE: Style = Style::new().fg(Color::Blue);
/// ANSI magenta.
pub const MAGENTA: Style = Style::new().fg(Color::Magenta);
/// Palette grey 244.
pub const SOFT: Style = Style::new().fg(Color::Indexed(244));
/// Palette grey 240.
pub const FAINT: Style = Style::new().fg(Color::Indexed(240));
/// Palette grey 238, for rules.
pub const RULE: Style = Style::new().fg(Color::Indexed(238));
/// Bright white, bold.
pub const STRONG: Style = Style::new().fg(Color::White).add_modifier(Modifier::BOLD);

// Claude Code's own dark-theme colours, so the sidebar reads as part of it.

/// Claude's orange.
pub const CLAUDE: Style = Style::new().fg(Color::Rgb(0xd7, 0x77, 0x57));
/// Success green.
pub const SUCCESS: Style = Style::new().fg(Color::Rgb(0x4e, 0xba, 0x65));
/// Error red.
pub const ERROR: Style = Style::new().fg(Color::Rgb(0xff, 0x6b, 0x80));
/// Warning yellow.
pub const WARNING: Style = Style::new().fg(Color::Rgb(0xff, 0xc1, 0x07));
/// Permission lavender.
pub const PERMISSION: Style = Style::new().fg(Color::Rgb(0xb1, 0xb9, 0xf9));
/// Inactive grey.
pub const INACTIVE: Style = Style::new().fg(Color::Rgb(0x99, 0x99, 0x99));
/// Subtle dark grey, for empty bar cells.
pub const SUBTLE: Style = Style::new().fg(Color::Rgb(0x50, 0x50, 0x50));
/// The mascot's colour: Claude's orange.
pub const MASCOT: Style = CLAUDE;
/// The background of the current session's row.
pub const CURRENT: Style = Style::new().bg(Color::Indexed(235));

/// Python's `x // y` on floats: the floor of the exact quotient, as CPython computes it.
pub(crate) fn floor_div(x: f64, y: f64) -> f64 {
    let rem = x % y;
    let mut div = (x - rem) / y;
    if rem != 0.0 && ((y < 0.0) != (rem < 0.0)) {
        div -= 1.0;
    }
    if div == 0.0 {
        return 0.0_f64.copysign(x / y);
    }
    let floor = div.floor();
    if div - floor > 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// Says a length of time the way the sidebar shows it.
///
/// Returns `40m` under an hour, else hours and minutes as `1h12`; empty when `seconds` is
/// `None` (unknown).
pub fn span(seconds: Option<f64>) -> String {
    let Some(seconds) = seconds else {
        return String::new();
    };
    let minutes = floor_div(seconds, 60.0) as i64;
    if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h{:02}", minutes / 60, minutes % 60)
    }
}

/// How long until a moment, in five cells at most: `40m`, `2h33` or `3d4h`.
pub fn remaining(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    if seconds < 86400.0 {
        return span(Some(seconds));
    }
    let days = floor_div(seconds, 86400.0) as i64;
    let hours = floor_div(seconds % 86400.0, 3600.0) as i64;
    if hours > 0 {
        format!("{days}d{hours}h")
    } else {
        format!("{days}d")
    }
}

/// Seconds until the next redraw: the next whole second, or the next frame when animating.
///
/// A delay shorter than [`MIN_TICK`] would be dropped by a one-shot timer that has already
/// passed it by the time it starts, which ends a redraw loop for good; it waits for the step
/// after instead. A `per_second` of 0 counts as 1.
pub fn next_tick(now: f64, per_second: u32) -> f64 {
    let step = 1.0 / f64::from(per_second.max(1));
    let delay = step - now.rem_euclid(step);
    if delay >= MIN_TICK {
        delay
    } else {
        delay + step
    }
}

/// How long a session has waited, as the sidebar shows it; empty under a minute.
pub fn waiting_label(seconds: f64) -> String {
    if seconds < 60.0 {
        String::new()
    } else if seconds < 3600.0 {
        format!("{}m", floor_div(seconds, 60.0) as i64)
    } else {
        format!("{}h", floor_div(seconds, 3600.0) as i64)
    }
}

/// An amount in dollars, as `$4.12`; empty when unknown.
pub fn money(amount: Option<f64>) -> String {
    amount.map_or_else(String::new, |amount| format!("${amount:.2}"))
}

/// Cuts text to a number of terminal columns, marking a cut with an ellipsis.
///
/// Runs of whitespace collapse to one space first. Text that fits is returned whole; a width
/// of 0 gives an empty string.
pub fn fit(text: &str, width: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().map(cells).sum::<usize>() <= width {
        return text;
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        if used + cells(c) > width - 1 {
            break;
        }
        out.push(c);
        used += cells(c);
    }
    out.push('…');
    out
}

/// A bar cell: a line through the middle of the cell, level with the text, so stacked bars
/// have a gap between them.
pub const BAR: &str = "━";

/// The style of a quota bar's filled cells: Claude's orange, then yellow, then red.
pub fn bar_style(pct: f64) -> Style {
    if pct >= RED {
        ERROR
    } else if pct >= AMBER {
        WARNING
    } else {
        CLAUDE
    }
}

/// A share of 100 as a bar of `cells` [`BAR`] cells, coloured by level, empty in subtle.
///
/// The filled part rounds half to even, as Python's `round` does. A part with no cells is left
/// out, so the bar has at most two spans.
pub fn bar(pct: f64, cells: usize) -> Vec<Span<'static>> {
    let filled = (pct * cells as f64 / 100.0)
        .round_ties_even()
        .clamp(0.0, cells as f64) as usize;
    [(filled, bar_style(pct)), (cells - filled, SUBTLE)]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, style)| Span::styled(BAR.repeat(count), style))
        .collect()
}

/// How long ago, in four cells at most: `5m`, `3h`, `12d` or `40w`.
pub fn age(timestamp: f64, now: f64) -> String {
    let seconds = (now - timestamp).max(0.0);
    if seconds < 3600.0 {
        format!("{}m", (floor_div(seconds, 60.0) as i64).max(1))
    } else if seconds < 86400.0 {
        format!("{}h", floor_div(seconds, 3600.0) as i64)
    } else if seconds < 100.0 * 86400.0 {
        format!("{}d", floor_div(seconds, 86400.0) as i64)
    } else {
        format!("{}w", floor_div(seconds, 7.0 * 86400.0) as i64)
    }
}

/// An integer with a comma between thousands, as `4,210`.
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Modifier, Style};

    use crate::ui::line::cells;
    use crate::ui::words::{
        age, bar, bar_style, fit, floor_div, money, next_tick, remaining, span, thousands,
        waiting_label, CLAUDE, CURRENT, ERROR, MASCOT, STRONG, SUBTLE, WARNING,
    };

    fn near(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn bar_fills_by_rounding_and_colours_by_level() {
        let b = bar(58.0, 14);
        let text: String = b.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "━".repeat(14));
        assert_eq!(b[0].content.chars().count(), 8);
        assert_eq!(b[0].style, CLAUDE);
        assert_eq!(b[1].style, SUBTLE);
        assert_eq!(bar_style(69.0), CLAUDE);
        assert_eq!(bar_style(70.0), WARNING);
        assert_eq!(bar_style(85.0), ERROR);
        assert_eq!(bar(31.0, 16)[0].content.chars().count(), 5);
    }

    #[test]
    fn bar_rounds_half_to_even_like_python() {
        // round(2.5) == 2 and round(3.5) == 4 in Python
        assert_eq!(bar(50.0, 5)[0].content.chars().count(), 2);
        assert_eq!(bar(70.0, 5)[0].content.chars().count(), 4);
    }

    #[test]
    fn bar_clamps_and_leaves_out_empty_spans() {
        assert_eq!(bar(-20.0, 4).len(), 1);
        assert_eq!(bar(-20.0, 4)[0].style, SUBTLE);
        let full = bar(250.0, 4);
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].content, "━━━━");
        assert_eq!(full[0].style, ERROR);
        assert!(bar(50.0, 0).is_empty());
    }

    #[test]
    fn floor_div_floors_negative_quotients_like_python() {
        // -1.0 // 60.0 == -1.0 and -7.5 // 2.0 == -4.0 in Python
        assert_eq!(floor_div(-1.0, 60.0), -1.0);
        assert_eq!(floor_div(-120.0, 60.0), -2.0);
        assert_eq!(floor_div(-7.5, 2.0), -4.0);
        assert_eq!(floor_div(7.5, 2.0), 3.0);
        assert_eq!(floor_div(3599.9, 60.0), 59.0);
    }

    #[test]
    fn thousands_uses_a_comma() {
        assert_eq!(thousands(4210), "4,210");
        assert_eq!(thousands(23), "23");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(-1000), "-1,000");
    }

    #[test]
    fn moved_helpers() {
        assert_eq!(fit("abcdefghij", 5), "abcd…");
        assert_eq!(money(Some(4.1)), "$4.10");
        assert_eq!(money(None), "");
        assert_eq!(waiting_label(30.0), "");
        assert_eq!(waiting_label(125.0), "2m");
    }

    #[test]
    fn money_rounds_the_exact_value_half_to_even_like_python() {
        // f"{0.125:.2f}" == "0.12" in Python
        assert_eq!(money(Some(0.125)), "$0.12");
        assert_eq!(money(Some(1234.5)), "$1234.50");
    }

    #[test]
    fn waiting_label_switches_to_hours() {
        assert_eq!(waiting_label(3599.0), "59m");
        assert_eq!(waiting_label(7300.0), "2h");
    }

    #[test]
    fn next_tick_waits_for_the_next_second_but_never_almost_nothing() {
        assert!(near(next_tick(10.25, 1), 0.75, 1e-9));
        // a delay of a few microseconds would end the redraw loop, so it waits for the second
        // after
        assert!(near(next_tick(10.999_999_9, 1), 1.000_000_1, 1e-6));
        assert_eq!(next_tick(10.0, 1), 1.0);
    }

    #[test]
    fn next_tick_can_wake_on_the_next_frame() {
        assert!(near(next_tick(10.03, 8), 0.095, 1e-9));
        assert_eq!(next_tick(10.0, 8), 0.125);
    }

    #[test]
    fn span_is_minutes_then_hours_and_minutes() {
        assert_eq!(span(Some(59.0)), "0m");
        assert_eq!(span(Some(2400.0)), "40m");
        assert_eq!(span(Some(4320.0)), "1h12");
        assert_eq!(span(Some(10800.0)), "3h00");
        assert_eq!(span(None), "");
    }

    #[test]
    fn remaining_is_minutes_hours_then_days() {
        assert_eq!(remaining(-5.0), "0m");
        assert_eq!(remaining(2400.0), "40m");
        assert_eq!(remaining(9180.0), "2h33");
        assert_eq!(remaining(86400.0 * 3.0 + 4.0 * 3600.0 + 59.0), "3d4h");
        assert_eq!(remaining(86400.0 * 2.0 + 60.0), "2d");
    }

    #[test]
    fn age_picks_the_largest_unit_that_fits() {
        let now = 1_000_000.0;
        assert_eq!(age(now + 10.0, now), "1m");
        assert_eq!(age(now - 10.0, now), "1m");
        assert_eq!(age(now - 300.0, now), "5m");
        assert_eq!(age(now - 3.0 * 3600.0, now), "3h");
        assert_eq!(age(now - 12.0 * 86400.0, now), "12d");
        assert_eq!(age(now - 99.0 * 86400.0, now), "99d");
        assert_eq!(age(now - 280.0 * 86400.0, now), "40w");
    }

    #[test]
    fn fit_cuts_wide_chars_by_cells() {
        // 2 + 2 cells, then the ellipsis
        assert_eq!(fit("日本語テキスト", 6), "日本…");
    }

    proptest::proptest! {
        #[test]
        fn fit_never_exceeds_the_width(text in "[a-z日本é \\t]{0,30}", width in 0usize..20) {
            let fitted = fit(&text, width);
            proptest::prop_assert!(fitted.chars().map(cells).sum::<usize>() <= width);
        }
    }

    #[test]
    fn fit_zero_width_is_empty() {
        assert_eq!(fit("abc", 0), "");
    }

    #[test]
    fn fit_collapses_whitespace_and_keeps_what_fits() {
        assert_eq!(fit("  a \n b\t c ", 10), "a b c");
        assert_eq!(fit("abcde", 5), "abcde");
        assert_eq!(fit("", 3), "");
    }

    #[test]
    fn styles_are_the_python_ones() {
        assert_eq!(
            STRONG,
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD)
        );
        assert_eq!(CLAUDE, Style::new().fg(Color::Rgb(0xd7, 0x77, 0x57)));
        assert_eq!(MASCOT, CLAUDE);
        assert_eq!(CURRENT, Style::new().bg(Color::Indexed(235)));
    }
}
