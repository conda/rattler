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
    pub(crate) parts_per_download: usize,
    /// Shared by every download that uses these options or a clone of them.
    pub(crate) parts: Arc<Semaphore>,
}

impl DownloadOptions {
    /// The default number of bytes requested per range, as in `s3transfer`.
    pub const DEFAULT_PART_SIZE: u64 = 8 * 1024 * 1024;
    /// The default number of ranges requested at once for one package, as in
    /// `s3transfer`.
    pub const DEFAULT_PARTS_PER_DOWNLOAD: usize = 10;
    /// The default number of ranges requested at once across all packages,
    /// the floor the AWS Rust transfer manager uses.
    pub const DEFAULT_PARTS_TOTAL: usize = 32;

    /// Download packages in ranges of `part_size` bytes, at most
    /// `parts_per_download` at once per package and `parts_total` at once
    /// across all packages downloaded with these options. Zeros are raised to
    /// one.
    ///
    /// Ranges that finish ahead of an earlier one are held in memory, so a
    /// download buffers at most `(parts_per_download + 1) * part_size` bytes.
    pub fn new(part_size: u64, parts_per_download: usize, parts_total: usize) -> Self {
        Self {
            part_size: part_size.max(1),
            parts_per_download: parts_per_download.max(1),
            parts: Arc::new(Semaphore::new(parts_total.max(1))),
        }
    }
}

impl Default for DownloadOptions {
    /// The default options, whose total limit is shared by the whole process.
    fn default() -> Self {
        static DEFAULT: LazyLock<DownloadOptions> = LazyLock::new(|| {
            DownloadOptions::new(
                DownloadOptions::DEFAULT_PART_SIZE,
                DownloadOptions::DEFAULT_PARTS_PER_DOWNLOAD,
                DownloadOptions::DEFAULT_PARTS_TOTAL,
            )
        });
        DEFAULT.clone()
    }
}
