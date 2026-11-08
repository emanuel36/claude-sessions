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
