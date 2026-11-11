use std::collections::HashMap;

use serde_json::{Value, json};

use super::{Conversation, tokens};
use crate::agent::context::Elision;
use crate::llm::client::chat::{Message, Role};

/// The start of the note left where output was dropped.
const NOTE: &str = r#"{"elided":"#;

#[expect(clippy::multiple_inherent_impl, reason = "its impls are split by topic across files")]
impl Conversation {
    fn drop_obsolete_reasoning(&mut self) {
        let latest_turn = self.messages.iter().rposition(|m| m.role == Role::User);
        for message in self.messages.iter_mut().take(latest_turn.unwrap_or(0)) {
            message.reasoning.clear();
        }
    }

    #[inline]
    pub fn elide(&mut self, rules: &Elision) {
        let _elided = self.elide_before(self.messages.len(), rules);
    }

    pub(super) fn elide_before(&mut self, end: usize, rules: &Elision) -> bool {
        let elided = self.messages.get_mut(..end).is_some_and(|head| elide_messages(head, rules));
        if elided {
            self.drop_obsolete_reasoning();
        }
        elided
    }
}

/// Elides the oldest bulky text once the total passes the trigger. Returns whether it changed anything.
pub(super) fn elide_messages(messages: &mut [Message], rules: &Elision) -> bool {
    let labels = call_labels(messages);
    let mut bulky: Vec<(&mut String, String)> = messages
        .iter_mut()
        .flat_map(|message| elidable(message, &labels))
        .filter(|entry| tokens(entry.0.as_str()) >= rules.smallest && !entry.0.starts_with(NOTE))
        .collect();
    if bulky.iter().map(|entry| tokens(entry.0.as_str())).sum::<usize>() <= rules.trigger {
        return false;
    }
    let mut elided = false;
    let mut kept = 0usize;
    for &mut (ref mut text, ref label) in bulky.iter_mut().rev() {
        let size = tokens(text);
        kept = kept.saturating_add(size);
        if kept > rules.target {
            **text = note(label, size);
            elided = true;
        }
    }
    elided
}

/// The note that stands in for dropped content. It says what was dropped, so the model knows
/// which file or command to look at again. It stays valid JSON, because it can replace the
/// arguments of a tool call.
fn note(label: &str, size: usize) -> String {
    let reason = format!("{label}: about {size} tokens dropped to save space; run the tool again if you need them");
    json!({ "elided": reason }).to_string()
}

/// What each tool call was, by call id: its name and the path or command it was given.
fn call_labels(messages: &[Message]) -> HashMap<String, String> {
    messages
        .iter()
        .flat_map(|message| &message.tool_calls)
        .map(|call| {
            let arguments = serde_json::from_str::<Value>(&call.function.arguments).ok();
            let target = arguments
                .as_ref()
                .and_then(|value| ["path", "command", "query"].iter().find_map(|key| value.get(key)?.as_str()));
            let label = target.map_or_else(
                || call.function.name.clone(),
                |text| {
                    let shortened: String = text.chars().take(80).collect();
                    format!("{}({shortened})", call.function.name)
                },
            );
            (call.id.clone(), label)
        })
        .collect()
}

/// The texts of a message that may be elided, each with the label its note will use.
fn elidable<'message>(
    message: &'message mut Message,
    labels: &HashMap<String, String>,
) -> Vec<(&'message mut String, String)> {
    let result_label = message
        .tool_call_id
        .as_ref()
        .and_then(|id| labels.get(id))
        .map_or_else(|| "tool result".to_owned(), |label| format!("{label} result"));
    let is_result = message.role == Role::Tool;
    let mut found = Vec::new();
    if is_result && let Some(content) = message.content.as_mut() {
        found.push((content, result_label));
    }
    for call in &mut message.tool_calls {
        let label = format!("{} arguments", call.function.name);
        found.push((&mut call.function.arguments, label));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::super::tests::call;
    use super::*;
    use crate::agent::tools::tool::Outcome as ToolResult;
    use crate::agent::types::AgentMessage;
    use crate::llm::client::chat::Reasoning;

    fn rules() -> Elision {
        Elision { trigger: 25_000, target: 12_500, smallest: 250 }
    }

    /// A finished Turn in which a tool was called with `args` tokens and returned `output` tokens.
    fn turn(conversation: &mut Conversation, args: usize, output: usize) {
        conversation.elide(&rules());
        let mut turn = conversation.begin_turn("go");
        let mut call = call("c");
        call.function.arguments = "a".repeat(args.saturating_mul(4));
        turn.record_tool_step(
            AgentMessage::new(None, vec![Reasoning::Anthropic("thought".into())]),
            vec![(call, ToolResult::ok("x".repeat(output.saturating_mul(4))))],
        );
        turn.complete("done");
    }

    fn sizes(conversation: &Conversation) -> Vec<(usize, usize)> {
        let messages = conversation.messages();
        let size = |text: &str| {
            if text.starts_with(NOTE) { 0 } else { tokens(text) }
        };
        messages
            .iter()
            .zip(&messages[1..])
            .filter(|&(_, next)| next.role == Role::Tool)
            .map(|(asked, answer)| {
                let args = &asked.tool_calls[0].function.arguments;
                (size(args), size(answer.content.as_deref().unwrap()))
            })
            .collect()
    }

    #[test]
    fn old_tool_content_is_kept_while_it_is_modest() {
        let mut conversation = Conversation::new();
        for _ in 0i32..5i32 {
            turn(&mut conversation, 0, 5_000);
        }
        assert_eq!(sizes(&conversation), [(0, 5_000); 5]);
    }

    #[test]
    fn too_much_drops_the_oldest_results_and_arguments_down_to_the_target() {
        let mut conversation = Conversation::new();
        for _ in 0i32..6i32 {
            turn(&mut conversation, 2_000, 3_000);
        }
        turn(&mut conversation, 0, 0);
        assert_eq!(sizes(&conversation), [(0, 0), (0, 0), (0, 0), (0, 0), (2_000, 3_000), (2_000, 3_000), (0, 0)]);
        let note = conversation.messages()[2].content.clone().unwrap();
        assert!(note.contains("about 3000 tokens"), "{note}");
        let parsed: Value = serde_json::from_str(&note).unwrap();
        assert!(parsed.is_object(), "notes stay valid JSON arguments");
    }

    #[test]
    fn reasoning_is_dropped_with_the_content_except_in_the_latest_turn() {
        let mut conversation = Conversation::new();
        for _ in 0i32..6i32 {
            turn(&mut conversation, 0, 5_000);
        }
        conversation.elide(&rules());
        let with_reasoning: Vec<bool> = conversation
            .messages()
            .iter()
            .filter(|m| m.role == Role::Assistant && !m.tool_calls.is_empty())
            .map(|m| !m.reasoning.is_empty())
            .collect();
        assert_eq!(with_reasoning, [false, false, false, false, false, true]);
    }

    #[test]
    fn elision_before_a_point_leaves_what_follows_alone() {
        let mut conversation = Conversation::new();
        turn(&mut conversation, 0, 5_000);
        turn(&mut conversation, 0, 5_000);
        let end = conversation.messages().len() - 2;
        let hard = Elision { trigger: 0, target: 0, smallest: 0 };
        assert!(conversation.elide_before(end, &hard));
        assert_eq!(sizes(&conversation), [(0, 0), (0, 5_000)]);
        assert!(!conversation.elide_before(end, &hard), "nothing is left to elide");
    }

    #[test]
    fn notes_are_not_elided_again_and_small_content_is_left_alone() {
        let mut conversation = Conversation::new();
        turn(&mut conversation, 0, 50);
        for _ in 0i32..6i32 {
            turn(&mut conversation, 0, 5_000);
        }
        conversation.elide(&rules());
        let once = conversation.messages().to_vec();
        conversation.elide(&rules());
        assert_eq!(conversation.messages()[2].content.as_deref().map(tokens), Some(50));
        assert_eq!(format!("{:?}", conversation.messages()), format!("{once:?}"), "a second pass changes nothing");
    }

    #[test]
    fn a_note_says_which_call_it_replaces() {
        let mut conversation = Conversation::new();
        let mut turn = conversation.begin_turn("go");
        let mut read = call("r1");
        read.function.name = "read_file".into();
        read.function.arguments = r#"{"path":"src/lib.rs"}"#.into();
        turn.record_tool_step(AgentMessage::default(), vec![(read, ToolResult::ok("x".repeat(40_000)))]);
        turn.complete("done");
        drop(turn);
        conversation.elide(&Elision::EVERYTHING);
        let note = conversation.messages()[2].content.clone().unwrap();
        assert!(note.contains("read_file(src/lib.rs) result"), "{note}");
        serde_json::from_str::<Value>(&note).unwrap();
    }
}
