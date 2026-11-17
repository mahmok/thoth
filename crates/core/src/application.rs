//! The use cases: the Harness, its Sessions and the things they drive.

/// The Harness: the Sessions and what they share.
pub mod harness;
/// A login attempt in progress.
pub mod login;
/// Refreshing the models each provider offers.
pub mod model_refresh;
/// Ports the application uses to drive the runtime.
pub mod ports;
/// A Session: one conversation and its view.
pub mod session;
