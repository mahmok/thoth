//! Thoth core: Agents, Tools, Sessions and the Harness that owns them.

#![cfg_attr(test, expect(unused_results, reason = "tests call things for their effect and ignore what they return"))]

/// The Agent that talks to a model and runs Tools.
pub mod agent;
pub mod agent_builder;
/// Provider credentials and login.
pub mod auth;
/// Work that runs off the UI thread.
pub mod background;
/// Starting the app.
pub mod bootstrap;
/// Built-in references: files and skills.
pub mod builtin;
/// Where configuration lives.
pub mod config;
/// File helpers.
pub mod fsutil;
/// The Harness: the Sessions and what they share.
pub mod harness;
/// Model clients.
pub mod llm;
/// The models each provider offers.
pub mod models;
/// Notices for the user.
pub mod notice;
/// Personas an Agent can play.
pub mod persona;
/// The services a Harness is built from.
pub mod services;
/// A Session: one conversation and its view.
pub mod session;
pub mod sync;
#[cfg(any(test, feature = "testing"))]
#[expect(clippy::unwrap_used, reason = "test support: a failed setup should fail the test")]
pub mod test;
/// What the user sees: messages and Tool cards.
pub mod transcript;
/// Small shared helpers.
pub mod utils;
/// The working directory and what Tools may touch.
pub mod workspace;

extern crate alloc;

pub const APP_NAME: &str = "thoth";
