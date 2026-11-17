//! Thoth core: Agents, Tools, Sessions and the Harness that owns them.

#![cfg_attr(test, expect(unused_results, reason = "tests call things for their effect and ignore what they return"))]

/// The clients that talk to the outside world.
pub mod adapters;
/// Provider credentials and login.
pub mod auth;
/// Starting the app.
pub mod bootstrap;
/// Built-in references: files and skills.
pub mod builtin;
/// The system clock.
pub mod clock;
/// Where configuration lives.
pub mod config;
/// The records and rules that hold without a runtime.
pub mod domain;
/// File helpers.
pub mod fsutil;
/// The Harness: the Sessions and what they share.
pub mod harness;
/// The models each provider offers.
pub mod models;
/// Notices for the user.
pub mod notice;
/// Where db constructs live
pub mod persistence;
/// Personas an Agent can play.
pub mod persona;
/// The Agent, its Turns and the tasks they run on.
pub mod runtime;
/// The services a Harness is built from.
pub mod services;
/// A Session: one conversation and its view.
pub mod session;
pub mod sync;
#[cfg(any(test, feature = "testing"))]
#[expect(clippy::unwrap_used, reason = "test support: a failed setup should fail the test")]
pub mod test;
/// Small shared helpers.
pub mod utils;
/// The working directory and what Tools may touch.
pub mod workspace;

extern crate alloc;

pub const APP_NAME: &str = "thoth";
