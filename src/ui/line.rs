//! Styled lines measured, padded and cut by terminal cells.
//!
//! These do for ratatui lines what `rich.text.Text` does for the Python screens: the same
//! calls, counted in cells (wide characters take two, combining marks none).

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// How many terminal cells a character takes: two when wide, none when combining.
pub fn cells(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// How many terminal cells a line takes.
pub fn cell_len(line: &Line) -> usize {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .map(cells)
        .sum()
}

/// Pads a line with unstyled spaces on the right until it is `cells` wide.
///
/// A line already that wide or wider is left as it is.
pub fn pad_right(line: &mut Line, cells: usize) {
    let missing = cells.saturating_sub(cell_len(line));
    if missing > 0 {
        line.spans.push(Span::raw(" ".repeat(missing)));
    }
}

/// Cuts a line to `cells` terminal cells, keeping the style of what is left.
///
/// A wide character that would straddle the cut is replaced by a space, so a line cut from
/// more than `cells` always ends up exactly `cells` wide. A line within `cells` is left as is.
pub fn truncate(line: &mut Line, cells: usize) {
    if cell_len(line) <= cells {
        return;
    }
    let mut used = 0;
    let mut kept = Vec::new();
    for span in line.spans.drain(..) {
        let mut text = String::new();
        let mut cut = false;
        for c in span.content.chars() {
            let width = self::cells(c);
            if used + width > cells {
                // only a wide character can land here with room left: fill the gap
                if used < cells {
                    text.push(' ');
                    used = cells;
                }
                cut = true;
                break;
            }
            text.push(c);
            used += width;
        }
        if !text.is_empty() {
            kept.push(Span::styled(text, span.style));
        }
        if cut {
            break;
        }
    }
    line.spans = kept;
}

/// A line of `(text, style)` parts, leaving out parts with no text.
pub fn styled(parts: &[(&str, Style)]) -> Line<'static> {
    parts
        .iter()
        .filter(|(text, _)| !text.is_empty())
        .map(|(text, style)| Span::styled((*text).to_owned(), *style))
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};

    use crate::ui::line::{cell_len, cells, pad_right, styled, truncate};

    fn plain(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn cells_counts_wide_as_two_and_combining_as_none() {
        assert_eq!(cells('a'), 1);
        assert_eq!(cells('日'), 2);
        assert_eq!(cells('\u{301}'), 0);
        assert_eq!(cells('▄'), 1);
    }

    #[test]
    fn cell_len_sums_spans_by_cells() {
        let line = styled(&[("ab", Style::new()), ("日本", Style::new().fg(Color::Red))]);
        assert_eq!(cell_len(&line), 6);
        assert_eq!(cell_len(&Line::default()), 0);
    }

    #[test]
    fn pad_right_pads_to_the_width_unstyled() {
        let mut line = styled(&[("日", Style::new().fg(Color::Red))]);
        pad_right(&mut line, 5);
        assert_eq!(plain(&line), "日   ");
        assert_eq!(line.spans.last().map(|s| s.style), Some(Style::new()));
        assert_eq!(cell_len(&line), 5);
    }

    #[test]
    fn pad_right_leaves_a_line_that_is_wide_enough() {
        let mut line = styled(&[("abcd", Style::new())]);
        pad_right(&mut line, 3);
        pad_right(&mut line, 4);
        assert_eq!(plain(&line), "abcd");
        assert_eq!(line.spans.len(), 1);
    }

    #[test]
    fn truncate_cuts_across_spans_keeping_styles() {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let mut line = styled(&[("ab", Style::new()), ("cdef", bold), ("gh", Style::new())]);
        truncate(&mut line, 4);
        assert_eq!(plain(&line), "abcd");
        assert_eq!(line.spans, vec![Span::raw("ab"), Span::styled("cd", bold)]);
    }

    #[test]
    fn truncate_replaces_a_split_wide_char_with_a_space() {
        let red = Style::new().fg(Color::Red);
        let mut line = styled(&[("a", Style::new()), ("日本", red)]);
        truncate(&mut line, 4);
        assert_eq!(plain(&line), "a日 ");
        assert_eq!(cell_len(&line), 4);
        assert_eq!(line.spans.last().map(|s| s.style), Some(red));
    }

    #[test]
    fn truncate_leaves_a_line_within_the_width_and_empties_at_zero() {
        let mut line = styled(&[("日本", Style::new())]);
        truncate(&mut line, 4);
        assert_eq!(plain(&line), "日本");
        truncate(&mut line, 0);
        assert_eq!(cell_len(&line), 0);
    }

    proptest::proptest! {
        #[test]
        fn truncate_always_ends_exactly_at_the_width(text in "[a-z日本é ]{0,20}", cut in 0usize..30) {
            let mut line = styled(&[(&text, Style::new())]);
            let before = cell_len(&line);
            truncate(&mut line, cut);
            proptest::prop_assert_eq!(cell_len(&line), before.min(cut));
        }
    }

    #[test]
    fn styled_skips_empty_parts() {
        let line = styled(&[("", Style::new()), ("a", Style::new().fg(Color::Blue))]);
        assert_eq!(line.spans.len(), 1);
        assert_eq!(plain(&line), "a");
    }
}
