//! The golden harness: the neutral form the Rust screens are compared in.

mod support;

use std::collections::HashMap;
use std::path::Path;

use cst::ui::landing;
use cst::ui::picker::{self, Folder, Picker};
use cst::ui::sidebar::{column, Entry, Run, Stats};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;
use serde_json::{json, Value};

use support::golden::runs;

/// The fields of a `sidebar` fixture (see `tests/golden/fixtures.json`).
#[derive(Deserialize)]
struct SidebarFixture {
    now: f64,
    entries: Vec<Entry>,
    current: Option<String>,
    stats: Stats,
    height: u16,
    runs: Vec<Run>,
}

/// What the user did in the folder list, as `landing` and `panel` fixtures hold it.
#[derive(Deserialize)]
struct ListFixture {
    folders: Vec<Folder>,
    query: String,
    cursor: usize,
    expanded: Vec<String>,
    scroll: HashMap<String, usize>,
    /// The typed paths that are folders.
    dirs: Vec<String>,
}

impl ListFixture {
    /// A picker holding the folders and the user's state, as the goldens were rendered with.
    fn picker(self) -> Picker {
        let dirs = self.dirs;
        let mut p = Picker::new(Box::new(move |path| dirs.iter().any(|d| d == path)));
        p.folders = self.folders;
        p.query = self.query;
        p.cursor = self.cursor;
        p.expanded = self.expanded.into_iter().collect();
        p.scroll = self.scroll;
        p
    }
}

/// The fields of a `landing` fixture (see `tests/golden/fixtures.json`).
#[derive(Deserialize)]
struct LandingFixture {
    now: f64,
    #[serde(flatten)]
    list: ListFixture,
    searching: bool,
    opened: usize,
    stats: Stats,
    width: u16,
    height: u16,
}

/// The fields of a `panel` fixture (see `tests/golden/fixtures.json`).
#[derive(Deserialize)]
struct PanelFixture {
    now: f64,
    #[serde(flatten)]
    list: ListFixture,
    /// The popup's inner width; Python's popup was 50 columns wide, one spare.
    #[serde(default = "panel_width")]
    width: u16,
    height: u16,
}

fn panel_width() -> u16 {
    picker::WIDTH as u16 + 1
}

/// Renders a fixture as the Rust screens draw it; `None` for an unknown kind.
fn render(fixture: &Value) -> Option<Vec<Line<'static>>> {
    match fixture["kind"].as_str() {
        Some("sidebar") => {
            let f: SidebarFixture =
                serde_json::from_value(fixture.clone()).expect("sidebar fixture");
            let current = f.current.as_deref().unwrap_or("");
            Some(column(
                &f.entries, current, &f.stats, f.now, f.height, &f.runs,
            ))
        }
        Some("landing") => {
            let f: LandingFixture =
                serde_json::from_value(fixture.clone()).expect("landing fixture");
            let mut p = f.list.picker();
            Some(landing::screen(
                &mut p,
                f.searching,
                f.opened,
                &f.stats,
                f.now,
                f.width,
                f.height,
            ))
        }
        Some("panel") => {
            let f: PanelFixture = serde_json::from_value(fixture.clone()).expect("panel fixture");
            let mut p = f.list.picker();
            Some(picker::screen(&mut p, f.now, f.width, f.height))
        }
        _ => None,
    }
}

#[test]
fn screens_match_python() {
    // Before anything reads local time: the goldens were made in UTC
    std::env::set_var("TZ", "UTC");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let text = std::fs::read_to_string(dir.join("fixtures.json")).expect("read fixtures");
    let fixtures: Vec<Value> = serde_json::from_str(&text).expect("parse fixtures");
    let mut checked = 0;
    for fixture in &fixtures {
        let name = fixture["name"].as_str().expect("fixture name");
        let Some(lines) = render(fixture) else {
            continue;
        };
        let path = dir.join("expected").join(format!("{name}.json"));
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("read expected"))
                .expect("parse expected");
        let expected = expected.as_array().expect("expected lines");
        let actual = runs(&lines);
        let actual = actual.as_array().expect("actual lines");
        for (i, (want, got)) in expected.iter().zip(actual).enumerate() {
            assert_eq!(
                got, want,
                "{name}: line {i} differs\nexpected {want}\nactual   {got}"
            );
        }
        assert_eq!(actual.len(), expected.len(), "{name}: line count");
        checked += 1;
    }
    // Every fixture is compared: 7 sidebar, 5 landing and 4 panel
    assert_eq!(checked, fixtures.len(), "fixtures checked");
    assert_eq!(checked, 16, "fixtures checked");
}

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
