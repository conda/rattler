//! Shared path ownership and I/O limits for one package transaction.

use std::sync::Arc;

use parking_lot::{Mutex, MutexGuard};
use rattler_conda_types::PrefixRecord;
use simple_spawn_blocking::{Cancelled, tokio::run_blocking_task};
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};

use super::clobber_registry::ClobberRegistry;

/// Coordinates path ownership and asynchronous filesystem work across packages.
///
/// Share one context between all packages in a transaction, and create a fresh
/// context for each transaction. Use [`super::PreparedTransaction`] when applying
/// a transaction that also needs lifecycle hooks and clobber finalization.
/// Synchronous linking uses the shared path registry, but runs on Rayon rather
/// than acquiring asynchronous I/O permits.
pub struct TransactionLinkContext {
    io_concurrency_semaphore: Option<Arc<Semaphore>>,
    clobber_registry: Mutex<ClobberRegistry>,
}

impl Default for TransactionLinkContext {
    fn default() -> Self {
        Self {
            io_concurrency_semaphore: Some(Arc::new(Semaphore::new(100))),
            clobber_registry: Mutex::new(ClobberRegistry::default()),
        }
    }
}

impl TransactionLinkContext {
    /// Creates a context with a limit of 100 asynchronous filesystem operations.
    pub fn new() -> Self {
        Self::default()
    }

    /// Limits concurrent asynchronous filesystem operations.
    #[must_use]
    pub fn with_io_concurrency_limit(self, limit: usize) -> Self {
        self.with_io_concurrency_semaphore(Arc::new(Semaphore::new(limit)))
    }

    /// Shares an asynchronous filesystem-operation budget with other contexts.
    #[must_use]
    pub fn with_io_concurrency_semaphore(self, semaphore: Arc<Semaphore>) -> Self {
        Self {
            io_concurrency_semaphore: Some(semaphore),
            ..self
        }
    }

    /// Allows asynchronous filesystem operations without a semaphore limit.
    #[must_use]
    pub fn without_io_concurrency_limit(self) -> Self {
        Self {
            io_concurrency_semaphore: None,
            ..self
        }
    }

    /// Initializes path ownership from packages already installed in the prefix.
    #[must_use]
    pub fn with_prefix_records<'a>(
        self,
        prefix_records: impl IntoIterator<Item = &'a PrefixRecord>,
    ) -> Self {
        Self {
            clobber_registry: Mutex::new(ClobberRegistry::new(prefix_records)),
            ..self
        }
    }

    /// Releases a package's path ownership before its files are unlinked.
    pub fn unregister_paths(&self, prefix_record: &PrefixRecord) {
        self.clobber_registry().unregister_paths(prefix_record);
    }

    pub(crate) fn clobber_registry(&self) -> MutexGuard<'_, ClobberRegistry> {
        self.clobber_registry.lock()
    }

    /// Acquires permission for asynchronous filesystem work, or `None` if unlimited.
    pub async fn acquire_io_permit(&self) -> Result<Option<OwnedSemaphorePermit>, AcquireError> {
        match &self.io_concurrency_semaphore {
            Some(semaphore) => semaphore.clone().acquire_owned().await.map(Some),
            None => Ok(None),
        }
    }

    /// Runs blocking filesystem work while retaining its permit until completion.
    pub async fn run_blocking_io_task<
        T: Send + 'static,
        E: Send + From<Cancelled> + 'static,
        F: FnOnce() -> Result<T, E> + Send + 'static,
    >(
        &self,
        function: F,
    ) -> Result<T, E> {
        let permit = self.acquire_io_permit().await.map_err(|_error| Cancelled)?;
        run_blocking_task(move || {
            let _permit = permit;
            function()
        })
        .await
    }
}
