use crate::install::Transaction;
use rattler_conda_types::{PrefixRecord, RepoDataRecord};

/// A trait for reporting progress of the installation process.
pub trait Reporter: Send + Sync {
    /// Called when the transaction starts
    fn on_transaction_start(&self, transaction: &Transaction<PrefixRecord, RepoDataRecord>);

    /// Called when a transaction operation starts
    fn on_transaction_operation_start(&self, operation: usize);

    /// Called when starting to populate the cache for a package
    fn on_populate_cache_start(&self, operation: usize, record: &RepoDataRecord) -> usize;

    /// Called when validation of a package starts
    fn on_validate_start(&self, cache_entry: usize) -> usize;
    /// Called when validation completex
    fn on_validate_complete(&self, validate_idx: usize);

    /// Called when a download starts
    fn on_download_start(&self, cache_entry: usize) -> usize;
    /// Called with regular updates on the download progress
    fn on_download_progress(&self, download_idx: usize, progress: u64, total: Option<u64>);
    /// Called when a download completes
    fn on_download_completed(&self, download_idx: usize);

    /// Called when starting to populate the cache for a package
    fn on_populate_cache_complete(&self, cache_entry: usize);

    /// Called when an unlink operation started.
    fn on_unlink_start(&self, operation: usize, record: &PrefixRecord) -> usize;

    /// Called when an unlink operation started.
    fn on_unlink_complete(&self, index: usize);

    /// Called when linking of a package has started
    fn on_link_start(&self, operation: usize, record: &RepoDataRecord) -> usize;

    /// Called when linking of a package compelted.
    fn on_link_complete(&self, index: usize);

    /// Called when a transaction operation finishes
    fn on_transaction_operation_complete(&self, operation: usize);

    /// Called when the transaction completes
    fn on_transaction_complete(&self);
}
