use core::ops::Range;

/// A row and a column.
pub type Point = (usize, usize);

/// A row of text that can be selected from, counted in columns.
pub trait RowText {
    /// The row's width in columns.
    fn width(&self) -> usize;

    /// The text in `columns`, which is within the row.
    fn text(&self, columns: Range<usize>) -> String;
}

impl RowText for str {
    fn width(&self) -> usize {
        self.chars().count()
    }

    fn text(&self, columns: Range<usize>) -> String {
        self.chars().skip(columns.start).take(columns.len()).collect()
    }
}

/// The columns of row `index` that the selection from `lo` to `hi` covers, clamped to the
/// row's `width`. `hi.1` is the last selected column.
pub fn columns(lo: Point, hi: Point, index: usize, width: usize) -> Option<Range<usize>> {
    if index < lo.0 || index > hi.0 {
        return None;
    }
    let start = if index == lo.0 { lo.1 } else { 0 };
    let end = if index == hi.0 { hi.1.saturating_add(1) } else { width };
    let (from, to) = (start.min(width), end.min(width));
    (from < to).then_some(from..to)
}

/// The selected text of `rows`, which start at row `lo.0`: each row's selected columns,
/// clamped to the row, joined with newlines.
pub fn extract<'a, R: RowText + ?Sized + 'a>(lo: Point, hi: Point, rows: impl Iterator<Item = &'a R>) -> String {
    rows.enumerate()
        .map(|(offset, row)| {
            columns(lo, hi, lo.0.saturating_add(offset), row.width())
                .map_or_else(String::new, |columns| row.text(columns))
        })
        .collect::<Vec<_>>()
        .join("\n")
}
