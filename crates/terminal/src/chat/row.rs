use core::iter;
use core::ops::Range;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar as _;

/// Columns a tab is drawn as.
const TAB_WIDTH: usize = 4;

/// One character with its style and width in columns. Only a working form: rows keep their
/// text as runs (see [`Run`]), which take a fraction of the memory.
#[derive(Debug, Clone)]
struct Cell {
    ch: char,
    style: Style,
    width: usize,
}

/// A stretch of text in one style.
#[derive(Debug, Clone)]
struct Run {
    style: Style,
    text: String,
}

#[derive(Debug, Clone, Default)]
/// A wrapped row of cells.
pub(super) struct Row {
    runs: Vec<Run>,
}

impl Row {
    pub(super) fn blank() -> Self {
        Self::default()
    }

    fn cells(&self) -> impl Iterator<Item = Cell> + '_ {
        self.runs
            .iter()
            .flat_map(|run| run.text.chars().map(|ch| Cell { ch, style: run.style, width: ch.width().unwrap_or(0) }))
    }

    fn from_cells(cells: &[Cell], trim_trailing_spaces: bool) -> Self {
        let end = if trim_trailing_spaces {
            cells.iter().rposition(|c| c.ch != ' ').map_or(0, |last| last.saturating_add(1))
        } else {
            cells.len()
        };
        let mut runs = Vec::new();
        for cell in cells.iter().take(end) {
            push_char(&mut runs, cell.ch, cell.style);
        }
        Self { runs }
    }

    pub(super) fn text(&self, columns: Range<usize>) -> String {
        let mut column = 0usize;
        let mut out = String::new();
        for cell in self.cells() {
            let end = column.saturating_add(cell.width.max(1));
            if column < columns.end && end > columns.start {
                out.push(cell.ch);
            }
            column = column.saturating_add(cell.width);
        }
        out
    }

    /// The row as a line, with the selected columns reversed.
    pub(super) fn to_line(&self, selected: Option<Range<usize>>) -> Line<'static> {
        let Some(range) = selected else {
            return Line::from(
                self.runs.iter().map(|run| Span::styled(run.text.clone(), run.style)).collect::<Vec<_>>(),
            );
        };
        let mut runs: Vec<Run> = Vec::new();
        let mut column = 0usize;
        for cell in self.cells() {
            let end = column.saturating_add(cell.width.max(1));
            let mut style = cell.style;
            if column < range.end && end > range.start {
                style = style.add_modifier(Modifier::REVERSED);
            }
            push_char(&mut runs, cell.ch, style);
            column = column.saturating_add(cell.width);
        }
        Line::from(runs.into_iter().map(|run| Span::styled(run.text, run.style)).collect::<Vec<_>>())
    }

    /// The `http(s)://` link under the given column, if there is one.
    pub(super) fn url_at(&self, column: usize) -> Option<String> {
        let cells: Vec<Cell> = self.cells().collect();
        let mut start = 0usize;
        let spans: Vec<(usize, usize)> = cells
            .iter()
            .map(|cell| {
                let span = (start, start.saturating_add(cell.width.max(1)));
                start = span.1;
                span
            })
            .collect();
        let hit = spans.iter().position(|&(from, to)| column >= from && column < to)?;
        if cells.get(hit)?.ch.is_whitespace() {
            return None;
        }
        let mut first = hit;
        while first > 0 && cells.get(first.saturating_sub(1)).is_some_and(|cell| !cell.ch.is_whitespace()) {
            first = first.saturating_sub(1);
        }
        let mut last = hit;
        while cells.get(last.saturating_add(1)).is_some_and(|cell| !cell.ch.is_whitespace()) {
            last = last.saturating_add(1);
        }
        let token: String = cells.get(first..=last)?.iter().map(|c| c.ch).collect();
        let unwrapped = token.trim_start_matches(['(', '[', '<', '"', '\'']);
        let url = unwrapped.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '>', '"', '\'']);
        let from = first.saturating_add(token.chars().count().saturating_sub(unwrapped.chars().count()));
        let inside = hit >= from && hit < from.saturating_add(url.chars().count());
        (inside && (url.starts_with("https://") || url.starts_with("http://"))).then(|| url.to_owned())
    }

    pub(super) fn width(&self) -> usize {
        self.cells().map(|c| c.width).sum()
    }
}

fn push_char(runs: &mut Vec<Run>, ch: char, style: Style) {
    match runs.last_mut() {
        Some(run) if run.style == style => run.text.push(ch),
        _ => runs.push(Run { style, text: ch.to_string() }),
    }
}

pub(super) fn wrap(lines: &[Line<'_>], width: usize) -> Vec<Row> {
    lines.iter().flat_map(|line| wrap_cells(&cells_of(line), width)).collect()
}

fn cells_of(line: &Line<'_>) -> Vec<Cell> {
    let mut cells = Vec::new();
    for span in &line.spans {
        let style = line.style.patch(span.style);
        for ch in span.content.chars() {
            if ch == '\t' {
                cells.extend(iter::repeat_n(Cell { ch: ' ', style, width: 1 }, TAB_WIDTH));
            } else if let Some(width) = ch.width() {
                cells.push(Cell { ch, style, width });
            } else {
                // A control character has no width and is not drawn.
            }
        }
    }
    cells
}

/// Breaks cells into rows of at most `max_width` columns, at spaces where it can.
fn wrap_cells(cells: &[Cell], max_width: usize) -> Vec<Row> {
    let width = max_width.max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    let mut column = 0usize;
    // Index just after the last space in the current row: a place to break.
    let mut last_break: Option<usize> = None;

    for (index, cell) in cells.iter().enumerate() {
        let cell_width = cell.width;
        if column.saturating_add(cell_width) > width && column > 0 {
            let split = last_break.filter(|&b| b > start).unwrap_or(index);
            rows.push(Row::from_cells(cells.get(start..split).unwrap_or_default(), true));
            start = split;
            column = cells.get(start..index).unwrap_or_default().iter().map(|c| c.width).sum();
            last_break = None;
        }
        column = column.saturating_add(cell_width);
        if cell.ch == ' ' {
            last_break = Some(index.saturating_add(1));
        }
    }
    rows.push(Row::from_cells(cells.get(start..).unwrap_or_default(), false));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn rows(text: &str, width: usize) -> Vec<String> {
        wrap(&[Line::from(text.to_owned())], width).iter().map(|r| r.text(0..usize::MAX)).collect()
    }

    #[test]
    fn short_lines_stay_on_one_row() {
        assert_eq!(rows("hello", 10), ["hello"]);
    }

    #[test]
    fn breaks_after_spaces() {
        assert_eq!(rows("the quick brown fox", 10), ["the quick", "brown fox"]);
    }

    #[test]
    fn long_words_are_split_across_rows() {
        assert_eq!(rows("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn a_long_word_after_text_moves_down_whole_when_it_fits() {
        assert_eq!(rows("hi abcdefgh", 8), ["hi", "abcdefgh"]);
    }

    #[test]
    fn empty_lines_keep_a_row() {
        assert_eq!(rows("", 10), [""]);
        let two = wrap(&[Line::from("a"), Line::from(""), Line::from("b")], 10);
        assert_eq!(two.len(), 3);
    }

    #[test]
    fn wide_characters_count_as_two_columns() {
        let wrapped = wrap(&[Line::from("\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}")], 6);
        assert_eq!(wrapped.len(), 2);
        assert!(wrapped.iter().all(|r| r.width() <= 6));
    }

    #[test]
    fn rows_never_exceed_the_width() {
        for width in 1..12 {
            for row in wrap(&[Line::from("some words, and a_very_long_unbroken_token here")], width) {
                assert!(row.width() <= width.max(1) || row.cells().count() == 1, "width {width}");
            }
        }
    }

    #[test]
    fn styles_survive_wrapping() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![Span::raw("plain "), Span::styled("redword", red)]);
        let wrapped = wrap(&[line], 8);
        assert_eq!(wrapped.len(), 2);
        let second = wrapped[1].to_line(None);
        assert_eq!(second.spans[0].style, red);
        assert_eq!(second.spans[0].content, "redword");
    }

    #[test]
    fn line_style_applies_to_its_spans() {
        let line = Line::from("x").style(Style::new().fg(Color::Green));
        let wrapped = wrap(&[line], 5);
        assert_eq!(wrapped[0].to_line(None).spans[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn selected_cells_are_highlighted() {
        let row = &wrap(&[Line::from("abcd")], 10)[0];
        let line = row.to_line(Some(1..3));
        let highlighted: String = line
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(highlighted, "bc");
    }

    #[test]
    fn text_returns_the_characters_in_a_column_range() {
        let row = &wrap(&[Line::from("abcdef")], 10)[0];
        assert_eq!(row.text(2..4), "cd");
        assert_eq!(row.text(0..usize::MAX), "abcdef");
    }

    #[test]
    fn a_link_is_found_under_any_of_its_columns() {
        let row = &wrap(&[Line::from("open https://github.com/login/device now")], 80)[0];
        assert_eq!(row.url_at(5).as_deref(), Some("https://github.com/login/device"));
        assert_eq!(row.url_at(20).as_deref(), Some("https://github.com/login/device"));
        assert_eq!(row.url_at(35).as_deref(), Some("https://github.com/login/device"));
        assert_eq!(row.url_at(0), None);
        assert_eq!(row.url_at(36), None, "the space after the link");
        assert_eq!(row.url_at(37), None);
        assert_eq!(row.url_at(500), None);
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_link() {
        let row = &wrap(&[Line::from("see (https://x.test/a).")], 80)[0];
        assert_eq!(row.url_at(8).as_deref(), Some("https://x.test/a"));
        assert_eq!(row.url_at(row.width() - 1), None);
    }

    #[test]
    fn only_http_links_count() {
        let row = &wrap(&[Line::from("file:///etc/passwd ftp://x.test")], 80)[0];
        assert_eq!(row.url_at(3), None);
        assert_eq!(row.url_at(25), None);
    }

    #[test]
    fn text_in_one_style_is_stored_as_one_run() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![Span::raw("plain "), Span::raw("text "), Span::styled("red", red)]);
        let row = &wrap(&[line], 80)[0];
        assert_eq!(row.runs.len(), 2, "one run per style, not one cell per character");
    }

    #[test]
    fn tabs_become_spaces() {
        assert_eq!(rows("a\tb", 20), ["a    b"]);
    }
}
