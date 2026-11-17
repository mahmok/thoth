//! The runtime: the Agent, its Turns and the tasks they run on.

/// The Agent that talks to a model and runs Tools.
pub mod agent;
pub mod agent_builder;
/// Work that runs off the UI thread.
pub mod background;
/// Tools an Agent can call.
pub mod tools;
/// Runs a Turn on its own task and reports back.
pub mod turn_runner;
/// The messages and responses of an Agent.
pub mod types;
