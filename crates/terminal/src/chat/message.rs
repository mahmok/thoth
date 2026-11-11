use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use thoth_core::transcript::{Message, Speaker};

use super::markdown;
use crate::theme::{self, Shade, palette};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// How a message body is drawn.
enum BodyFormat {
    Markdown,
    Muted,
    Plain,
}

const fn label(speaker: Speaker) -> &'static str {
    match speaker {
        Speaker::You => "you",
        Speaker::Agent => "agent",
        Speaker::Thinking => "thinking",
        Speaker::Tool => "tool",
        Speaker::Error => "error",
        Speaker::Notice => "notice",
        Speaker::Skills => "skills",
        Speaker::Login => "login",
        Speaker::Models => "models",
    }
}

/// The label colour of each speaker: teal for the user, plain text for the agent, gold for
/// what needs attention, and quiet text for everything else.
const fn color(speaker: Speaker) -> Shade {
    match speaker {
        Speaker::You => palette::TEAL,
        Speaker::Agent => palette::TEXT,
        Speaker::Thinking => palette::TEXT_MUTED,
        Speaker::Tool | Speaker::Models => palette::TEXT_SECONDARY,
        Speaker::Error => palette::ERROR,
        Speaker::Notice | Speaker::Skills | Speaker::Login => palette::GOLD,
    }
}

/// Notices are the app speaking up rather than the user or the agent, so they are set apart in
/// gold: a bright label over a softer body.
fn body_style(speaker: Speaker) -> Style {
    let theme = theme::current();
    match speaker {
        Speaker::Notice | Speaker::Skills | Speaker::Login => theme.fg(palette::GOLD_MUTED),
        Speaker::Error => theme.status(theme::Status::Error),
        Speaker::You | Speaker::Agent | Speaker::Thinking | Speaker::Tool | Speaker::Models => theme.text(),
    }
}

const fn body_format(speaker: Speaker) -> BodyFormat {
    match speaker {
        Speaker::Agent => BodyFormat::Markdown,
        Speaker::Thinking => BodyFormat::Muted,
        Speaker::You
        | Speaker::Tool
        | Speaker::Error
        | Speaker::Notice
        | Speaker::Skills
        | Speaker::Login
        | Speaker::Models => BodyFormat::Plain,
    }
}

/// Markdown messages are the ones that grow by streaming and are costly to render.
pub(super) fn is_markdown(message: &Message) -> bool {
    body_format(message.speaker) == BodyFormat::Markdown
}

pub(super) fn label_line(message: &Message) -> Line<'static> {
    let label_style = theme::current().fg(color(message.speaker));
    Line::from(Span::styled(format!("{}:", label(message.speaker)), label_style))
}

/// The lines of a message at a width.
pub(super) fn lines(message: &Message, width: usize) -> Vec<Line<'static>> {
    let label_style = theme::current().fg(color(message.speaker));
    match body_format(message.speaker) {
        BodyFormat::Plain => {
            let tag = Span::styled(format!("{}: ", label(message.speaker)), label_style);
            let mut lines = message
                .text
                .split('\n')
                .map(|l| Line::from(Span::styled(l.to_owned(), body_style(message.speaker))))
                .collect::<Vec<_>>();
            if let Some(first) = lines.first_mut() {
                first.spans.insert(0, tag);
            }
            lines
        },
        BodyFormat::Muted => {
            let style = theme::current().muted().add_modifier(Modifier::ITALIC);
            message.text.split('\n').map(|l| Line::from(Span::styled(l.to_owned(), style))).collect()
        },
        BodyFormat::Markdown => {
            let mut lines = vec![label_line(message)];
            lines.extend(markdown::render(&message.text, width));
            lines
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(speaker: Speaker, text: &str) -> Message {
        Message { speaker, text: text.to_owned() }
    }

    fn plain(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn plain_messages_put_the_label_inline() {
        let lines = lines(&message(Speaker::You, "hello\nsecond"), 80);
        assert_eq!(lines.iter().map(plain).collect::<Vec<_>>(), ["you: hello", "second"]);
    }

    #[test]
    fn thinking_is_muted_italic_text_without_a_label() {
        let lines = lines(&message(Speaker::Thinking, "hmm\nok"), 80);
        assert_eq!(lines.iter().map(plain).collect::<Vec<_>>(), ["hmm", "ok"]);
        let style = lines[0].spans[0].style;
        assert!(style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn plain_messages_do_not_interpret_markdown() {
        let lines = lines(&message(Speaker::You, "**not bold**"), 80);
        assert_eq!(plain(&lines[0]), "you: **not bold**");
    }

    #[test]
    fn an_empty_plain_message_still_has_its_label() {
        let lines = lines(&message(Speaker::Tool, ""), 80);
        assert_eq!(plain(&lines[0]), "tool: ");
    }

    #[test]
    fn agent_messages_render_markdown_under_a_label_line() {
        let lines = lines(&message(Speaker::Agent, "**hi** there"), 80);
        assert_eq!(plain(&lines[0]), "agent:");
        let shown = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
        assert!(shown.contains("hi there") && !shown.contains("**"), "{shown}");
    }

    #[test]
    fn labels_are_colored_per_speaker() {
        let lines = lines(&message(Speaker::Error, "bad"), 80);
        assert_eq!(lines[0].spans[0].style.fg, theme::current().color(palette::ERROR));
    }

    #[test]
    fn notices_are_gold_with_a_softer_body() {
        let theme = theme::current();
        for speaker in [Speaker::Notice, Speaker::Skills, Speaker::Login] {
            let lines = lines(&message(speaker, "one\ntwo"), 80);
            assert_eq!(lines[0].spans[0].style.fg, theme.color(palette::GOLD));
            for line in &lines {
                let body = line.spans.last().unwrap();
                assert_eq!(body.style.fg, theme.color(palette::GOLD_MUTED), "{speaker:?}");
            }
        }
    }

    #[test]
    fn the_user_and_agent_are_not_gold() {
        let theme = theme::current();
        let you = lines(&message(Speaker::You, "hi"), 80);
        if theme.color(palette::GOLD).is_some() {
            assert_ne!(you[0].spans[0].style.fg, theme.color(palette::GOLD));
        }
        assert_eq!(you[0].spans[1].style.fg, theme.color(palette::TEXT));
    }
}
