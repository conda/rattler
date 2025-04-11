use rattler_conda_types::{PrefixRecord, RepoDataRecord};

use crate::install::Transaction;

/// A trait for reporting progress of the installation process.
pub trait Reporter: Send + Sync {
    /// Called when the transaction starts. This is the first method called.
    fn on_transaction_start(&self, transaction: &Transaction<PrefixRecord, RepoDataRecord>);

    /// Called when a transaction operation starts. During the operation the
    /// cache is populated, previous installations are uninstalled and new
    /// installations are linked.
    ///
    /// The `operation` is the index of the operation in the transaction passed
    /// to `on_transaction_start`.
    fn on_transaction_operation_start(&self, operation: usize);

    /// Called when starting to populate the cache for a package. This is called
    /// for any new package that will be installed. Before installation any
    /// package is first added to the cache.
    ///
    /// The `operation` is the index of the operation in the transaction passed
    /// to `on_transaction_start`.
    fn on_populate_cache_start(&self, operation: usize, record: &RepoDataRecord) -> usize;

    /// Called when validation of a package starts. If a package is already
    /// present in the cache, the contents of the package is validated
    /// against its manifest, this is done to ensure that the package is not
    /// corrupted.
    fn on_validate_start(&self, cache_entry: usize) -> usize;

    /// Called when validation completes. If the package is valid, the package
    /// is immediately used and no downloading is required.
    ///
    /// The `validate_idx` is the value return by `on_validate_start` for the
    /// corresponding package.
    fn on_validate_complete(&self, validate_idx: usize);

    /// Called when a download starts. If a package is not present in the cache
    /// or the package in the cache is corrupt, the package is downloaded. This
    /// function is called right before that happens.
    ///
    /// The value returned by this function is passed as the `download_idx` to
    /// `on_download_progress` and `on_download_complete`.
    ///
    /// The `cache_entry` is the value return by `on_populate_cache_start` for
    /// the corresponding package.
    fn on_download_start(&self, cache_entry: usize) -> usize;

    /// Called with regular updates on the download progress.
    ///
    /// The `download_idx` is the value return by `on_download_start` for the
    /// corresponding download.
    fn on_download_progress(&self, download_idx: usize, progress: u64, total: Option<u64>);

    /// Called when a download completes.
    ///
    /// The `download_idx` is the value return by `on_download_start` for the
    /// corresponding download.
    fn on_download_completed(&self, download_idx: usize);

    /// Called when the cache for a package was populated
    ///
    /// The `cache_entry` is the value return by `on_populate_cache_start` for
    /// the corresponding package.
    fn on_populate_cache_complete(&self, cache_entry: usize);

    /// Called when an unlink operation started.
    ///
    /// The `operation` is the index of the operation in the transaction passed
    /// to `on_transaction_start`.
    fn on_unlink_start(&self, operation: usize, record: &PrefixRecord) -> usize;

    /// Called when an unlink operation completed.
    ///
    /// The `index` is the value return by `on_unlink_start` for the
    /// corresponding package.
    fn on_unlink_complete(&self, index: usize);

    /// Called when linking of a package has started
    ///
    /// The `operation` is the index of the operation in the transaction passed
    /// to `on_transaction_start`.
    fn on_link_start(&self, operation: usize, record: &RepoDataRecord) -> usize;

    /// Called when linking of a package completed.
    ///
    /// The `index` is the value return by `on_link_start` for the corresponding
    /// package.
    fn on_link_complete(&self, index: usize);

    /// Called when a transaction operation finishes.
    fn on_transaction_operation_complete(&self, operation: usize);

    /// Called when the transaction completes. Unless an error occurs, this is
    /// the last function that is called.
    fn on_transaction_complete(&self);
}
