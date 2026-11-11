/// One message laid out.
mod entry;
/// The transcript wrapped to a width.
mod layout;
/// Markdown rendering.
mod markdown;
/// Message lines.
mod message;
/// Rows of wrapped cells.
mod row;
/// Scroll position.
mod scroll;
/// Text selection.
mod selection;
/// Tool card lines.
mod tool_card;
/// The chat pane.
mod view;

pub use view::{ChatAction, ChatView};
