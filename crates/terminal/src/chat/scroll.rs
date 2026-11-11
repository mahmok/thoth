#[derive(Debug, Default)]
/// Scroll position of the chat.
pub(super) struct Scroll {
    from_bottom: usize,
    max: usize,
}

impl Scroll {
    pub(super) const fn down(&mut self, rows: usize) {
        self.from_bottom = self.from_bottom.saturating_sub(rows);
    }

    pub(super) fn up(&mut self, rows: usize) {
        self.from_bottom = self.from_bottom.saturating_add(rows).min(self.max);
    }

    pub(super) fn update(&mut self, total_rows: usize, height: usize) -> usize {
        self.max = total_rows.saturating_sub(height);
        self.from_bottom = self.from_bottom.min(self.max);
        self.max.saturating_sub(self.from_bottom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_pinned_to_the_bottom() {
        let mut scroll = Scroll::default();
        assert_eq!(scroll.update(100, 10), 90);
    }

    #[test]
    fn short_content_has_nothing_to_scroll() {
        let mut scroll = Scroll::default();
        assert_eq!(scroll.update(5, 10), 0);
        scroll.up(3);
        assert_eq!(scroll.update(5, 10), 0);
    }

    #[test]
    fn scrolling_up_and_down_moves_the_top() {
        let mut scroll = Scroll::default();
        scroll.update(100, 10);
        scroll.up(5);
        assert_eq!(scroll.update(100, 10), 85);
        scroll.down(2);
        assert_eq!(scroll.update(100, 10), 87);
    }

    #[test]
    fn position_is_clamped_to_the_content() {
        let mut scroll = Scroll::default();
        scroll.update(100, 10);
        scroll.up(10_000);
        assert_eq!(scroll.update(100, 10), 0);
        scroll.down(usize::MAX);
        assert_eq!(scroll.update(100, 10), 90);
    }

    #[test]
    fn an_unpinned_view_stays_put_while_content_grows() {
        let mut scroll = Scroll::default();
        scroll.update(100, 10);
        scroll.up(20);
        let before = scroll.update(100, 10);
        let after = scroll.update(110, 10);
        // The distance from the bottom is kept, so the top moves down with the new rows.
        assert_eq!(after, before + 10);
    }
}
