use super::clobber_registry::ClobberRegistry;
use super::link_script::PrePostLinkResult;
use super::unlink::{recursively_remove_empty_directories, UnlinkError};
use super::{InstallError, Transaction};
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use indexmap::IndexSet;
use itertools::Itertools;
use rattler_conda_types::prefix_record::PathType;
use rattler_conda_types::{PackageRecord, PrefixRecord, RepoDataRecord};
use std::collections::HashSet;
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
    execute_link_scripts: bool,
}

struct InstallDriverInner {
    tx: UnboundedSender<Task>,
    join_handle: JoinHandle<()>,
}

type Task = Box<dyn FnOnce() + Send + 'static>;

impl Default for InstallDriver {
    fn default() -> Self {
        Self::new(100, None, true)
    }
}

impl InstallDriver {
    /// Constructs a new [`InstallDriver`] with a given maximum number of concurrent tasks. This is
    /// the number of tasks spawned through the driver that can run concurrently. This is especially
    /// useful to make sure no filesystem limits are encountered.
    pub fn new(
        concurrency_limit: usize,
        prefix_records: Option<&[PrefixRecord]>,
        execute_link_scripts: bool,
    ) -> Self {
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

        let clobber_registry = prefix_records
            .map(ClobberRegistry::from_prefix_records)
            .unwrap_or_default();

        Self {
            inner: Arc::new(Mutex::new(InstallDriverInner { tx, join_handle })),
            concurrency_limit,
            clobber_registry: Arc::new(Mutex::new(clobber_registry)),
            execute_link_scripts,
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

    /// Call this before any packages are installed to perform any pre processing that is required.
    pub fn pre_process(
        &self,
        transaction: &Transaction<PrefixRecord, RepoDataRecord>,
        target_prefix: &Path,
    ) -> Result<Option<PrePostLinkResult>, InstallError> {
        if self.execute_link_scripts {
            match self.run_pre_unlink_scripts(transaction, target_prefix) {
                Ok(res) => {
                    return Ok(Some(res));
                }
                Err(e) => {
                    tracing::error!("Error running pre-unlink scripts: {:?}", e);
                }
            }
        }

        Ok(None)
    }

    /// Call this after all packages have been installed to perform any post processing that is
    /// required.
    ///
    /// This function will select a winner among multiple packages that might write to a single package
    /// and will also execute any `post-link.sh/bat` scripts
    pub fn post_process(
        &self,
        transaction: &Transaction<PrefixRecord, RepoDataRecord>,
        target_prefix: &Path,
    ) -> Result<Option<PrePostLinkResult>, InstallError> {
        let prefix_records = PrefixRecord::collect_from_prefix(target_prefix)
            .map_err(InstallError::PostProcessFailed)?;

        let required_packages =
            PackageRecord::sort_topologically(prefix_records.iter().collect::<Vec<_>>());

        self.remove_empty_directories(transaction, &prefix_records, target_prefix)
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to remove empty directories: {} (ignored)", e);
            });

        self.clobber_registry()
            .unclobber(&required_packages, target_prefix)
            .unwrap_or_else(|e| {
                tracing::error!("Error unclobbering packages: {:?}", e);
            });

        if self.execute_link_scripts {
            match self.run_post_link_scripts(transaction, &required_packages, target_prefix) {
                Ok(res) => {
                    return Ok(Some(res));
                }
                Err(e) => {
                    tracing::error!("Error running post-link scripts: {:?}", e);
                }
            }
        }

        Ok(None)
    }

    /// Remove all empty directories that are not part of the new prefix records.
    pub fn remove_empty_directories(
        &self,
        transaction: &Transaction<PrefixRecord, RepoDataRecord>,
        new_prefix_records: &[PrefixRecord],
        target_prefix: &Path,
    ) -> Result<(), UnlinkError> {
        let mut keep_directories = HashSet::new();

        // find all forced directories in the prefix records
        for record in new_prefix_records {
            for paths in record.paths_data.paths.iter() {
                if paths.path_type == PathType::Directory {
                    let path = target_prefix.join(&paths.relative_path);
                    keep_directories.insert(path);
                }
            }
        }

        // find all removed directories
        for record in transaction.removed_packages() {
            let mut removed_directories = HashSet::new();

            for paths in record.paths_data.paths.iter() {
                if paths.path_type != PathType::Directory {
                    if let Some(parent) = paths.relative_path.parent() {
                        removed_directories.insert(parent);
                    }
                }
            }

            let is_python_noarch = record.repodata_record.package_record.noarch.is_python();

            // Sort the directories by length, so that we delete the deepest directories first.
            let mut directories: IndexSet<_> = removed_directories.into_iter().sorted().collect();

            while let Some(directory) = directories.pop() {
                let directory_path = target_prefix.join(directory);
                let removed_until = recursively_remove_empty_directories(
                    &directory_path,
                    target_prefix,
                    is_python_noarch,
                    &keep_directories,
                )?;

                // The directory is not empty which means our parent directory is also not empty,
                // recursively remove the parent directory from the set as well.
                while let Some(parent) = removed_until.parent() {
                    if !directories.shift_remove(parent) {
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

impl Drop for InstallDriverInner {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
