//! The golden harness: the neutral form the Rust screens are compared in.

mod support;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::json;

use support::golden::runs;

#[test]
fn runs_merge_adjacent_equal_styles() {
    let white = Style::new().fg(Color::White);
    let orange = Style::new().fg(Color::Rgb(215, 119, 87));
    let merged = Line::from(vec![
        Span::styled("ab", white),
        Span::styled("", orange),
        Span::styled("cd", white),
        Span::styled("ef", orange),
    ]);
    // The line's own style lies under its spans: a span's colour wins, the line's fills in
    let based = Line::from(vec![
        Span::styled(
            "x",
            Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD),
        ),
        Span::styled("y", Style::new().fg(Color::Indexed(244))),
    ])
    .style(Style::new().bg(Color::Indexed(235)).fg(Color::Red));
    assert_eq!(
        runs(&[merged, based, Line::default()]),
        json!([
            [
                {"text": "abcd", "fg": "idx:15", "bg": "default", "bold": false},
                {"text": "ef", "fg": "rgb:#d77757", "bg": "default", "bold": false},
            ],
            [
                {"text": "x", "fg": "default", "bg": "idx:235", "bold": true},
                {"text": "y", "fg": "idx:244", "bg": "idx:235", "bold": false},
            ],
            [],
        ])
    );
}

#[test]
fn all_sixteen_named_colours_map_to_their_ansi_index() {
    let named = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    for (index, color) in named.into_iter().enumerate() {
        let line = Line::from(Span::styled("x", Style::new().fg(color).bg(color)));
        let expected = format!("idx:{index}");
        assert_eq!(
            runs(&[line]),
            json!([[{"text": "x", "fg": expected, "bg": expected, "bold": false}]]),
            "{color:?}"
        );
    }
}

#[test]
#[should_panic(expected = "ITALIC")]
fn runs_refuse_modifiers_other_than_bold() {
    let italic = Style::new().add_modifier(Modifier::BOLD | Modifier::ITALIC);
    runs(&[Line::from(Span::styled("x", italic))]);
}

#[test]
#[should_panic(expected = "DIM")]
fn runs_refuse_a_modifier_on_the_line_style() {
    let line = Line::from("x").style(Style::new().add_modifier(Modifier::DIM));
    runs(&[line]);
}

#[test]
#[should_panic(expected = "underline colour")]
fn runs_refuse_an_underline_colour() {
    let underlined = Style::new().underline_color(Color::Red);
    runs(&[Line::from(Span::styled("x", underlined))]);
}
