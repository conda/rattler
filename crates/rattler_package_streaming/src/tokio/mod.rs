//! Functionality to stream and extract packages in an [`tokio`] async context.

mod shared;

pub mod async_read;
pub mod async_seek;
pub mod fs;
