//! Python bindings for rattler_pty
//!
//! This module provides Python access to pseudoterminal (PTY) functionality.
//! It closely mirrors the Rust rattler_pty API structure.
//!
//! ## Module Structure
//! - `pty_process` - Lower-level PTY process control
//! - `pty_session` - Higher-level interactive session management
//!
//! ## Platform Support
//! This module is only available when the `pty` feature is enabled.
//! The underlying `rattler_pty` crate is Unix-only, so this will only
//! work on Linux and macOS.

mod pty_process;
mod pty_session;

pub use pty_process::{PyPtyProcess, PyPtyProcessOptions};
pub use pty_session::PyPtySession;
