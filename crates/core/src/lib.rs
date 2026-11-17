//! Thoth core: Agents, Tools, Sessions and the Harness that owns them.

#![cfg_attr(test, expect(unused_results, reason = "tests call things for their effect and ignore what they return"))]

/// The clients that talk to the outside world.
pub mod adapters;
/// The use cases: the Harness, its Sessions and the things they drive.
pub mod application;
/// The system clock.
pub mod clock;
/// Starting the app and the services it is built from.
pub mod composition;
/// The records and rules that hold without a runtime.
pub mod domain;
/// Notices for the user.
pub(crate) mod notice;
/// Where db constructs live
pub(crate) mod persistence;
/// The Agent, its Turns and the tasks they run on.
pub mod runtime;
pub(crate) mod sync;
#[cfg(any(test, feature = "testing"))]
#[expect(clippy::unwrap_used, reason = "test support: a failed setup should fail the test")]
pub mod test;
/// Small shared helpers.
pub(crate) mod utils;

extern crate alloc;

pub const APP_NAME: &str = "thoth";
