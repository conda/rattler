//! Functionality to stream and extract packages in an [`tokio`] async context.

#[cfg(not(target_arch = "wasm32"))]
mod shared;

pub mod async_read;
#[cfg(not(target_arch = "wasm32"))]
pub mod async_seek;
pub mod fs;

/// Buffer size for async I/O operations (128KB).
pub(crate) const DEFAULT_BUF_SIZE: usize = 128 * 1024;
