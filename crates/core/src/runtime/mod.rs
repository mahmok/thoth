//! The runtime: the Agent, its Turns and the tasks they run on.

/// The Agent that talks to a model and runs Tools.
pub mod agent;
pub mod agent_builder;
/// Work that runs off the UI thread.
pub mod background;
/// Prompt context: AGENTS.md files and the token budget.
pub mod context;
/// Skills a prompt can pull in.
pub mod skills;
/// Tools an Agent can call.
pub mod tools;
/// Runs a Turn on its own task and reports back.
pub mod turn_runner;
/// The messages and responses of an Agent.
pub mod types;
