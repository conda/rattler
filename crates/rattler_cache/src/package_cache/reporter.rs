/// A trait that can be implemented to report progress of the download and
/// validation process.
pub trait CacheReporter: Send + Sync {
    /// Called when validation starts
    fn on_validate_start(&self) -> usize;
    /// Called when validation completex
    fn on_validate_complete(&self, index: usize);
    /// Called when a download starts
    fn on_download_start(&self) -> usize;
    /// Called with regular updates on the download progress
    fn on_download_progress(&self, index: usize, progress: u64, total: Option<u64>);
    /// Called when a download completes
    fn on_download_completed(&self, index: usize);
}
