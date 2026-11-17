//! The domain: records and rules that hold without a runtime, a filesystem or a clock.

/// The message history, its budget and how it shrinks.
pub mod conversation;
/// The messages a Conversation is made of.
pub mod message;
/// The Persona and what it may use.
pub mod persona;
/// Providers, protocols and models.
pub mod provider;
/// Identities and token counts shared across the rings.
pub mod shared;
/// Thinking levels.
pub mod thinking;
/// What a Tool call returns.
pub mod tool;
/// What the user sees: messages and Tool cards.
pub mod transcript;
