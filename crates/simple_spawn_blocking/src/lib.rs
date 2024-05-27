//! A simpel crate that makes it more ergonomic to spawn blocking tasks and
//! await their completion.

#[cfg(feature = "tokio")]
pub mod tokio;

/// A marker type that is used to signal that a task was cancelled.
pub struct Cancelled;
