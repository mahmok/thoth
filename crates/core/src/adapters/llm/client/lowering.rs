//! Conversation lowering shared by the dialects.

use super::chat::{Message, Role};

/// What one dialect does with each kind of message.
pub(super) trait Dialect<'message> {
    /// The wire form of the lowered conversation.
    type Output;

    /// The joined system prompt.
    fn system(&mut self, text: &str);

    /// A user message.
    fn user(&mut self, text: &'message str);

    /// An assistant message: its reasoning, text, and Tool calls in dialect order.
    fn assistant(&mut self, message: &'message Message);

    /// A Tool result.
    fn tool_result(&mut self, message: &'message Message);

    /// The lowered conversation.
    fn finish(self) -> Self::Output
    where
        Self: Sized;
}

/// Lowers a conversation once, leaving each dialect to say what its turns become.
pub(super) fn lower<'message, D>(messages: &'message [Message], mut dialect: D) -> D::Output
where
    D: Dialect<'message>,
{
    let mut system: Vec<&'message str> = Vec::new();
    for message in messages {
        match message.role {
            Role::System => system.extend(message.text()),
            Role::User => {
                if let Some(text) = message.text() {
                    dialect.user(text);
                }
            },
            Role::Assistant => dialect.assistant(message),
            Role::Tool => dialect.tool_result(message),
        }
    }
    if !system.is_empty() {
        let system = system.join("\n\n");
        dialect.system(&system);
    }
    dialect.finish()
}
