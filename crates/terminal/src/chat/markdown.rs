use alloc::borrow::Cow;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tui_markdown::{AlertKind, BuiltinCodeTheme, Options, StyleSheet, from_str_with_options};

use crate::theme::{self, Status};
use crate::units::to_u16;

#[derive(Debug, Clone)]
/// Styles for rendered markdown.
struct ChatStyles;

impl StyleSheet for ChatStyles {
    fn alert(&self, kind: AlertKind) -> Style {
        let theme = theme::current();
        match kind {
            AlertKind::Note => theme.accent(),
            AlertKind::Tip => theme.status(Status::Completed),
            AlertKind::Important | AlertKind::Warning => theme.status(Status::Waiting),
            AlertKind::Caution => theme.status(Status::Error),
        }
    }

    fn alert_icon(&self, kind: AlertKind) -> &'static str {
        match kind {
            AlertKind::Note => "\u{2139}\u{FE0F}",
            AlertKind::Tip => "\u{1F4A1}",
            AlertKind::Important => "\u{2757}",
            AlertKind::Warning => "\u{26A0}\u{FE0F}",
            AlertKind::Caution => "\u{1F534}",
        }
    }

    fn alert_label(&self, kind: AlertKind) -> &'static str {
        kind.label()
    }

    fn blockquote(&self) -> Style {
        theme::current().secondary().add_modifier(Modifier::ITALIC)
    }

    /// Inline code gets the theme's code style. Fenced blocks are highlighted (see `label_untagged_fences`).
    fn code(&self) -> Style {
        theme::current().code()
    }

    fn code_block_fence(&self) -> &'static str {
        ""
    }

    fn definition_description(&self) -> Style {
        Style::default()
    }

    fn definition_term(&self) -> Style {
        theme::current().text()
    }

    fn footnote_def(&self) -> Style {
        theme::current().muted()
    }

    fn footnote_ref(&self) -> Style {
        theme::current().muted()
    }

    fn heading(&self, level: u8) -> Style {
        theme::current().heading(level)
    }

    fn heading_marker(&self, _level: u8) -> &'static str {
        ""
    }

    fn heading_meta(&self) -> Style {
        theme::current().muted()
    }

    fn html(&self) -> Style {
        theme::current().muted()
    }

    fn image_alt(&self) -> Style {
        theme::current().muted().add_modifier(Modifier::ITALIC)
    }

    // The library paints everything it can in its own named colours. Each of those is replaced
    // so that no element looks foreign next to the rest of the UI.

    /// Links are the one place an underline helps: it says the text can be clicked.
    fn link(&self) -> Style {
        theme::current().accent().add_modifier(Modifier::UNDERLINED)
    }

    fn list_marker(&self) -> Style {
        theme::current().accent()
    }

    fn math_display(&self) -> Style {
        theme::current().code()
    }

    fn math_inline(&self) -> Style {
        theme::current().code().add_modifier(Modifier::ITALIC)
    }

    fn metadata_block(&self) -> Style {
        theme::current().muted()
    }

    fn table_border(&self) -> Style {
        theme::current().border()
    }

    fn table_cell(&self) -> Style {
        Style::default()
    }

    fn table_header(&self) -> Style {
        theme::current().secondary().add_modifier(Modifier::BOLD)
    }
}

pub(super) fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    // The app background is dark, so highlighting uses a dark theme. Its colours are then
    // converted to what the terminal can show (see `Theme::adapt`).
    let options = Options::new(ChatStyles).table_width(to_u16(width)).code_theme(BuiltinCodeTheme::Base16OceanDark);
    let theme = theme::current();
    let source = label_untagged_fences(text);
    from_str_with_options(&source, &options).lines.into_iter().map(|line| adapted(into_owned(line), *theme)).collect()
}

/// How many bytes at the start of a growing `text` are whole blocks that later text cannot
/// change, so they can be rendered once and kept.
///
/// A block is final once a blank line outside a code fence is followed by a complete line that
/// starts a new top-level block: not indented (which might continue a list item or code block)
/// and not a list item (which might continue a list). The result ends where that line begins.
/// A line still being received is never looked at: `1` could yet become `1. item`.
pub(super) fn complete_blocks_len(text: &str) -> usize {
    let mut complete = 0;
    let mut offset = 0usize;
    let mut fence: Option<&str> = None;
    let mut after_blank = false;
    for line in text.split_inclusive('\n') {
        if !line.ends_with('\n') {
            break;
        }
        let start = offset;
        offset = offset.saturating_add(line.len());
        let marker = fence_marker(line);
        if let Some(open) = fence {
            if marker == Some(open) {
                fence = None;
            }
            continue;
        }
        if after_blank && starts_top_level_block(line) {
            complete = start;
        }
        fence = marker;
        after_blank = line.trim().is_empty();
    }
    complete
}

/// The fence characters (```` ``` ```` or `~~~`) a line starts with, if it is a fence line.
fn fence_marker(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    ["```", "~~~"].into_iter().find(|marker| trimmed.starts_with(marker))
}

fn starts_top_level_block(line: &str) -> bool {
    let first = line.chars().next();
    let indented = first.is_none_or(char::is_whitespace);
    !indented && !is_list_item(line)
}

fn is_list_item(line: &str) -> bool {
    let after_digits = line.trim_start_matches(|c: char| c.is_ascii_digit());
    let rest = if after_digits.len() < line.len() {
        after_digits.strip_prefix(['.', ')'])
    } else {
        line.strip_prefix(['-', '*', '+'])
    };
    rest.is_some_and(|text| text.starts_with([' ', '\t']))
}

/// Gives every bare opening fence a language so it is highlighted too.
///
/// With `highlight-code` on, the library styles only fences that name a language, so
/// untagged blocks would come out unstyled. `txt` maps to syntect's plain-text grammar:
/// the block is still colored by the code theme and keeps its exact text.
fn label_untagged_fences(text: &str) -> Cow<'_, str> {
    if !text.contains("```") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len().saturating_add(16));
    let mut in_fence = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            if in_fence {
                in_fence = false;
            } else {
                in_fence = true;
                let info = trimmed.trim_start_matches('`').trim();
                if info.is_empty() {
                    let body = line.trim_end_matches(['\r', '\n']);
                    out.push_str(body);
                    out.push_str("txt");
                    out.push_str(line.strip_prefix(body).unwrap_or_default());
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    Cow::Owned(out)
}

/// Syntax highlighting carries its own RGB colours and backgrounds. Convert them for the
/// terminal (nothing at all under `NO_COLOR`) and let the app background show through.
fn adapted(mut line: Line<'static>, theme: theme::Theme) -> Line<'static> {
    line.style = theme.adapt(line.style);
    for span in &mut line.spans {
        span.style = theme.adapt(span.style);
    }
    line
}

fn into_owned(line: Line<'_>) -> Line<'static> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .into_iter()
            .map(|span| Span { style: span.style, content: Cow::Owned(span.content.into_owned()) })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn plain(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(plain).collect()
    }

    #[test]
    fn bold_and_italic_become_modifiers() {
        let lines = render("a **bold** and *slanted* word", 80);
        let spans: Vec<&Span<'_>> = lines.iter().flat_map(|l| &l.spans).collect();
        let bold = spans.iter().find(|s| s.content == "bold").unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let italic = spans.iter().find(|s| s.content == "slanted").unwrap();
        assert!(italic.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn markup_characters_are_not_shown() {
        let shown = texts(&render("a **bold** and `code`", 80)).join("\n");
        assert!(!shown.contains("**") && !shown.contains('`'), "{shown}");
        assert!(shown.contains("bold") && shown.contains("code"));
    }

    #[test]
    fn lists_render_one_line_per_item() {
        let shown = texts(&render("- one\n- two\n- three", 80));
        let items: Vec<&String> = shown.iter().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(items.len(), 3, "{shown:?}");
        assert!(items[0].contains("one") && items[2].contains("three"));
    }

    #[test]
    fn code_blocks_keep_their_lines() {
        let shown = texts(&render("```rust\nfn main() {}\nlet x = 1;\n```", 80)).join("\n");
        assert!(shown.contains("fn main() {}") && shown.contains("let x = 1;"), "{shown}");
    }

    #[test]
    fn code_fences_are_hidden() {
        let shown = texts(&render("```rust\nlet x = 1;\n```", 80));
        assert!(shown.iter().all(|l| !l.contains("```")), "{shown:?}");
    }

    #[test]
    fn untagged_code_is_highlighted_like_tagged_code() {
        let lines = render("```\nplain text\n```", 80);
        let span = lines.iter().flat_map(|l| &l.spans).find(|s| s.content.contains("plain")).unwrap();
        assert!(span.style.fg.is_some() || theme::current().color(theme::palette::TEXT).is_none(), "{span:?}");
    }

    #[test]
    fn only_opening_bare_fences_are_labelled() {
        assert_eq!(label_untagged_fences("```\nx\n```\n"), "```txt\nx\n```\n");
        assert_eq!(label_untagged_fences("```rust\nx\n```\n"), "```rust\nx\n```\n");
        assert_eq!(label_untagged_fences("```\r\nx\r\n```"), "```txt\r\nx\r\n```");
        // A closing fence inside a block is not a new opening fence.
        assert_eq!(label_untagged_fences("```rust\nx\n```\n```\ny\n```"), "```rust\nx\n```\n```txt\ny\n```");
        assert_eq!(label_untagged_fences("no code"), "no code");
    }

    fn frozen(text: &str) -> &str {
        text.get(..complete_blocks_len(text)).unwrap_or_default()
    }

    #[test]
    fn a_block_is_complete_once_the_next_one_has_started() {
        assert_eq!(frozen("one\n\ntwo\n"), "one\n\n");
        assert_eq!(frozen("one\n\ntwo"), "", "`two` may still be growing");
        assert_eq!(frozen("one\n\n"), "", "nothing follows the blank line yet");
        assert_eq!(frozen("one\ntwo\n\nthree\n\nfour\n"), "one\ntwo\n\nthree\n\n");
    }

    #[test]
    fn blank_lines_inside_a_code_fence_do_not_end_a_block() {
        assert_eq!(frozen("```\na\n\nb\n```\n\nafter\n"), "```\na\n\nb\n```\n\n");
        assert_eq!(frozen("```\na\n\nb\n\nc\n"), "", "the fence is still open");
        assert_eq!(frozen("~~~\na\n```\n\nb\n"), "", "a different fence does not close it");
    }

    #[test]
    fn lists_and_indented_text_may_continue_the_previous_block() {
        assert_eq!(frozen("- a\n\n- b\n"), "");
        assert_eq!(frozen("1. a\n\n2. b\n"), "");
        assert_eq!(frozen("- a\n\n  more\n"), "");
        assert_eq!(frozen("para\n\n    code\n"), "");
        assert_eq!(frozen("para\n\n-- dashes\n"), "para\n\n", "not a list item");
        assert_eq!(frozen("para\n\n2024 was a year\n"), "para\n\n", "not a list item");
    }

    #[test]
    fn headings_are_styled_without_a_marker() {
        let lines = render("# Title", 80);
        let heading = lines.iter().find(|l| plain(l).contains("Title")).unwrap();
        assert_eq!(plain(heading), "Title");
        // The library styles the whole line, not its spans.
        assert!(heading.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn incomplete_markdown_still_renders() {
        for partial in ["```rust\nfn main(", "**bold", "- item\n  - nested", "| a | b |\n|---", "[link](http"] {
            let lines = render(partial, 40);
            assert!(!lines.is_empty(), "{partial:?}");
        }
    }

    #[test]
    fn tables_fit_the_width() {
        let table = "| Name | Description |\n| --- | --- |\n| alpha | a rather long description that must wrap |";
        for line in render(table, 30) {
            assert!(line.width() <= 30, "{:?}", plain(&line));
        }
    }
}
