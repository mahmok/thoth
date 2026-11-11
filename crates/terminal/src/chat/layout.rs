use core::ops::Range;

use thoth_core::auth::credential::now_ms;
use thoth_core::transcript::{Content, Item, Transcript};

use super::entry::{Entry, Layout};
use super::row::{Row, wrap};
use super::tool_card;

/// A slot laid out as a message or a card.
enum Laid {
    Message(Entry),
    Tool(Option<Layout>),
}

/// One transcript item and its rows.
struct Slot {
    /// Whether a Tool card shows all of its output rather than the preview.
    expanded: bool,
    id: u64,
    laid: Laid,
    revision: Option<u64>,
}

impl Slot {
    /// Lays out the item. Returns whether anything changed.
    fn lay_out(&mut self, item: &Item, width: usize, now: u64) -> bool {
        match self.laid {
            Laid::Message(ref mut entry) => match item.content {
                Content::Message(ref message) => entry.lay_out(message, item.revision, width),
                Content::Tool(_) => kinds_disagree(),
            },
            Laid::Tool(ref mut layout) => match item.content {
                Content::Tool(ref call) => {
                    let stale = call.is_running()
                        || self.revision != Some(item.revision)
                        || layout.as_ref().is_none_or(|laid| laid.width != width);
                    if stale {
                        let mut rows = wrap(&tool_card::lines(call, now, self.expanded), width);
                        rows.push(Row::blank());
                        *layout = Some(Layout { rows, width });
                        self.revision = Some(item.revision);
                    }
                    stale
                },
                Content::Message(_) => kinds_disagree(),
            },
        }
    }

    fn new(item: &Item) -> Self {
        let laid = match item.content {
            Content::Message(_) => Laid::Message(Entry::new()),
            Content::Tool(_) => Laid::Tool(None),
        };
        Self { id: item.id, revision: None, laid, expanded: false }
    }

    fn rows(&self) -> &[Row] {
        match self.laid {
            Laid::Message(ref entry) => entry.rows(),
            Laid::Tool(ref tool) => tool.as_ref().map_or(&[], |laid| &laid.rows),
        }
    }
}

/// The transcript wrapped to a width, kept between frames so only what changed is laid out again.
#[derive(Default)]
pub(super) struct ChatLayout {
    slots: Vec<Slot>,
}

impl ChatLayout {
    /// The Tool card whose header is on `row`, if a card's header is.
    pub(super) fn card_at(&self, row: usize) -> Option<u64> {
        let mut start = 0usize;
        for slot in &self.slots {
            let end = start.saturating_add(slot.rows().len());
            if (start..end).contains(&row) {
                let is_card = matches!(slot.laid, Laid::Tool(_));
                return (is_card && row == start).then_some(slot.id);
            }
            start = end;
        }
        None
    }

    /// Returns how many items had to be laid out. Tool cards are laid out every call while
    /// they run, because their elapsed time keeps changing.
    pub(super) fn lay_out(&mut self, transcript: &Transcript, width: usize) -> usize {
        let items = transcript.items();
        let kept = self.slots.iter().zip(items).take_while(|&(slot, item)| slot.id == item.id).count();
        self.slots.truncate(kept);
        self.slots.extend(items.get(kept..).unwrap_or_default().iter().map(Slot::new));

        let now = now_ms();
        self.slots.iter_mut().zip(items).map(|(slot, item)| usize::from(slot.lay_out(item, width, now))).sum()
    }

    pub(super) fn row_count(&self) -> usize {
        self.slots.iter().map(|slot| slot.rows().len()).sum()
    }

    /// Rows past the end are left out.
    pub(super) fn rows(&self, range: Range<usize>) -> impl Iterator<Item = &Row> {
        // Whole items above the window are skipped by their row counts, not row by row.
        let mut before = 0usize;
        let mut first = self.slots.len();
        for (index, slot) in self.slots.iter().enumerate() {
            let rows = slot.rows().len();
            if before.saturating_add(rows) > range.start {
                first = index;
                break;
            }
            before = before.saturating_add(rows);
        }
        self.slots
            .get(first..)
            .unwrap_or_default()
            .iter()
            .flat_map(Slot::rows)
            .skip(range.start.saturating_sub(before))
            .take(range.len())
    }

    /// Opens or closes the Tool card `id`. A card with nothing beyond its preview has nothing to
    /// open, and is left as it is. Returns whether it changed.
    pub(super) fn toggle_card(&mut self, id: u64, transcript: &Transcript) -> bool {
        let has_more = transcript
            .items()
            .iter()
            .any(|item| item.id == id && matches!(&item.content, Content::Tool(call) if call.hidden_lines() > 0));
        let Some(slot) = self.slots.iter_mut().find(|slot| slot.id == id) else {
            return false;
        };
        if !slot.expanded && !has_more {
            return false;
        }
        slot.expanded = !slot.expanded;
        // Forces the card to be laid out again at its next look.
        slot.revision = None;
        true
    }
}

/// A slot is made for the kind of its item, so the two never disagree.
#[expect(clippy::unreachable, reason = "a slot is made for the kind of its item")]
fn kinds_disagree() -> ! {
    unreachable!("a slot is made for the kind of its item")
}

#[cfg(test)]
mod tests {
    use super::*;
    use thoth_core::agent::Event as AgentEvent;
    use thoth_core::agent::tools::tool::Outcome as ToolResult;
    use thoth_core::transcript::{Change as TranscriptChange, Speaker};

    /// Records what the Session would send for an agent event, the way a client applies it.
    fn record(transcript: &mut Transcript, event: AgentEvent) {
        let change = transcript.agent_event(event).expect("the event changes the transcript");
        transcript.apply(&change);
    }

    fn say(transcript: &mut Transcript, speaker: Speaker, text: impl Into<String>) {
        let change = transcript.said(speaker, text);
        transcript.apply(&change);
    }

    fn texts(layout: &mut ChatLayout, transcript: &Transcript, width: usize) -> Vec<String> {
        layout.lay_out(transcript, width);
        layout.rows(0..layout.row_count()).map(|r| r.text(0..usize::MAX)).collect()
    }

    #[test]
    fn a_window_of_rows_is_the_same_as_slicing_all_of_them() {
        let mut transcript = Transcript::default();
        for i in 0i32..6i32 {
            say(&mut transcript, Speaker::You, format!("message {i}\nwith\nthree lines"));
            record(&mut transcript, started(&format!("bash({i})")));
        }
        let mut layout = ChatLayout::default();
        layout.lay_out(&transcript, 20);
        let all: Vec<String> = layout.rows(0..layout.row_count()).map(|r| r.text(0..usize::MAX)).collect();
        for start in 0..all.len() + 2 {
            for len in [0, 1, 5, 100] {
                let window: Vec<String> = layout.rows(start..start + len).map(|r| r.text(0..usize::MAX)).collect();
                let end = (start + len).min(all.len());
                let expected = all.get(start..end).unwrap_or(&[]);
                assert_eq!(window, expected, "rows {start}..{}", start + len);
            }
        }
    }

    fn text(delta: &str) -> AgentEvent {
        AgentEvent::Text(delta.to_owned())
    }

    fn started(summary: &str) -> AgentEvent {
        AgentEvent::ToolStarted { id: summary.to_owned(), summary: summary.to_owned() }
    }

    fn finished(summary: &str, result: ToolResult) -> AgentEvent {
        AgentEvent::ToolFinished { id: summary.to_owned(), summary: summary.to_owned(), result }
    }

    #[test]
    fn messages_are_separated_by_a_blank_row() {
        let mut transcript = Transcript::default();
        say(&mut transcript, Speaker::You, "hi");
        say(&mut transcript, Speaker::Tool, "read_file(a)");
        assert_eq!(texts(&mut ChatLayout::default(), &transcript, 40), ["you: hi", "", "tool: read_file(a)", ""]);
    }

    #[test]
    fn a_streamed_reply_has_one_label() {
        let mut transcript = Transcript::default();
        say(&mut transcript, Speaker::You, "q");
        record(&mut transcript, text("Hel"));
        record(&mut transcript, text("lo"));
        let rows = texts(&mut ChatLayout::default(), &transcript, 40);
        assert!(rows.iter().any(|r| r == "Hello"), "{rows:?}");
        assert_eq!(rows.iter().filter(|r| *r == "agent:").count(), 1);
    }

    #[test]
    fn thinking_is_shown_without_a_label_before_the_reply() {
        let mut transcript = Transcript::default();
        record(&mut transcript, AgentEvent::Reasoning("Let me look.".into()));
        record(&mut transcript, text("Found it"));
        assert_eq!(texts(&mut ChatLayout::default(), &transcript, 40), ["Let me look.", "", "agent:", "Found it", ""]);
    }

    #[test]
    fn a_running_tool_call_shows_its_status() {
        let mut transcript = Transcript::default();
        record(&mut transcript, started("read_file(Cargo.toml)"));
        let rows = texts(&mut ChatLayout::default(), &transcript, 60);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows[0].contains("running"), "{rows:?}");
        assert!(rows[0].contains("read_file(Cargo.toml)"), "{rows:?}");
    }

    #[test]
    fn a_finished_tool_call_previews_its_output_on_the_same_card() {
        let mut transcript = Transcript::default();
        let mut layout = ChatLayout::default();
        record(&mut transcript, started("read_file(a)"));
        texts(&mut layout, &transcript, 60);
        record(&mut transcript, finished("read_file(a)", ToolResult::ok("file contents")));
        let rows = texts(&mut layout, &transcript, 60);
        assert!(rows[0].contains("done"), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("file contents")), "{rows:?}");
    }

    #[test]
    fn a_failed_tool_call_shows_the_error() {
        let mut transcript = Transcript::default();
        record(&mut transcript, started("read_file(../x)"));
        record(&mut transcript, finished("read_file(../x)", ToolResult::error("outside the working directory")));
        let rows = texts(&mut ChatLayout::default(), &transcript, 80);
        assert!(rows[0].contains("failed"), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("outside the working directory")), "{rows:?}");
    }

    #[test]
    fn rows_are_wrapped_to_the_width() {
        let mut transcript = Transcript::default();
        let mut layout = ChatLayout::default();
        say(&mut transcript, Speaker::You, "one two three four five six");
        layout.lay_out(&transcript, 10);
        assert!(layout.rows(0..usize::MAX).all(|r| r.width() <= 10));
        let narrow = layout.row_count();
        layout.lay_out(&transcript, 40);
        assert!(layout.row_count() < narrow);
    }

    #[test]
    fn layouts_are_reused_until_the_text_or_width_changes() {
        let mut transcript = Transcript::default();
        let mut layout = ChatLayout::default();
        say(&mut transcript, Speaker::You, "hi");
        record(&mut transcript, text("a"));

        assert_eq!(layout.lay_out(&transcript, 40), 2);
        assert_eq!(layout.lay_out(&transcript, 40), 0, "unchanged messages are not laid out again");

        record(&mut transcript, text("b"));
        assert_eq!(layout.lay_out(&transcript, 40), 1, "only the growing message is laid out again");

        assert_eq!(layout.lay_out(&transcript, 20), 2, "a new width lays everything out again");
    }

    #[test]
    fn a_cleared_transcript_is_laid_out_from_scratch() {
        let mut transcript = Transcript::default();
        let mut layout = ChatLayout::default();
        say(&mut transcript, Speaker::You, "old");
        layout.lay_out(&transcript, 40);
        transcript.apply(&TranscriptChange::Cleared);
        say(&mut transcript, Speaker::You, "new");
        assert_eq!(texts(&mut layout, &transcript, 40), ["you: new", ""]);
    }

    #[test]
    fn a_range_of_rows_is_read_without_gathering_the_rest() {
        let mut transcript = Transcript::default();
        let mut layout = ChatLayout::default();
        for i in 0i32..5i32 {
            say(&mut transcript, Speaker::You, format!("message {i}"));
        }
        layout.lay_out(&transcript, 40);
        assert_eq!(layout.row_count(), 10);
        let middle: Vec<String> = layout.rows(4..7).map(|r| r.text(0..usize::MAX)).collect();
        assert_eq!(middle, ["you: message 2", "", "you: message 3"]);
        assert_eq!(layout.rows(8..20).count(), 2, "rows past the end are left out");
    }

    #[test]
    fn a_card_opens_and_closes_and_only_cards_with_more_output_open() {
        let mut transcript = Transcript::default();
        record(&mut transcript, started("bash(many)"));
        record(&mut transcript, finished("bash(many)", ToolResult::ok("1\n2\n3\n4\n5")));
        record(&mut transcript, started("bash(short)"));
        record(&mut transcript, finished("bash(short)", ToolResult::ok("only")));
        let (many, short) = (transcript.items()[0].id, transcript.items()[1].id);

        let mut layout = ChatLayout::default();
        layout.lay_out(&transcript, 40);
        let collapsed = layout.row_count();
        assert_eq!(layout.card_at(0), Some(many), "the header is the first row");

        assert!(!layout.toggle_card(short, &transcript), "nothing to open");
        assert!(layout.toggle_card(many, &transcript));
        layout.lay_out(&transcript, 40);
        assert!(layout.row_count() > collapsed, "the rest of the output appears");

        assert!(layout.toggle_card(many, &transcript));
        layout.lay_out(&transcript, 40);
        assert_eq!(layout.row_count(), collapsed, "and closes again");
    }

    #[test]
    fn only_a_card_header_is_a_card_row() {
        let mut transcript = Transcript::default();
        record(&mut transcript, started("bash(many)"));
        record(&mut transcript, finished("bash(many)", ToolResult::ok("1\n2\n3\n4\n5")));
        say(&mut transcript, Speaker::Agent, "after");
        let card = transcript.items()[0].id;
        let mut layout = ChatLayout::default();
        layout.lay_out(&transcript, 40);
        assert_eq!(layout.card_at(0), Some(card));
        assert_eq!(layout.card_at(1), None, "the output is not the header");
        assert_eq!(layout.card_at(layout.row_count() - 1), None);
    }
}
