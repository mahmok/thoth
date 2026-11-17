//! Adapters: the clients that talk to the outside world.

/// Provider credentials and login.
pub mod auth;
/// Where configuration lives.
pub mod config;
/// Prompt context: AGENTS.md files and the token budget.
pub mod context;
/// `@file` references and the index behind them.
pub mod files;
/// File helpers.
pub mod fsutil;
/// Model clients.
pub mod llm;
/// The models each provider offers.
pub mod models;
/// Personas an Agent can play.
pub mod persona;
/// Skills a prompt can pull in.
pub mod skills;
/// The built-in Tools an Agent can call.
pub mod tools;
/// The working directory and what Tools may touch.
pub mod workspace;
