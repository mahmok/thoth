use super::row::Row;

/// Rows laid out from a revision at a width.
///
/// Laying out is expensive, so the rows are kept and only built again when the revision or the
/// width changes. The caller writes the rows while it lays them out, then calls [`commit`].
///
/// [`commit`]: Self::commit
#[derive(Default)]
pub(super) struct Cache {
    pub rows: Vec<Row>,
    revision: Option<u64>,
    width: usize,
}

impl Cache {
    /// Whether the rows were laid out from `revision` at `width`.
    pub(super) fn holds(&self, revision: u64, width: usize) -> bool {
        self.revision == Some(revision) && self.width == width
    }

    /// Starts a layout at `width`, clearing the rows if the width changed. Returns whether it
    /// changed, so that anything derived from the old width can be dropped as well.
    pub(super) fn set_width(&mut self, width: usize) -> bool {
        let changed = self.width != width;
        if changed {
            self.rows.clear();
            self.width = width;
        }
        changed
    }

    /// Marks the rows as the layout of `revision`.
    pub(super) const fn commit(&mut self, revision: u64) {
        self.revision = Some(revision);
    }

    /// Keeps `rows` as the layout of `revision` at `width`.
    pub(super) fn replace(&mut self, revision: u64, width: usize, rows: Vec<Row>) {
        self.rows = rows;
        self.width = width;
        self.revision = Some(revision);
    }

    /// Forgets the revision, so the next look lays the rows out again.
    pub(super) const fn invalidate(&mut self) {
        self.revision = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_until_the_revision_or_the_width_changes() {
        let mut cache = Cache::default();
        assert!(!cache.holds(1, 40), "nothing is laid out yet");
        let _resized = cache.set_width(40);
        cache.commit(1);
        assert!(cache.holds(1, 40));
        assert!(!cache.holds(2, 40), "a new revision is not laid out yet");
        assert!(!cache.holds(1, 20), "a new width is not laid out yet");
    }

    #[test]
    fn an_invalidated_cache_holds_nothing() {
        let mut cache = Cache::default();
        let _resized = cache.set_width(40);
        cache.commit(1);
        cache.invalidate();
        assert!(!cache.holds(1, 40));
    }

    #[test]
    fn a_new_width_clears_the_rows_and_says_so() {
        let mut cache = Cache::default();
        assert!(cache.set_width(40), "the first width has nothing to keep");
        cache.rows.push(Row::blank());
        cache.commit(1);
        assert!(!cache.set_width(40), "the same width keeps the rows");
        assert_eq!(cache.rows.len(), 1);
        assert!(cache.set_width(20), "a new width clears them");
        assert!(cache.rows.is_empty());
    }

    #[test]
    fn replacing_the_rows_records_their_revision_and_width() {
        let mut cache = Cache::default();
        cache.replace(1, 40, vec![Row::blank()]);
        assert!(cache.holds(1, 40));
        assert!(!cache.holds(2, 40));
        assert!(!cache.holds(1, 20));
    }
}
