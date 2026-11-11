use core::ops::Range;

use super::row::Row;

/// A row and a column.
pub(super) type Point = (usize, usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A selection of text in the chat.
pub(super) struct Selection {
    anchor: Point,
    head: Point,
}

impl Selection {
    pub(super) const fn at(point: Point) -> Self {
        Self { anchor: point, head: point }
    }

    pub(super) fn columns(&self, index: usize, row_width: usize) -> Option<Range<usize>> {
        let (lo, hi) = self.ordered();
        if index < lo.0 || index > hi.0 {
            return None;
        }
        let start = if index == lo.0 { lo.1 } else { 0 };
        let end = if index == hi.0 { hi.1.saturating_add(1) } else { row_width };
        let (from, to) = (start.min(row_width), end.min(row_width));
        (from < to).then_some(from..to)
    }

    pub(super) const fn extend_to(&mut self, head: Point) {
        self.head = head;
    }

    /// The selected text. `rows` are the rows of [`span`](Self::span), in order; any the chat
    /// does not have are left out.
    pub(super) fn extract<'rows>(&self, rows: impl Iterator<Item = &'rows Row>) -> String {
        let first = self.span().start;
        rows.enumerate()
            .map(|(offset, row)| {
                self.columns(first.saturating_add(offset), row.width())
                    .map(|columns| row.text(columns))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(super) fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    fn ordered(&self) -> (Point, Point) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }

    /// The rows the selection touches.
    pub(super) fn span(&self) -> Range<usize> {
        let (lo, hi) = self.ordered();
        lo.0..hi.0.saturating_add(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::row::wrap;
    use core::iter;
    use ratatui::text::Line;

    fn rows(lines: &[&str]) -> Vec<Row> {
        wrap(&lines.iter().map(|l| Line::from(l.to_string())).collect::<Vec<_>>(), 80)
    }

    fn select(anchor: Point, head: Point) -> Selection {
        let mut selection = Selection::at(anchor);
        selection.extend_to(head);
        selection
    }

    #[test]
    fn a_click_is_an_empty_selection() {
        assert!(Selection::at((1, 2)).is_empty());
        assert!(!select((1, 2), (1, 3)).is_empty());
    }

    #[test]
    fn selects_within_one_row_including_the_end_character() {
        let rows = rows(&["hello world"]);
        assert_eq!(select((0, 0), (0, 4)).extract(rows.iter()), "hello");
    }

    #[test]
    fn the_direction_of_the_drag_does_not_matter() {
        let rows = rows(&["hello world"]);
        assert_eq!(select((0, 4), (0, 0)).extract(rows.iter()), "hello");
    }

    #[test]
    fn spans_several_rows() {
        let rows = rows(&["first line", "second line", "third line"]);
        assert_eq!(select((0, 6), (2, 4)).extract(rows.iter()), "line\nsecond line\nthird");
    }

    #[test]
    fn columns_are_clamped_to_the_row() {
        let rows = rows(&["abc"]);
        assert_eq!(select((0, 1), (0, 50)).extract(rows.iter()), "bc");
        assert_eq!(select((0, 1), (0, 50)).columns(0, 3), Some(1..3));
        assert_eq!(select((0, 5), (0, 9)).columns(0, 3), None);
    }

    #[test]
    fn rows_outside_the_selection_have_no_columns() {
        let selection = select((1, 0), (2, 3));
        assert_eq!(selection.columns(0, 10), None);
        assert_eq!(selection.columns(3, 10), None);
        assert_eq!(selection.columns(1, 10), Some(0..10));
    }

    #[test]
    fn extracting_from_nothing_is_empty() {
        assert_eq!(select((0, 0), (5, 5)).extract(iter::empty()), "");
    }

    #[test]
    fn rows_past_the_end_are_ignored() {
        let rows = rows(&["only"]);
        assert_eq!(select((0, 0), (9, 9)).extract(rows.iter()), "only");
    }
}
