//! The domain: records and rules that hold without a runtime, a filesystem or a clock.

/// The message history, its budget and how it shrinks.
pub mod conversation;
/// Providers, protocols and models.
pub mod provider;
/// Identities shared across the rings.
pub mod shared;
/// Thinking levels.
pub mod thinking;
/// What the user sees: messages and Tool cards.
pub mod transcript;
