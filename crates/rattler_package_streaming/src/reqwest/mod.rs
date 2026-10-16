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
/// One connection to S3 carries about 100 MB/s, so hosts with fast networks
/// may want more parts in flight.
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
    /// The default number of ranges requested at once, the same as rattler's
    /// default limit on concurrent downloads, so as many requests are in
    /// flight as before ranged downloads.
    pub const DEFAULT_PARTS: usize = 50;

    /// Download packages in ranges of `part_size` bytes, with at most `parts`
    /// requested at once across all packages downloaded with these options.
    ///
    /// A `part_size` of 0 disables ranged downloads: every package is fetched
    /// with a single request. A `parts` of 0 is raised to 1.
    ///
    /// Ranges that finish ahead of an earlier one are held in memory, so a
    /// download buffers at most `(parts + 1) * part_size` bytes.
    pub fn new(part_size: u64, parts: usize) -> Self {
        let parts = parts.max(1);
        Self {
            part_size,
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
