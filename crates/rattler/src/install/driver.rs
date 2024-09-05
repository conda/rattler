use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use indexmap::IndexSet;
use itertools::Itertools;
use rattler_conda_types::{prefix_record::PathType, PackageRecord, PrefixRecord};
use simple_spawn_blocking::{tokio::run_blocking_task, Cancelled};
use thiserror::Error;
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};

use super::{
    clobber_registry::{ClobberError, ClobberRegistry, ClobberedPath},
    link_script::{PrePostLinkError, PrePostLinkResult},
    unlink::{recursively_remove_empty_directories, UnlinkError},
    Transaction,
};
use crate::install::link_script::LinkScriptError;

/// Packages can mostly be installed in isolation and therefore in parallel.
/// However, when installing a large number of packages at the same time the
/// different installation tasks start competing for resources. The
/// [`InstallDriver`] helps to assist in making sure that tasks don't starve
/// each other from resource as well as making sure that due to the large number
/// of requests the process doesn't try to acquire more resources than the
/// system has available.
pub struct InstallDriver {
    io_concurrency_semaphore: Option<Arc<Semaphore>>,
    clobber_registry: Arc<Mutex<ClobberRegistry>>,
    execute_link_scripts: bool,
}

impl Default for InstallDriver {
    fn default() -> Self {
        Self::builder()
            .execute_link_scripts(false)
            .with_io_concurrency_limit(100)
            .finish()
    }
}

/// A builder to configure a new `InstallDriver`.
#[derive(Debug, Default)]
pub struct InstallDriverBuilder {
    io_concurrency_semaphore: Option<Arc<Semaphore>>,
    clobber_registry: Option<ClobberRegistry>,
    execute_link_scripts: bool,
}

/// The result of the post-processing step.
#[derive(Debug)]
pub struct PostProcessResult {
    /// The result of running the post link scripts. This is only present if
    /// running the scripts is allowed.
    pub post_link_result: Option<Result<PrePostLinkResult, LinkScriptError>>,

    /// The paths that were clobbered during the installation process.
    pub clobbered_paths: HashMap<PathBuf, ClobberedPath>,
}

/// An error that might have occurred during post-processing
#[derive(Debug, Error)]
pub enum PostProcessingError {
    #[error("failed to unclobber clobbered files")]
    ClobberError(#[from] ClobberError),

    /// Failed to determine the currently installed packages.
    #[error("failed to determine the installed packages")]
    FailedToDetectInstalledPackages(#[source] std::io::Error),
}

impl InstallDriverBuilder {
    /// Sets an optional IO concurrency limit. This is used to make sure
    /// that the system doesn't acquire more IO resources than the system has
    /// available.
    pub fn with_io_concurrency_limit(self, limit: usize) -> Self {
        Self {
            io_concurrency_semaphore: Some(Arc::new(Semaphore::new(limit))),
            ..self
        }
    }

    /// Sets an optional IO concurrency semaphore. This is used to make sure
    /// that the system doesn't acquire more IO resources than the system has
    /// available.
    pub fn with_io_concurrency_semaphore(self, io_concurrency_semaphore: Arc<Semaphore>) -> Self {
        Self {
            io_concurrency_semaphore: Some(io_concurrency_semaphore),
            ..self
        }
    }

    /// Sets the prefix records that are present in the current environment.
    /// This is used to initialize the clobber registry.
    pub fn with_prefix_records<'i>(
        self,
        prefix_records: impl IntoIterator<Item = &'i PrefixRecord>,
    ) -> Self {
        Self {
            clobber_registry: Some(ClobberRegistry::new(prefix_records)),
            ..self
        }
    }

    /// Sets whether to execute link scripts or not.
    pub fn execute_link_scripts(self, execute_link_scripts: bool) -> Self {
        Self {
            execute_link_scripts,
            ..self
        }
    }

    pub fn finish(self) -> InstallDriver {
        InstallDriver {
            io_concurrency_semaphore: self.io_concurrency_semaphore,
            clobber_registry: self
                .clobber_registry
                .map(Mutex::new)
                .map(Arc::new)
                .unwrap_or_default(),
            execute_link_scripts: self.execute_link_scripts,
        }
    }
}

impl InstallDriver {
    /// Constructs a builder to configure a new `InstallDriver`.
    pub fn builder() -> InstallDriverBuilder {
        InstallDriverBuilder::default()
    }

    /// Returns a permit that will allow the caller to perform IO operations.
    /// This is used to make sure that the system doesn't try to acquire
    /// more IO resources than the system has available.
    pub async fn acquire_io_permit(&self) -> Result<Option<OwnedSemaphorePermit>, AcquireError> {
        match self.io_concurrency_semaphore.clone() {
            None => Ok(None),
            Some(semaphore) => Ok(Some(semaphore.acquire_owned().await?)),
        }
    }

    /// Return a locked reference to the paths registry. This is used to make
    /// sure that the same path is not installed twice.
    pub fn clobber_registry(&self) -> MutexGuard<'_, ClobberRegistry> {
        self.clobber_registry.lock().unwrap()
    }

    /// Call this before any packages are installed to perform any pre
    /// processing that is required.
    pub fn pre_process<Old: Borrow<PrefixRecord>, New>(
        &self,
        transaction: &Transaction<Old, New>,
        target_prefix: &Path,
    ) -> Result<Option<PrePostLinkResult>, PrePostLinkError> {
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

    /// Runs a blocking task that will execute on a separate thread. The task is
    /// not started until an IO permit is acquired. This is used to make
    /// sure that the system doesn't try to acquire more IO resources than
    /// the system has available.
    pub async fn run_blocking_io_task<
        T: Send + 'static,
        E: Send + From<Cancelled> + 'static,
        F: FnOnce() -> Result<T, E> + Send + 'static,
    >(
        &self,
        f: F,
    ) -> Result<T, E> {
        let permit = self.acquire_io_permit().await.map_err(|_err| Cancelled)?;

        run_blocking_task(move || {
            let _permit = permit;
            f()
        })
        .await
    }

    /// Call this after all packages have been installed to perform any post
    /// processing that is required.
    ///
    /// This function will select a winner among multiple packages that might
    /// write to a single package and will also execute any
    /// `post-link.sh/bat` scripts
    pub fn post_process<Old: Borrow<PrefixRecord> + AsRef<New>, New: AsRef<PackageRecord>>(
        &self,
        transaction: &Transaction<Old, New>,
        target_prefix: &Path,
    ) -> Result<PostProcessResult, PostProcessingError> {
        let prefix_records = PrefixRecord::collect_from_prefix(target_prefix)
            .map_err(PostProcessingError::FailedToDetectInstalledPackages)?;

        let required_packages =
            PackageRecord::sort_topologically(prefix_records.iter().collect::<Vec<_>>());

        self.remove_empty_directories(transaction, &prefix_records, target_prefix)
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to remove empty directories: {} (ignored)", e);
            });

        let clobbered_paths = self
            .clobber_registry()
            .unclobber(&required_packages, target_prefix)?;

        let post_link_result = if self.execute_link_scripts {
            Some(self.run_post_link_scripts(transaction, &required_packages, target_prefix))
        } else {
            None
        };

        Ok(PostProcessResult {
            post_link_result,
            clobbered_paths,
        })
    }

    /// Remove all empty directories that are not part of the new prefix
    /// records.
    pub fn remove_empty_directories<Old: Borrow<PrefixRecord>, New>(
        &self,
        transaction: &Transaction<Old, New>,
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
        for record in transaction.removed_packages().map(Borrow::borrow) {
            let mut removed_directories = HashSet::new();

            for paths in record.paths_data.paths.iter() {
                if paths.path_type != PathType::Directory {
                    if let Some(parent) = paths.relative_path.parent() {
                        removed_directories.insert(parent);
                    }
                }
            }

            let is_python_noarch = record.repodata_record.package_record.noarch.is_python();

            // Sort the directories by length, so that we delete the deepest directories
            // first.
            let mut directories: IndexSet<_> = removed_directories.into_iter().sorted().collect();

            while let Some(directory) = directories.pop() {
                let directory_path = target_prefix.join(directory);
                let removed_until = recursively_remove_empty_directories(
                    &directory_path,
                    target_prefix,
                    is_python_noarch,
                    &keep_directories,
                )?;

                // The directory is not empty which means our parent directory is also not
                // empty, recursively remove the parent directory from the set
                // as well.
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
