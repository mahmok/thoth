use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use thoth_core::transcript::tool_call::{ToolCall, ToolStatus};

use crate::theme::{self, Status};

const fn status(tool: ToolStatus) -> Status {
    match tool {
        ToolStatus::Running => Status::Running,
        ToolStatus::Done => Status::Completed,
        ToolStatus::Failed => Status::Error,
    }
}

const fn icon(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Running => "\u{25d0}",
        ToolStatus::Done => "\u{2714}",
        ToolStatus::Failed => "\u{2718}",
    }
}

const fn status_word(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Running => "running",
        ToolStatus::Done => "done",
        ToolStatus::Failed => "failed",
    }
}

/// A Tool card: its header, its progress while it runs, and its output, either the preview or
/// everything when it is `expanded`.
pub(super) fn lines(call: &ToolCall, now: u64, expanded: bool) -> Vec<Line<'static>> {
    let theme = theme::current();
    let state = theme.status(status(call.status));
    let muted = theme.muted();

    let (name, args) = match call.summary.split_once('(') {
        Some((name, rest)) => (name.to_owned(), format!("({rest}")),
        None => (call.summary.clone(), String::new()),
    };
    // Only the glyph and the status word carry the state colour; the name is teal and the
    // arguments and timing recede, so a column of cards stays calm.
    let mut header = vec![Span::styled(format!("{} ", icon(call.status)), state), Span::styled(name, theme.accent())];
    if !args.is_empty() {
        header.push(Span::styled(args, theme.secondary()));
    }
    header.push(Span::styled("  ", Style::new()));
    header.push(Span::styled(status_word(call.status).to_owned(), state));
    header.push(Span::styled(format!(" {}", format_duration(call.duration_ms(now))), theme.log_meta()));
    if expanded || call.hidden_lines() > 0 {
        let marker = if expanded { " \u{25be}" } else { " \u{25b8}" };
        header.push(Span::styled(marker, theme.accent()));
    }

    let output_style = if call.status == ToolStatus::Failed { theme.status(Status::Error) } else { muted };
    let gutter = || Span::styled("\u{2502} ", theme.border());
    let mut lines = vec![Line::from(header)];
    if let Some(progress) = call.progress.as_deref().filter(|_| call.is_running()) {
        lines.push(Line::from(vec![gutter(), Span::styled(progress.to_owned(), muted.add_modifier(Modifier::ITALIC))]));
    }
    let shown = if expanded { call.output() } else { call.preview() };
    lines.extend(shown.iter().map(|text| Line::from(vec![gutter(), Span::styled(text.clone(), output_style)])));
    if !expanded && call.hidden_lines() > 0 {
        lines.push(Line::from(vec![
            gutter(),
            Span::styled(
                format!("… {} more lines · click to expand", call.hidden_lines()),
                muted.add_modifier(Modifier::ITALIC),
            ),
        ]));
    }
    lines
}

#[expect(clippy::float_arithmetic, reason = "seconds are shown with one decimal")]
fn format_duration(millis: u64) -> String {
    if millis < 1000 {
        format!("{millis}ms")
    } else if millis < 60_000 {
        format!("{:.1}s", f64::from(u32::try_from(millis).unwrap_or(u32::MAX)) / 1000.0)
    } else {
        format!("{}m{:02}s", millis.div_euclid(60_000), millis.div_euclid(1000).rem_euclid(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn a_running_call_shows_its_status_and_live_time() {
        let start = 1_000;
        let card = ToolCall::running("1", "read_file(a)", start);
        let lines = lines(&card, start + 1500, false);
        assert_eq!(lines.len(), 1);
        let text = plain(&lines[0]);
        assert!(text.contains("running"), "{text}");
        assert!(text.contains("read_file(a)"), "{text}");
        assert!(text.contains("1.5s"), "{text}");
    }

    #[test]
    fn a_running_call_shows_the_progress_it_reports_under_its_header() {
        let start = 1_000;
        let mut card = ToolCall::running("1", "delegate(tester: look)", start);
        let no_progress = lines(&card, start, false).len();
        card.progress = Some("tester-1 \u{b7} step 2 \u{b7} bash(cargo test)".into());
        let text: Vec<String> = lines(&card, start, false).iter().map(plain).collect();
        assert_eq!(text.len(), no_progress + 1);
        assert!(text[1].contains("tester-1 \u{b7} step 2 \u{b7} bash(cargo test)"), "{text:?}");
    }

    #[test]
    fn a_finished_call_shows_no_progress() {
        let start = 1_000;
        let mut card = ToolCall::running("1", "delegate(tester: look)", start);
        card.progress = Some("tester-1 \u{b7} step 2".into());
        card.finish(start + 5, false, "done");
        let text: Vec<String> = lines(&card, start, false).iter().map(plain).collect();
        assert!(!text.iter().any(|l| l.contains("step 2")), "{text:?}");
    }

    #[test]
    fn a_finished_call_keeps_its_time_and_previews_output() {
        let start = 1_000;
        let mut card = ToolCall::running("1", "read_file(a)", start);
        card.finish(start + 250, false, "one\ntwo\nthree\nfour\nfive");
        let lines = lines(&card, start + 9_000, false);
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert!(text[0].contains("done") && text[0].contains("250ms"), "{text:?}");
        assert!(text.iter().any(|l| l.contains("one")), "{text:?}");
        assert!(text.iter().any(|l| l.contains("two")), "{text:?}");
        assert!(text.iter().any(|l| l.contains("2 more lines")), "{text:?}");
        assert!(!text.iter().any(|l| l.contains("five")), "{text:?}");
    }

    #[test]
    fn a_failed_call_shows_the_whole_error() {
        let start = 1_000;
        let mut card = ToolCall::running("1", "read_file(../x)", start);
        card.finish(start, true, "outside the working directory");
        let text: Vec<String> = lines(&card, start, false).iter().map(plain).collect();
        assert!(text[0].contains("failed"), "{text:?}");
        assert!(text.iter().any(|l| l.contains("outside the working directory")), "{text:?}");
    }

    #[test]
    fn durations_switch_units() {
        assert_eq!(format_duration(42), "42ms");
        assert_eq!(format_duration(1234), "1.2s");
        assert_eq!(format_duration(75_000), "1m15s");
    }

    #[test]
    fn a_collapsed_card_previews_and_says_how_to_open_it() {
        let mut card = ToolCall::running("1", "bash(ls)", 0);
        card.finish(0, false, "1\n2\n3\n4\n5");
        let text: Vec<String> = lines(&card, 0, false).iter().map(plain).collect();
        assert!(text[0].ends_with("\u{25b8}"), "{text:?}");
        assert!(text.iter().any(|l| l.contains('3')), "{text:?}");
        assert!(!text.iter().any(|l| l.contains('4')), "{text:?}");
        assert!(text.iter().any(|l| l.contains("click to expand")), "{text:?}");
    }

    #[test]
    fn an_expanded_card_shows_all_of_its_output() {
        let mut card = ToolCall::running("1", "bash(ls)", 0);
        card.finish(0, false, "1\n2\n3\n4\n5");
        let text: Vec<String> = lines(&card, 0, true).iter().map(plain).collect();
        assert!(text[0].ends_with("\u{25be}"), "{text:?}");
        for expected in ["1", "2", "3", "4", "5"] {
            assert!(text.iter().any(|l| l.ends_with(expected)), "{text:?}");
        }
        assert!(!text.iter().any(|l| l.contains("click to expand")), "{text:?}");
    }

    #[test]
    fn a_card_with_nothing_more_to_show_has_no_marker() {
        let mut card = ToolCall::running("1", "bash(true)", 0);
        card.finish(0, false, "ok");
        let text: Vec<String> = lines(&card, 0, false).iter().map(plain).collect();
        assert!(!text[0].contains('\u{25b8}') && !text[0].contains('\u{25be}'), "{text:?}");
    }
}
