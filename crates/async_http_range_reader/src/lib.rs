pub use crate::async_http_range_reader::{AsyncHttpRangeReader, AsyncHttpRangeReaderError, CheckSupportMethod};

pub mod async_http_range_reader;
pub mod async_http_range_reader_error;

mod sparse_range;

#[cfg(test)]
mod static_directory_server;
