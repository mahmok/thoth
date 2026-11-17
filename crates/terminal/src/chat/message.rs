use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use thoth_core::domain::transcript::{Message, Speaker};

use super::markdown;
use crate::theme::{self, Shade, palette};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// How a message body is drawn.
enum BodyFormat {
    Markdown,
    Muted,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// What colours a message body. Notices are the app speaking up rather than the user or the
/// agent, so they are set apart in gold: a bright label over a softer body.
enum Body {
    Error,
    Gold,
    Text,
}

/// Everything a speaker decides about how its message looks, in one row, so adding a speaker is
/// one edit rather than four matches that must be kept in step.
#[derive(Debug, Clone, Copy)]
struct Look {
    body: Body,
    format: BodyFormat,
    label: &'static str,
    shade: Shade,
}

const fn look(speaker: Speaker) -> Look {
    match speaker {
        Speaker::You => Look { body: Body::Text, format: BodyFormat::Plain, label: "you", shade: palette::TEAL },
        Speaker::Agent => Look { body: Body::Text, format: BodyFormat::Markdown, label: "agent", shade: palette::TEXT },
        Speaker::Thinking => {
            Look { body: Body::Text, format: BodyFormat::Muted, label: "thinking", shade: palette::TEXT_MUTED }
        },
        Speaker::Tool => {
            Look { body: Body::Text, format: BodyFormat::Plain, label: "tool", shade: palette::TEXT_SECONDARY }
        },
        Speaker::Error => Look { body: Body::Error, format: BodyFormat::Plain, label: "error", shade: palette::ERROR },
        Speaker::Notice => Look { body: Body::Gold, format: BodyFormat::Plain, label: "notice", shade: palette::GOLD },
        Speaker::Skills => Look { body: Body::Gold, format: BodyFormat::Plain, label: "skills", shade: palette::GOLD },
        Speaker::Login => Look { body: Body::Gold, format: BodyFormat::Plain, label: "login", shade: palette::GOLD },
        Speaker::Models => {
            Look { body: Body::Text, format: BodyFormat::Plain, label: "models", shade: palette::TEXT_SECONDARY }
        },
    }
}

/// The body style needs the theme, so it is resolved here rather than stored in the Look.
fn body_style(body: Body) -> Style {
    let theme = theme::current();
    match body {
        Body::Error => theme.status(theme::Status::Error),
        Body::Gold => theme.fg(palette::GOLD_MUTED),
        Body::Text => theme.text(),
    }
}

/// Markdown messages are the ones that grow by streaming and are costly to render.
pub(super) fn is_markdown(message: &Message) -> bool {
    look(message.speaker).format == BodyFormat::Markdown
}

pub(super) fn label_line(message: &Message) -> Line<'static> {
    let look = look(message.speaker);
    let label_style = theme::current().fg(look.shade);
    Line::from(Span::styled(format!("{}:", look.label), label_style))
}

/// The lines of a message at a width.
pub(super) fn lines(message: &Message, width: usize) -> Vec<Line<'static>> {
    let look = look(message.speaker);
    let label_style = theme::current().fg(look.shade);
    match look.format {
        BodyFormat::Plain => {
            let body = body_style(look.body);
            let tag = Span::styled(format!("{}: ", look.label), label_style);
            let mut lines =
                message.text.split('\n').map(|l| Line::from(Span::styled(l.to_owned(), body))).collect::<Vec<_>>();
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

    const SPEAKERS: [Speaker; 9] = [
        Speaker::Agent,
        Speaker::Error,
        Speaker::Login,
        Speaker::Models,
        Speaker::Notice,
        Speaker::Skills,
        Speaker::Thinking,
        Speaker::Tool,
        Speaker::You,
    ];

    #[test]
    fn every_speaker_has_its_own_label() {
        let labels: Vec<&str> = SPEAKERS.into_iter().map(|speaker| look(speaker).label).collect();
        assert_eq!(labels.len(), SPEAKERS.len());
        for (index, label) in labels.iter().enumerate() {
            assert!(!labels[index + 1..].contains(label), "{label} names two speakers");
        }
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
