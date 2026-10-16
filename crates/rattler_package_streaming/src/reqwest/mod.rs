//! Functionality to stream and extract packages directly from a [`reqwest::Url`].
#[cfg(not(target_arch = "wasm32"))]
pub mod fetch;
pub mod full_download;
#[cfg(not(target_arch = "wasm32"))]
mod ranged;
#[cfg(not(target_arch = "wasm32"))]
pub mod sparse;
pub mod tokio;

#[cfg(test)]
pub(crate) mod test_server;

use std::sync::{Arc, LazyLock};

use ::tokio::sync::Semaphore;

/// How a large package is downloaded as byte ranges fetched in parallel.
///
/// The defaults match the AWS transfer libraries. Hosts with fast networks
/// can raise them: one connection to S3 carries about 100 MB/s.
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub(crate) part_size: u64,
    pub(crate) parts: usize,
    /// Shared by every download that uses these options or a clone of them.
    pub(crate) parts_in_flight: Arc<Semaphore>,
}

impl DownloadOptions {
    /// The default number of bytes requested per range, as in `s3transfer`.
    pub const DEFAULT_PART_SIZE: u64 = 8 * 1024 * 1024;
    /// The default number of ranges requested at once, as in `s3transfer`.
    pub const DEFAULT_PARTS: usize = 10;

    /// Download packages in ranges of `part_size` bytes, with at most `parts`
    /// requested at once across all packages downloaded with these options.
    /// Zeros are raised to one.
    ///
    /// Ranges that finish ahead of an earlier one are held in memory, so a
    /// download buffers at most `(parts + 1) * part_size` bytes.
    pub fn new(part_size: u64, parts: usize) -> Self {
        let parts = parts.max(1);
        Self {
            part_size: part_size.max(1),
            parts,
            parts_in_flight: Arc::new(Semaphore::new(parts)),
        }
    }
}

impl Default for DownloadOptions {
    /// The default options, whose limit is shared by the whole process.
    fn default() -> Self {
        static DEFAULT: LazyLock<DownloadOptions> = LazyLock::new(|| {
            DownloadOptions::new(
                DownloadOptions::DEFAULT_PART_SIZE,
                DownloadOptions::DEFAULT_PARTS,
            )
        });
        DEFAULT.clone()
    }
}
