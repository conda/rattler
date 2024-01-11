use super::clobber_registry::ClobberRegistry;
use super::InstallError;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use rattler_conda_types::{PackageRecord, PrefixRecord};
use std::future::pending;
use std::path::Path;
use std::sync::MutexGuard;
use std::sync::{Arc, Mutex};
use tokio::{
    select,
    sync::mpsc::{unbounded_channel, UnboundedSender},
    sync::oneshot,
    task::JoinHandle,
};

/// Packages can mostly be installed in isolation and therefor in parallel. However, when installing
/// a large number of packages at the same time the different installation tasks start competing for
/// resources. The [`InstallDriver`] helps to assist in making sure that tasks don't starve
/// each other from resource as well as making sure that due to the large number of requests the
/// process doesn't try to acquire more resources than the system has available.
pub struct InstallDriver {
    inner: Arc<Mutex<InstallDriverInner>>,
    concurrency_limit: usize,
    clobber_registry: Arc<Mutex<ClobberRegistry>>,
}

struct InstallDriverInner {
    tx: UnboundedSender<Task>,
    join_handle: JoinHandle<()>,
}

type Task = Box<dyn FnOnce() + Send + 'static>;

impl Default for InstallDriver {
    fn default() -> Self {
        Self::new(100, None)
    }
}

impl InstallDriver {
    /// Constructs a new [`InstallDriver`] with a given maximum number of concurrent tasks. This is
    /// the number of tasks spawned through the driver that can run concurrently. This is especially
    /// useful to make sure no filesystem limits are encountered.
    pub fn new(concurrency_limit: usize, prefix_records: Option<&[PrefixRecord]>) -> Self {
        let (tx, mut rx) = unbounded_channel::<Task>();
        let join_handle = tokio::spawn(async move {
            let mut pending_futures = FuturesUnordered::new();
            loop {
                // Build a future to receive a new task to execute, or do not accept new tasks
                // if the current concurrency limit is reached.
                let next_task = if pending_futures.len() < concurrency_limit {
                    rx.recv().left_future()
                } else {
                    pending().right_future()
                };

                // Wait for a new tasks or on of the futures that finishes.
                select! {
                    task = next_task => {match task {
                        Some(task) => {
                            pending_futures.push(tokio::task::spawn_blocking(task));
                        }
                        None => {
                            // The sender closed, this means the outer struct was dropped, which
                            // means we can stop as well.
                            break;
                        }
                    }},
                    Some(result) = pending_futures.next() => {if let Err(err) = result {
                        // If a panic occurred in the blocking task we resume the error here to make sure
                        // its not getting lost.
                        if let Ok(panic) = err.try_into_panic() {
                            std::panic::resume_unwind(panic);
                        }

                        // Note: we don't handle the cancelled error here. This can be handled by a
                        // sender/receiver pair that get closed when the task drops.
                    }}
                }
            }
        });

        let clobber_registry = if let Some(prefix_records) = prefix_records {
            ClobberRegistry::from_prefix_records(prefix_records)
        } else {
            ClobberRegistry::default()
        };

        Self {
            inner: Arc::new(Mutex::new(InstallDriverInner { tx, join_handle })),
            concurrency_limit,
            clobber_registry: Arc::new(Mutex::new(clobber_registry)),
        }
    }

    /// Returns the number of tasks that can run in parallel.
    pub fn concurrency_limit(&self) -> usize {
        self.concurrency_limit
    }

    /// Spawns a blocking operation on another thread and waits for it to complete. This is similar
    /// to calling [`tokio::task::spawn_blocking`] except that the number of concurrent tasks is
    /// limited. This is especially useful when performing filesystem operations because most
    /// platforms have a limit on the number of concurrent filesystem operations.
    pub async fn spawn_throttled<
        R: Send + 'static,
        F: FnOnce() -> Result<R, InstallError> + Send + 'static,
    >(
        &self,
        f: F,
    ) -> Result<R, InstallError> {
        let (tx, rx) = oneshot::channel();

        // Spawn the task on the background
        self.spawn_throttled_and_forget(move || {
            if !tx.is_closed() {
                let result = f();
                let _ = tx.send(result);
            }
        });

        // Await the result being send back. If an error occurs during receive it means that the
        // sending end of the channel was closed. This can only really happen when the task has been
        // dropped. We assume that that means the task has been cancelled.
        rx.await.map_err(|_err| InstallError::Cancelled)?
    }

    /// Spawns a blocking operation on another thread but does not wait for it to complete. This is
    /// similar to calling [`tokio::task::spawn_blocking`] except that the number of concurrent
    /// tasks is limited. This is especially useful when performing filesystem operations because
    /// most platforms have a limit on the number of concurrent filesystem operations.
    pub fn spawn_throttled_and_forget<F: FnOnce() + Send + 'static>(&self, f: F) {
        let inner = self.inner.lock().unwrap();
        if inner.tx.send(Box::new(f)).is_err() {
            unreachable!(
                "if a send error occurs here it means the task processor is dropped. \
                Since this only happens when dropping this object there cannot be another call to \
                this function. Therefor this should never happen."
            );
        }
    }

    /// Return a locked reference to the paths registry. This is used to make sure that the same
    /// path is not installed twice.
    pub fn clobber_registry(&self) -> MutexGuard<'_, ClobberRegistry> {
        self.clobber_registry.lock().unwrap()
    }

    /// Call this after all packages have been installed to perform any post processing that is
    /// required.
    ///
    /// This function will select a winner among multiple packages that might write to a single package
    /// and will also execute any `post-link.sh/bat` scripts
    pub fn post_process(
        &self,
        prefix_records: &[PrefixRecord],
        target_prefix: &Path,
    ) -> Result<(), InstallError> {
        let required_packages =
            PackageRecord::sort_topologically(prefix_records.iter().collect::<Vec<_>>());

        self.clobber_registry()
            .post_process(&required_packages, target_prefix)
            .map_err(|e| InstallError::PostProcessFailed(e))?;

        Ok(())
    }
}

impl Drop for InstallDriverInner {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
