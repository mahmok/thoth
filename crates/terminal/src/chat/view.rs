use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use thoth_core::transcript::Transcript;

use super::layout::ChatLayout;
use super::scroll::Scroll;
use super::selection::{Point, Selection};
use crate::theme::{self, Theme};

/// Rows scrolled per wheel notch.
const WHEEL_STEP: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
/// What a click on the chat asks for.
pub enum ChatAction {
    Copy(String),
    OpenUrl(String),
}

#[derive(Default)]
/// The chat pane.
pub struct ChatView {
    area: Rect,
    dragging: bool,
    layout: ChatLayout,
    scroll: Scroll,
    selection: Option<Selection>,
    top: usize,
    width: usize,
}

impl ChatView {
    /// Handles the wheel, selection and clicks on the chat.
    pub fn handle_mouse(&mut self, ev: MouseEvent, transcript: &Transcript) -> Option<ChatAction> {
        let inside = inner(self.area).contains(Position::new(ev.column, ev.row));
        match ev.kind {
            MouseEventKind::ScrollUp if inside => self.scroll.up(WHEEL_STEP),
            MouseEventKind::ScrollDown if inside => self.scroll.down(WHEEL_STEP),
            MouseEventKind::Down(MouseButton::Left) if inside => {
                self.selection = Some(Selection::at(self.point_at(ev.column, ev.row)));
                self.dragging = true;
            },
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                let point = self.point_at(ev.column, ev.row);
                if let Some(selection) = self.selection.as_mut() {
                    selection.extend_to(point);
                }
            },
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                self.dragging = false;
                let selection = self.selection?;
                if selection.is_empty() {
                    self.selection = None;
                    let (row, column) = self.point_at(ev.column, ev.row);
                    let _laid_out = self.layout.lay_out(transcript, self.width.max(1));
                    if let Some(card) = self.layout.card_at(row) {
                        let _toggled = self.layout.toggle_card(card, transcript);
                        return None;
                    }
                    return self
                        .layout
                        .rows(row..row.saturating_add(1))
                        .next()
                        .and_then(|r| r.url_at(column))
                        .map(ChatAction::OpenUrl);
                }
                let _laid_out = self.layout.lay_out(transcript, self.width.max(1));
                return Some(ChatAction::Copy(selection.extract(self.layout.rows(selection.span()))));
            },
            MouseEventKind::Down(_)
            | MouseEventKind::Up(_)
            | MouseEventKind::Drag(_)
            | MouseEventKind::Moved
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {},
        }
        None
    }

    pub fn page_size(&self) -> usize {
        usize::from(inner(self.area).height.max(1))
    }

    fn point_at(&self, column: u16, row: u16) -> Point {
        let inner = inner(self.area);
        let x = column.clamp(inner.x, inner.right().saturating_sub(1).max(inner.x));
        let y = row.clamp(inner.y, inner.bottom().saturating_sub(1).max(inner.y));
        (self.top.saturating_add(usize::from(y.saturating_sub(inner.y))), usize::from(x.saturating_sub(inner.x)))
    }

    /// Draws the visible rows of the transcript.
    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect, transcript: &Transcript) {
        self.area = area;
        let inner = inner(area);
        let width = usize::from(inner.width.max(1));
        let height = usize::from(inner.height);

        let _laid_out = self.layout.lay_out(transcript, width);
        self.width = width;
        let top = self.scroll.update(self.layout.row_count(), height);
        self.top = top;

        let selection = self.selection;
        let lines: Vec<Line<'_>> = self
            .layout
            .rows(top..top.saturating_add(height))
            .enumerate()
            .map(|(offset, row)| {
                row.to_line(selection.and_then(|s| s.columns(top.saturating_add(offset), row.width())))
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).block(Theme::chat_pane()), area);

        let max = self.layout.row_count().saturating_sub(height);
        if max > 0 {
            let mut state = ScrollbarState::new(max).position(top).viewport_content_length(height);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None)
                    .track_symbol(None)
                    .thumb_symbol("\u{2503}")
                    .thumb_style(theme::current().muted()),
                area,
                &mut state,
            );
        }
    }

    /// Forgets the selection and jumps to the newest message, as after the transcript is cleared.
    pub const fn reset(&mut self) {
        self.selection = None;
        self.scroll.down(usize::MAX);
    }

    pub const fn scroll_down(&mut self, rows: usize) {
        self.scroll.down(rows);
    }

    pub fn scroll_up(&mut self, rows: usize) {
        self.scroll.up(rows);
    }
}

fn inner(area: Rect) -> Rect {
    Theme::chat_pane().inner(area)
}

#[cfg(test)]
mod tests {
    use thoth_core::agent::Event as AgentEvent;
    use thoth_core::transcript::Speaker;

    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use thoth_core::agent::tools::tool::Outcome as ToolResult;

    const WIDTH: u16 = 30;
    const HEIGHT: u16 = 8;

    #[derive(Default)]
    struct Chat {
        transcript: Transcript,
        view: ChatView,
    }

    impl Chat {
        fn apply(&mut self, event: AgentEvent) {
            if let Some(change) = self.transcript.agent_event(event) {
                self.transcript.apply(&change);
            }
        }

        fn handle_mouse(&mut self, ev: MouseEvent) -> Option<ChatAction> {
            self.view.handle_mouse(ev, &self.transcript)
        }

        fn push(&mut self, speaker: Speaker, text: impl Into<String>) {
            let change = self.transcript.said(speaker, text);
            self.transcript.apply(&change);
        }

        fn scroll_down(&mut self, rows: usize) {
            self.view.scroll_down(rows);
        }

        fn scroll_up(&mut self, rows: usize) {
            self.view.scroll_up(rows);
        }
    }

    fn draw(chat: &mut Chat) -> Vec<String> {
        draw_at(chat, HEIGHT)
    }

    fn draw_at(chat: &mut Chat, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, height)).unwrap();
        terminal.draw(|frame| chat.view.render(frame, frame.area(), &chat.transcript)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height).map(|y| (0..WIDTH).map(|x| buffer[(x, y)].symbol().to_owned()).collect::<String>()).collect()
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
    }

    fn screen(view: &mut Chat) -> String {
        draw(view).join("\n")
    }

    #[test]
    fn shows_messages_with_labels() {
        let mut view = Chat::default();
        view.push(Speaker::You, "hello");
        let shown = screen(&mut view);
        assert!(shown.contains("you: hello"), "{shown}");
    }

    #[test]
    fn renders_agent_markdown() {
        let mut view = Chat::default();
        view.push(Speaker::You, "hi");
        view.apply(AgentEvent::Text("Some **bold** text\n\n- one\n- two".into()));
        let shown = screen(&mut view);
        assert!(shown.contains("Some bold text"), "{shown}");
        assert!(!shown.contains("**"), "{shown}");
        assert!(shown.contains("one") && shown.contains("two"), "{shown}");
    }

    #[test]
    fn long_lines_wrap_inside_the_pane() {
        let mut view = Chat::default();
        view.push(Speaker::You, "alpha beta gamma delta epsilon zeta eta theta iota kappa");
        let lines = draw(&mut view);
        assert!(lines.iter().any(|l| l.contains("you: alpha beta gamma delta")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("kappa")), "{lines:?}");
    }

    #[test]
    fn stays_pinned_to_the_newest_messages() {
        let mut view = Chat::default();
        for i in 0i32..20i32 {
            view.push(Speaker::You, format!("message {i}"));
        }
        let shown = screen(&mut view);
        assert!(shown.contains("message 19") && !shown.contains("message 0"), "{shown}");
    }

    #[test]
    fn scrolling_up_reveals_older_messages() {
        let mut view = Chat::default();
        for i in 0i32..20i32 {
            view.push(Speaker::You, format!("message {i}"));
        }
        draw(&mut view);
        view.scroll_up(1000);
        let shown = screen(&mut view);
        assert!(shown.contains("message 0") && !shown.contains("message 19"), "{shown}");
        view.scroll_down(usize::MAX);
        assert!(screen(&mut view).contains("message 19"));
    }

    #[test]
    fn the_wheel_scrolls_inside_the_pane_only() {
        let mut view = Chat::default();
        for i in 0i32..20i32 {
            view.push(Speaker::You, format!("message {i}"));
        }
        draw(&mut view);
        view.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 0));
        assert!(!screen(&mut view).contains("message 0"), "the border is outside the pane");
        for _ in 0i32..20i32 {
            view.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 3));
        }
        assert!(screen(&mut view).contains("message 0"));
    }

    #[test]
    fn the_chat_leaves_a_row_of_air_under_the_header() {
        let mut view = Chat::default();
        view.push(Speaker::You, "hello");
        let rows = draw(&mut view);
        assert!(rows[0].trim().is_empty(), "{rows:?}");
        assert!(rows[1].contains("you: hello"), "{rows:?}");
    }

    #[test]
    fn dragging_selects_and_returns_the_text() {
        let mut view = Chat::default();
        view.push(Speaker::You, "hello world");
        draw(&mut view);
        // Inner area starts at (1, 1); "you: " takes 5 columns, so "hello" is columns 5..10.
        view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1 + 5, 1));
        view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 1 + 9, 1));
        let copied = view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 1 + 9, 1));
        assert_eq!(copied, Some(ChatAction::Copy("hello".into())));
    }

    #[test]
    fn a_plain_click_selects_nothing() {
        let mut view = Chat::default();
        view.push(Speaker::You, "hello");
        draw(&mut view);
        view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 1));
        let copied = view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 3, 1));
        assert_eq!(copied, None);
    }

    #[test]
    fn clicking_a_link_asks_to_open_it() {
        let mut view = Chat::default();
        view.push(Speaker::Login, "Open https://x.test/y\nand enter ABCD-1234");
        draw(&mut view);
        // "login: " takes 7 columns, "Open " 5 more, so the link starts at column 12.
        view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1 + 14, 1));
        let action = view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 1 + 14, 1));
        assert_eq!(action, Some(ChatAction::OpenUrl("https://x.test/y".into())));

        view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1 + 3, 2));
        assert_eq!(view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 1 + 3, 2)), None);
    }

    #[test]
    fn dragging_across_a_link_copies_instead_of_opening() {
        let mut view = Chat::default();
        view.push(Speaker::Login, "Open https://github.com/login/device");
        draw(&mut view);
        view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1 + 12, 1));
        view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 1 + 20, 1));
        let action = view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 1 + 20, 1));
        assert!(matches!(action, Some(ChatAction::Copy(_))), "{action:?}");
    }

    #[test]
    fn selection_across_wrapped_rows_copies_what_is_shown() {
        let mut view = Chat::default();
        view.push(Speaker::You, "alpha beta gamma delta epsilon zeta");
        let lines = draw(&mut view);
        assert!(lines[2].contains("epsilon") || lines[1].contains("epsilon"), "{lines:?}");
        view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
        view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 28, 3));
        let Some(ChatAction::Copy(copied)) = view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 28, 3))
        else {
            panic!("expected a copy");
        };
        assert!(copied.starts_with("you: alpha") && copied.contains('\n'), "{copied:?}");
    }

    #[test]
    fn streaming_text_updates_the_last_reply() {
        let mut view = Chat::default();
        for delta in ["Hel", "lo **wor", "ld**"] {
            view.apply(AgentEvent::Text(delta.into()));
        }
        let shown = screen(&mut view);
        assert!(shown.contains("Hello world") && !shown.contains("**"), "{shown}");
    }

    #[test]
    fn clicking_a_card_header_opens_it_and_clicking_its_output_does_not() {
        // A pane tall enough for the whole card, so its header is on screen.
        let mut chat = Chat::default();
        chat.apply(AgentEvent::ToolStarted { id: "1".into(), summary: "bash(ls)".into() });
        chat.apply(AgentEvent::ToolFinished {
            id: "1".into(),
            summary: "bash(ls)".into(),
            result: ToolResult::ok("1\n2\n3\n4\n5"),
        });
        let shown = draw_at(&mut chat, 20).join("\n");
        let header = shown.lines().position(|l| l.contains("bash")).unwrap().try_into().unwrap();
        assert!(!shown.contains('4'), "{shown}");

        // A click on a line of output leaves the card as it is.
        chat.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, header + 1));
        chat.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 3, header + 1));
        assert!(!draw_at(&mut chat, 20).join("\n").contains('4'));

        chat.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, header));
        assert_eq!(chat.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 3, header)), None);
        let opened = draw_at(&mut chat, 20).join("\n");
        assert!(opened.contains('4') && opened.contains('5'), "{opened}");
        assert!(opened.contains("\u{25be}"), "{opened}");
    }
}
