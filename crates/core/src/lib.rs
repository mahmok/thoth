//! Thoth core: Agents, Tools, Sessions and the Harness that owns them.

#![cfg_attr(test, expect(unused_results, reason = "tests call things for their effect and ignore what they return"))]

/// The clients that talk to the outside world.
pub mod adapters;
/// Starting the app.
pub mod bootstrap;
/// The system clock.
pub mod clock;
/// The records and rules that hold without a runtime.
pub mod domain;
/// The Harness: the Sessions and what they share.
pub mod harness;
/// Notices for the user.
pub mod notice;
/// Where db constructs live
pub mod persistence;
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

extern crate alloc;

pub const APP_NAME: &str = "thoth";
