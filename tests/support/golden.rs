//! The neutral form the screen goldens are written in (see `tools/golden.py`).
//!
//! A screen is an array of lines; a line an array of runs `{"text", "fg", "bg", "bold"}`, with
//! adjacent runs of equal style merged and empty runs dropped. A colour is `default`, `idx:N`
//! (the 16 named colours by ANSI index, and `Indexed(N)`) or `rgb:#rrggbb`.

use ratatui::style::{Color, Modifier};
use ratatui::text::Line;
use serde_json::{json, Value};

/// Lines as runs of text with a foreground, a background and bold.
///
/// Each span's style is laid over its line's own style, as ratatui draws it.
///
/// # Panics
///
/// On any modifier other than bold and on an underline colour, which the goldens do not carry.
pub fn runs(lines: &[Line]) -> Value {
    Value::Array(lines.iter().map(line_runs).collect())
}

fn line_runs(line: &Line) -> Value {
    // (text, (fg, bg, bold)) per run
    let mut out: Vec<(String, (String, String, bool))> = Vec::new();
    for span in line.spans.iter().filter(|span| !span.content.is_empty()) {
        let style = line.style.patch(span.style);
        // The Python generator refuses attributes the goldens do not carry; so does this
        let other = (style.add_modifier | style.sub_modifier) - Modifier::BOLD;
        assert!(
            other.is_empty(),
            "golden runs only carry bold, not {other:?}"
        );
        assert!(
            style.underline_color.is_none(),
            "golden runs carry no underline colour, got {:?}",
            style.underline_color
        );
        let key = (
            colour(style.fg),
            colour(style.bg),
            style.add_modifier.contains(Modifier::BOLD),
        );
        match out.last_mut() {
            Some((text, last)) if *last == key => text.push_str(&span.content),
            _ => out.push((span.content.to_string(), key)),
        }
    }
    out.into_iter()
        .map(|(text, (fg, bg, bold))| json!({"text": text, "fg": fg, "bg": bg, "bold": bold}))
        .collect()
}

fn colour(color: Option<Color>) -> String {
    let index = match color {
        None | Some(Color::Reset) => return "default".into(),
        Some(Color::Rgb(r, g, b)) => return format!("rgb:#{r:02x}{g:02x}{b:02x}"),
        Some(Color::Indexed(n)) => n,
        Some(Color::Black) => 0,
        Some(Color::Red) => 1,
        Some(Color::Green) => 2,
        Some(Color::Yellow) => 3,
        Some(Color::Blue) => 4,
        Some(Color::Magenta) => 5,
        Some(Color::Cyan) => 6,
        Some(Color::Gray) => 7,
        Some(Color::DarkGray) => 8,
        Some(Color::LightRed) => 9,
        Some(Color::LightGreen) => 10,
        Some(Color::LightYellow) => 11,
        Some(Color::LightBlue) => 12,
        Some(Color::LightMagenta) => 13,
        Some(Color::LightCyan) => 14,
        Some(Color::White) => 15,
    };
    format!("idx:{index}")
}
