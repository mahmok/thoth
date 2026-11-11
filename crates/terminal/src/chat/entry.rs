use thoth_core::transcript::Message;

use super::markdown;
use super::message::{is_markdown, label_line, lines};
use super::row::{Row, wrap};

/// Rows wrapped to one width.
pub(super) struct Layout {
    pub rows: Vec<Row>,
    pub width: usize,
}

/// The front of a markdown message's layout that later text cannot change.
#[derive(Default)]
struct Frozen {
    /// Bytes of the message text that are laid out in the frozen rows.
    bytes: usize,
    /// How many of those rows are the label.
    label_rows: usize,
    /// Rows at the front of the layout: the label and then the finished blocks.
    rows: usize,
}

/// The rows of one message.
///
/// A reply streams in a few characters at a time, and rendering markdown is slow. So a markdown
/// message is laid out block by block: blocks that are complete are rendered once and kept,
/// and only the unfinished one at the end is rendered again as text arrives.
pub(super) struct Entry {
    frozen: Frozen,
    layout: Layout,
    revision: Option<u64>,
}

impl Entry {
    /// Brings the rows up to date for `width`. Returns whether there was anything to do.
    pub(super) fn lay_out(&mut self, message: &Message, revision: u64, width: usize) -> bool {
        if self.revision == Some(revision) && self.layout.width == width {
            return false;
        }
        if self.layout.width != width {
            self.layout.rows.clear();
            self.frozen = Frozen::default();
            self.layout.width = width;
        }
        if is_markdown(message) {
            self.lay_out_markdown(message, width);
        } else {
            self.layout.rows = wrap(&lines(message, width), width);
        }
        self.layout.rows.push(Row::blank());
        self.revision = Some(revision);
        true
    }

    /// Lays out a markdown message, keeping the rows of blocks that are complete.
    fn lay_out_markdown(&mut self, message: &Message, width: usize) {
        let rows = &mut self.layout.rows;
        let frozen = &mut self.frozen;
        if frozen.rows == 0 {
            *rows = wrap(&[label_line(message)], width);
            frozen.rows = rows.len();
            frozen.label_rows = rows.len();
        }
        // Drop the unfinished part and the blank row after it; both are laid out again.
        rows.truncate(frozen.rows);

        let pending = message.text.get(frozen.bytes..).unwrap_or_default();
        let (complete, unfinished) = pending.split_at(markdown::complete_blocks_len(pending));
        append_blocks(rows, complete, width, frozen.label_rows);
        frozen.bytes = frozen.bytes.saturating_add(complete.len());
        frozen.rows = rows.len();
        append_blocks(rows, unfinished, width, frozen.label_rows);
    }

    pub(super) fn new() -> Self {
        Self { layout: Layout { width: 0, rows: Vec::new() }, frozen: Frozen::default(), revision: None }
    }

    pub(super) fn rows(&self) -> &[Row] {
        &self.layout.rows
    }
}

/// Adds the rows of markdown `text` below those already there, after the `label_rows` of the
/// label. The renderer puts one blank line between blocks; rendering blocks apart leaves it to us.
fn append_blocks(rows: &mut Vec<Row>, text: &str, width: usize, label_rows: usize) {
    let lines = markdown::render(text, width);
    if lines.is_empty() {
        return;
    }
    if rows.len() > label_rows {
        rows.push(Row::blank());
    }
    rows.extend(wrap(&lines, width));
}

#[cfg(test)]
mod tests {
    use super::*;
    use thoth_core::transcript::Speaker;

    const WIDTH: usize = 40;

    const DOCUMENT: &str = "## Heading\n\nSome **bold** text with `code`.\nA second line.\n\n```rust\nfn main() {\n\n    let x = 1;\n}\n```\n\n- one\n- two\n\n  continued item\n\n1. first\n2. second\n\n> quote\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n    indented code\n\nTail paragraph.\n";

    struct Streamed {
        entry: Entry,
        message: Message,
        revision: u64,
    }

    impl Streamed {
        fn lay_out(&mut self, width: usize) -> bool {
            self.entry.lay_out(&self.message, self.revision, width)
        }

        fn push_text(&mut self, delta: &str) {
            self.message.text.push_str(delta);
            self.revision = self.revision.saturating_add(1);
        }
    }

    fn entry(speaker: Speaker, text: &str) -> Streamed {
        Streamed { message: Message { speaker, text: text.into() }, revision: 0, entry: Entry::new() }
    }

    fn shown(entry: &Streamed) -> Vec<String> {
        entry.entry.rows().iter().map(|row| row.text(0..usize::MAX)).collect()
    }

    /// What a message looks like laid out in one go, with nothing kept from before.
    fn fresh(speaker: Speaker, text: &str) -> Vec<String> {
        let mut entry = entry(speaker, text);
        entry.lay_out(WIDTH);
        shown(&entry)
    }

    #[test]
    fn a_reply_streamed_in_pieces_looks_like_the_same_reply_laid_out_whole() {
        let mut streamed = entry(Speaker::Agent, "");
        let mut so_far = String::new();
        for chunk in DOCUMENT.chars().collect::<Vec<_>>().chunks(7) {
            let piece: String = chunk.iter().collect();
            streamed.push_text(&piece);
            so_far.push_str(&piece);
            streamed.lay_out(WIDTH);
            assert_eq!(shown(&streamed), fresh(Speaker::Agent, &so_far), "after {so_far:?}");
        }
    }

    #[test]
    fn finished_blocks_are_frozen_and_the_last_one_is_not() {
        let mut entry = entry(Speaker::Agent, "first paragraph\n\nsecond");
        entry.lay_out(WIDTH);
        assert_eq!(entry.entry.frozen.bytes, 0, "nothing follows the blank line yet");

        entry.push_text(" paragraph\n\nthird");
        entry.lay_out(WIDTH);
        assert_eq!(entry.entry.frozen.bytes, "first paragraph\n\n".len(), "`third` is still arriving");

        entry.push_text("\n");
        entry.lay_out(WIDTH);
        assert_eq!(entry.entry.frozen.bytes, "first paragraph\n\nsecond paragraph\n\n".len());
        assert_eq!(shown(&entry), ["agent:", "first paragraph", "", "second paragraph", "", "third", ""]);
    }

    #[test]
    fn a_new_width_lays_everything_out_again() {
        let mut entry = entry(Speaker::Agent, DOCUMENT);
        entry.lay_out(WIDTH);
        assert!(entry.lay_out(20));
        let mut narrow = self::entry(Speaker::Agent, DOCUMENT);
        narrow.lay_out(20);
        assert_eq!(shown(&entry), shown(&narrow));
    }

    #[test]
    fn an_unchanged_entry_is_not_laid_out_again() {
        let mut entry = entry(Speaker::You, "hi");
        assert!(entry.lay_out(WIDTH));
        assert!(!entry.lay_out(WIDTH));
        entry.push_text("!");
        assert!(entry.lay_out(WIDTH));
    }

    #[test]
    fn plain_messages_are_followed_by_a_blank_row() {
        assert_eq!(fresh(Speaker::You, "hi"), ["you: hi", ""]);
    }
}
