mod error;
#[cfg(feature = "indicatif")]
mod indicatif;
mod reporter;
pub(crate) mod result_record;

use std::{
    collections::{HashMap, HashSet},
    future::ready,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use super::{
    unlink_package, AppleCodeSignBehavior, InstallDriver, InstallOptions, Prefix, Transaction,
};
use crate::install::installer::result_record::InstallationResultRecord;
use crate::{
    default_cache_dir,
    install::{
        clobber_registry::ClobberedPath,
        link_script::{LinkScriptError, PrePostLinkResult},
    },
    package_cache::PackageCache,
};
pub use error::InstallerError;
use futures::{stream::FuturesUnordered, FutureExt, StreamExt, TryFutureExt};
#[cfg(feature = "indicatif")]
pub use indicatif::{
    DefaultProgressFormatter, IndicatifReporter, IndicatifReporterBuilder, Placement,
    ProgressFormatter,
};
use itertools::Itertools;
use rattler_cache::package_cache::{CacheLock, CacheReporter};
use rattler_conda_types::{
    prefix_record::{Link, LinkType},
    MatchSpec, PackageName, PackageNameMatcher, Platform, PrefixRecord, RepoDataRecord,
};
use rattler_networking::retry_policies::default_retry_policy;
use rattler_networking::LazyClient;
use rayon::prelude::*;
pub use reporter::Reporter;
use simple_spawn_blocking::tokio::run_blocking_task;
use tokio::{sync::Semaphore, task::JoinError};

#[derive(Default)]
pub struct LinkOptions {
    pub allow_symbolic_links: Option<bool>,
    pub allow_hard_links: Option<bool>,
    pub allow_ref_links: Option<bool>,
}

/// An installer that can install packages into a prefix.
#[derive(Default)]
pub struct Installer {
    installed: Option<Vec<PrefixRecord>>,
    package_cache: Option<PackageCache>,
    downloader: Option<LazyClient>,
    execute_link_scripts: bool,
    io_semaphore: Option<Arc<Semaphore>>,
    reporter: Option<Arc<dyn Reporter>>,
    target_platform: Option<Platform>,
    apple_code_sign_behavior: AppleCodeSignBehavior,
    alternative_target_prefix: Option<PathBuf>,
    reinstall_packages: Option<HashSet<PackageName>>,
    ignored_packages: Option<HashSet<PackageName>>,
    requested_specs: Option<Vec<MatchSpec>>,
    // TODO: Determine upfront if these are possible.
    link_options: LinkOptions,
}

#[derive(Debug)]
pub struct InstallationResult {
    /// The transaction that was applied
    pub transaction: Transaction<InstallationResultRecord, RepoDataRecord>,

    /// The result of running pre link scripts. `None` if no
    /// pre-processing was performed, possibly because link scripts were
    /// disabled.
    pub pre_link_script_result: Option<PrePostLinkResult>,

    /// The result of running post link scripts. `None` if no
    /// post-processing was performed, possibly because link scripts were
    /// disabled.
    pub post_link_script_result: Option<Result<PrePostLinkResult, LinkScriptError>>,

    /// The paths that were clobbered during the installation process.
    pub clobbered_paths: HashMap<PathBuf, ClobberedPath>,
}

impl Installer {
    /// Constructs a new installer
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets an optional IO concurrency limit. This is used to make sure
    /// that the system doesn't acquire more IO resources than the system has
    /// available.
    #[must_use]
    pub fn with_io_concurrency_limit(self, limit: usize) -> Self {
        Self {
            io_semaphore: Some(Arc::new(Semaphore::new(limit))),
            ..self
        }
    }

    /// Sets an optional IO concurrency limit.
    ///
    /// This function is similar to [`Self::with_io_concurrency_limit`],
    /// but modifies an existing instance.
    pub fn set_io_concurrency_limit(&mut self, limit: usize) -> &mut Self {
        self.io_semaphore = Some(Arc::new(Semaphore::new(limit)));
        self
    }

    /// Sets an optional IO concurrency semaphore. This is used to make sure
    /// that the system doesn't acquire more IO resources than the system has
    /// available.
    #[must_use]
    pub fn with_io_concurrency_semaphore(self, io_concurrency_semaphore: Arc<Semaphore>) -> Self {
        Self {
            io_semaphore: Some(io_concurrency_semaphore),
            ..self
        }
    }

    /// Sets an optional IO concurrency semaphore.
    ///
    /// This function is similar to [`Self::with_io_concurrency_semaphore`], but
    /// modifies an existing instance.
    pub fn set_io_concurrency_semaphore(&mut self, limit: usize) -> &mut Self {
        self.io_semaphore = Some(Arc::new(Semaphore::new(limit)));
        self
    }

    /// Sets whether to execute link scripts or not.
    ///
    /// By default, link scripts are not executed. Link scripts can run
    /// arbitrary code during the installation phase which makes them a security
    /// risk.
    #[must_use]
    pub fn with_execute_link_scripts(self, execute: bool) -> Self {
        Self {
            execute_link_scripts: execute,
            ..self
        }
    }

    /// Sets whether to execute link scripts or not.
    ///
    /// By default, link scripts are not executed. Link scripts can run
    /// arbitrary code during the installation phase which makes them a security
    /// risk.
    pub fn set_execute_link_scripts(&mut self, execute: bool) -> &mut Self {
        self.execute_link_scripts = execute;
        self
    }

    /// Sets the package cache to use.
    #[must_use]
    pub fn with_package_cache(self, package_cache: PackageCache) -> Self {
        Self {
            package_cache: Some(package_cache),
            ..self
        }
    }

    /// Sets the package cache to use.
    ///
    /// This function is similar to [`Self::with_package_cache`],but modifies an
    /// existing instance.
    pub fn set_package_cache(&mut self, package_cache: PackageCache) -> &mut Self {
        self.package_cache = Some(package_cache);
        self
    }

    /// Sets the download client to use
    #[must_use]
    pub fn with_download_client(self, downloader: impl Into<LazyClient>) -> Self {
        Self {
            downloader: Some(downloader.into()),
            ..self
        }
    }

    /// Sets the download client to use
    ///
    /// This function is similar to [`Self::with_download_client`], but modifies
    /// an existing instance.
    pub fn set_download_client(&mut self, downloader: impl Into<LazyClient>) -> &mut Self {
        self.downloader = Some(downloader.into());
        self
    }

    /// Sets a reporter that will receive events during the installation
    /// process.
    #[must_use]
    pub fn with_reporter<R: Reporter + 'static>(self, reporter: R) -> Self {
        Self {
            reporter: Some(Arc::new(reporter)),
            ..self
        }
    }

    /// Sets a reporter that will receive events during the installation
    /// process.
    ///
    /// This function is similar to [`Self::with_reporter`],but modifies an
    /// existing instance.
    pub fn set_reporter<R: Reporter + 'static>(&mut self, reporter: R) -> &mut Self {
        self.reporter = Some(Arc::new(reporter));
        self
    }

    /// Sets the packages that are currently installed in the prefix. If this
    /// is not set, the installation process will first figure this out.
    #[must_use]
    pub fn with_installed_packages(self, installed: Vec<PrefixRecord>) -> Self {
        Self {
            installed: Some(installed),
            ..self
        }
    }

    /// Set the packages that we want explicitly to be reinstalled.
    #[must_use]
    pub fn with_reinstall_packages(self, reinstall: HashSet<PackageName>) -> Self {
        Self {
            reinstall_packages: Some(reinstall),
            ..self
        }
    }

    /// Set the packages that we want explicitly to be reinstalled.
    /// This function is similar to [`Self::with_reinstall_packages`],but
    /// modifies an existing instance.
    pub fn set_reinstall_packages(&mut self, reinstall: HashSet<PackageName>) -> &mut Self {
        self.reinstall_packages = Some(reinstall);
        self
    }

    /// Set the packages that should be ignored (left untouched) during installation.
    /// Ignored packages will not be removed, installed, or updated.
    #[must_use]
    pub fn with_ignored_packages(self, ignored: HashSet<PackageName>) -> Self {
        Self {
            ignored_packages: Some(ignored),
            ..self
        }
    }

    /// Set the packages that should be ignored (left untouched) during installation.
    /// Ignored packages will not be removed, installed, or updated.
    /// This function is similar to [`Self::with_ignored_packages`], but
    /// modifies an existing instance.
    pub fn set_ignored_packages(&mut self, ignored: HashSet<PackageName>) -> &mut Self {
        self.ignored_packages = Some(ignored);
        self
    }

    /// Sets the packages that are currently installed in the prefix. If this
    /// is not set, the installation process will first figure this out.
    ///
    /// This function is similar to [`Self::with_installed_packages`],but
    /// modifies an existing instance.
    pub fn set_installed_packages(&mut self, installed: Vec<PrefixRecord>) -> &mut Self {
        self.installed = Some(installed);
        self
    }

    /// Sets the target platform of the installation. If not specifically set
    /// this will default to the current platform.
    #[must_use]
    pub fn with_target_platform(self, target_platform: Platform) -> Self {
        Self {
            target_platform: Some(target_platform),
            ..self
        }
    }

    /// Sets the target platform of the installation. If not specifically set
    /// this will default to the current platform.
    ///
    /// This function is similar to [`Self::with_target_platform`], but modifies
    /// an existing instance.
    pub fn set_target_platform(&mut self, target_platform: Platform) -> &mut Self {
        self.target_platform = Some(target_platform);
        self
    }

    /// Determines how to handle Apple code signing behavior.
    #[must_use]
    pub fn with_apple_code_signing_behavior(self, behavior: AppleCodeSignBehavior) -> Self {
        Self {
            apple_code_sign_behavior: behavior,
            ..self
        }
    }

    /// Determines how to handle Apple code signing behavior.
    ///
    /// This function is similar to
    /// [`Self::with_apple_code_signing_behavior`],but modifies an existing
    /// instance.
    pub fn set_apple_code_signing_behavior(
        &mut self,
        behavior: AppleCodeSignBehavior,
    ) -> &mut Self {
        self.apple_code_sign_behavior = behavior;
        self
    }

    /// Sets the link options for the installer.
    pub fn with_link_options(self, options: LinkOptions) -> Self {
        Self {
            link_options: options,
            ..self
        }
    }

    /// Sets the link options for the installer.
    pub fn set_link_options(&mut self, options: LinkOptions) -> &mut Self {
        self.link_options = options;
        self
    }

    /// Sets the requested specs for the installer. These will be used to
    /// populate the `requested_spec` field in generated `PrefixRecord`
    /// instances.
    #[must_use]
    pub fn with_requested_specs(self, specs: Vec<MatchSpec>) -> Self {
        Self {
            requested_specs: Some(specs),
            ..self
        }
    }

    /// Sets the requested specs for the installer. These will be used to
    /// populate the `requested_spec` field in generated `PrefixRecord`
    /// instances.
    pub fn set_requested_specs(&mut self, specs: Vec<MatchSpec>) -> &mut Self {
        self.requested_specs = Some(specs);
        self
    }

    /// Install the packages in the given prefix.
    pub async fn install(
        self,
        prefix: impl AsRef<Path>,
        records: impl IntoIterator<Item = RepoDataRecord>,
    ) -> Result<InstallationResult, InstallerError> {
        let prefix = Prefix::create(prefix.as_ref().to_path_buf()).map_err(|err| {
            InstallerError::FailedToCreatePrefix(prefix.as_ref().to_path_buf(), err)
        })?;

        // Create a future to determine the currently installed packages. We
        // can start this in parallel with the other operations and resolve it
        // when we need it.
        let installed_provided = self.installed.is_some();
        let mut installed: Vec<InstallationResultRecord> = if let Some(installed) = self.installed {
            installed
                .into_iter()
                .map(InstallationResultRecord::Max)
                .collect()
        } else {
            let prefix = prefix.clone();
            // Use sparse collection for much faster reading when checking if packages changed
            run_blocking_task(move || {
                use rattler_conda_types::MinimalPrefixCollection;
                PrefixRecord::collect_minimal_from_prefix(&prefix)
                    .map_err(InstallerError::FailedToDetectInstalledPackages)
            })
            .await?
            .into_iter()
            .map(InstallationResultRecord::Min)
            .collect()
        };

        // Construct a transaction from the current and desired situation.
        let target_platform = self.target_platform.unwrap_or_else(Platform::current);
        let desired_records: Vec<_> = records.into_iter().collect();
        let mut transaction = Transaction::from_current_and_desired(
            installed.iter(),
            desired_records.iter(),
            self.reinstall_packages.as_ref(),
            self.ignored_packages.as_ref(),
            target_platform,
        )?;
        // If transaction is non-empty, we need full prefix records for file operations
        // Reload them and reconstruct the transaction with full records
        if !transaction.operations.is_empty() && !installed_provided {
            let prefix = prefix.clone();
            installed = run_blocking_task(move || {
                PrefixRecord::collect_from_prefix(&prefix)
                    .map_err(InstallerError::FailedToDetectInstalledPackages)
            })
            .await?
            .into_iter()
            .map(InstallationResultRecord::Max)
            .collect();

            // Reconstruct transaction with full records to maintain consistency
            transaction = Transaction::from_current_and_desired(
                installed.iter(),
                desired_records.iter(),
                self.reinstall_packages.as_ref(),
                self.ignored_packages.as_ref(),
                target_platform,
            )?;
        }

        let transaction = transaction.to_owned();

        // Validate that if the target platform is NoArch, all packages to be installed
        // must also be noarch (subdir == "noarch")
        if target_platform == Platform::NoArch {
            let non_noarch_packages: Vec<String> = transaction
                .installed_packages()
                .filter(|record| record.package_record.subdir != "noarch")
                .map(|record| {
                    format!(
                        "{}/{}-{}-{}",
                        record.package_record.subdir,
                        record.package_record.name.as_normalized(),
                        record.package_record.version,
                        record.package_record.build
                    )
                })
                .collect();

            if !non_noarch_packages.is_empty() {
                return Err(InstallerError::PlatformSpecificPackagesWithNoarchPlatform(
                    non_noarch_packages,
                ));
            }
        }

        // Create a mapping from package names to requested specs
        let spec_mapping = self
            .requested_specs
            .as_ref()
            .map(|specs| create_spec_mapping(specs))
            .map(Arc::new);

        // Update existing records that weren't modified but have matching requested
        // specs This needs to happen even if the transaction is empty
        if let Some(spec_mapping) = &spec_mapping {
            // We have requested_specs (even if empty), so update/clear as needed
            update_existing_records(transaction.unchanged_packages(), spec_mapping, &prefix)?;
        }

        // If the transaction is empty we can short-circuit the installation
        if transaction.operations.is_empty() {
            return Ok(InstallationResult {
                transaction,
                pre_link_script_result: None,
                post_link_script_result: None,
                clobbered_paths: HashMap::default(),
            });
        }

        // At this point we can't have any minimal prefix records, so force them to be prefix records.
        let transaction = transaction
            .into_prefix_record(&prefix)
            .map_err(InstallerError::FailedToDetectInstalledPackages)?;

        let downloader = self.downloader.unwrap_or_default();
        let package_cache = self.package_cache.unwrap_or_else(|| {
            PackageCache::new(
                default_cache_dir()
                    .expect("failed to determine default cache directory")
                    .join(rattler_cache::PACKAGE_CACHE_DIR),
            )
        });

        // Acquire a global lock on the package cache for the entire installation.
        // This significantly reduces overhead by avoiding per-package locking.
        let _global_cache_lock = package_cache
            .acquire_global_lock()
            .await
            .map_err(InstallerError::FailedToAcquireCacheLock)?;

        // Construct a driver.
        let driver = InstallDriver::builder()
            .execute_link_scripts(self.execute_link_scripts)
            .with_io_concurrency_semaphore(
                self.io_semaphore.unwrap_or(Arc::new(Semaphore::new(100))),
            )
            .with_prefix_records(
                transaction
                    .unchanged_packages()
                    .iter()
                    .chain(transaction.removed_packages()),
            )
            .finish();

        // Determine base installer options.
        let base_install_options = InstallOptions {
            target_prefix: self.alternative_target_prefix.clone(),
            platform: Some(target_platform),
            python_info: transaction.python_info.clone(),
            apple_codesign_behavior: self.apple_code_sign_behavior,
            allow_symbolic_links: self.link_options.allow_symbolic_links,
            allow_hard_links: self.link_options.allow_hard_links,
            allow_ref_links: self.link_options.allow_ref_links,
            ..InstallOptions::default()
        };

        // Preprocess the transaction
        let pre_process_result = driver
            .pre_process(&transaction, &prefix, self.reporter.as_deref())
            .map_err(InstallerError::PreProcessingFailed)?;

        if let Some(reporter) = &self.reporter {
            reporter.on_transaction_start(&transaction);
        }

        let mut pending_unlink_futures = FuturesUnordered::new();
        // Execute the operations (remove) in the transaction.
        for (operation_idx, operation) in transaction.operations.iter().enumerate() {
            let reporter = self.reporter.clone();
            let driver = &driver;
            let prefix = &prefix;

            let op = async move {
                // Uninstall the package if it was removed.
                if let Some(record) = operation.record_to_remove() {
                    if let Some(reporter) = &reporter {
                        reporter.on_transaction_operation_start(operation_idx);
                    }

                    let reporter = reporter
                        .as_deref()
                        .map(move |r| (r, r.on_unlink_start(operation_idx, record)));
                    driver.clobber_registry().unregister_paths(record);
                    unlink_package(prefix, record).await.map_err(|e| {
                        InstallerError::UnlinkError(record.repodata_record.file_name.clone(), e)
                    })?;
                    if let Some((reporter, index)) = reporter {
                        reporter.on_unlink_complete(index);
                        if operation.record_to_install().is_none() {
                            reporter.on_transaction_operation_complete(operation_idx);
                        }
                    }
                }
                Ok::<(), InstallerError>(())
            };
            pending_unlink_futures.push(op);
        }

        let mut pending_link_futures = FuturesUnordered::new();
        // Execute the operations (install) in the transaction.
        for (operation_idx, operation) in transaction
            .operations
            .iter()
            .enumerate()
            .sorted_by_key(|(_, op)| {
                op.record_to_install()
                    .and_then(|r| r.package_record.size)
                    .unwrap_or(0)
            })
            .rev()
        {
            let downloader = &downloader;
            let package_cache = &package_cache;
            let reporter = self.reporter.clone();
            let base_install_options = &base_install_options;
            let driver = &driver;
            let prefix = &prefix;
            let spec_mapping_ref = spec_mapping.clone();
            let operation_future = async move {
                if let Some(reporter) = &reporter {
                    if operation.record_to_remove().is_none() {
                        reporter.on_transaction_operation_start(operation_idx);
                    }
                }

                // Start populating the cache with the package if it's not already there.
                let package_to_install = if let Some(record) = operation.record_to_install() {
                    let record = record.clone();
                    let downloader = downloader.clone();
                    let reporter = reporter.clone();
                    let package_cache = package_cache.clone();
                    tokio::spawn(async move {
                        let populate_cache_report = reporter.clone().map(|r| {
                            let cache_index = r.on_populate_cache_start(operation_idx, &record);
                            (r, cache_index)
                        });
                        let cache_lock = populate_cache(
                            &record,
                            downloader,
                            &package_cache,
                            populate_cache_report.clone(),
                        )
                        .await?;
                        if let Some((reporter, index)) = populate_cache_report {
                            reporter.on_populate_cache_complete(index);
                        }
                        Ok((cache_lock, record))
                    })
                    .map_err(JoinError::try_into_panic)
                    .map(|res| match res {
                        Ok(Ok(result)) => Ok(Some(result)),
                        Ok(Err(e)) => Err(e),
                        Err(Ok(payload)) => std::panic::resume_unwind(payload),
                        Err(Err(_err)) => Err(InstallerError::Cancelled),
                    })
                    .left_future()
                } else {
                    ready(Ok(None)).right_future()
                };

                // Install the package if it was fetched.
                if let Some((cache_lock, record)) = package_to_install.await? {
                    let reporter = reporter
                        .as_deref()
                        .map(|r| (r, r.on_link_start(operation_idx, &record)));
                    let requested_spec = spec_mapping_ref
                        .and_then(|mapping| mapping.get(&record.package_record.name).cloned())
                        .unwrap_or_default();
                    link_package(
                        &record,
                        prefix,
                        cache_lock.path(),
                        base_install_options.clone(),
                        driver,
                        requested_spec,
                    )
                    .await?;
                    if let Some((reporter, index)) = reporter {
                        reporter.on_link_complete(index);
                    }
                }
                if let Some(reporter) = &reporter {
                    if operation.record_to_install().is_some() {
                        reporter.on_transaction_operation_complete(operation_idx);
                    }
                }

                Ok::<_, InstallerError>(())
            };

            pending_link_futures.push(operation_future);
        }

        // Wait for all transaction operations to finish
        while let Some(result) = pending_unlink_futures.next().await {
            result?;
        }
        drop(pending_unlink_futures);

        driver
            .remove_empty_directories(
                &transaction.operations,
                transaction.unchanged_packages(),
                &prefix,
            )
            .unwrap();

        // Wait for all transaction operations to finish
        while let Some(result) = pending_link_futures.next().await {
            result?;
        }
        drop(pending_link_futures);

        // Post process the transaction
        let post_process_result =
            driver.post_process(&transaction, &prefix, self.reporter.as_deref())?;

        if let Some(reporter) = &self.reporter {
            reporter.on_transaction_complete();
        }

        let transaction = transaction.into_installation_result_record();

        Ok(InstallationResult {
            transaction,
            pre_link_script_result: pre_process_result,
            post_link_script_result: post_process_result.post_link_result,
            clobbered_paths: post_process_result.clobbered_paths,
        })
    }
}

async fn link_package(
    record: &RepoDataRecord,
    target_prefix: &Prefix,
    cached_package_dir: &Path,
    install_options: InstallOptions,
    driver: &InstallDriver,
    requested_specs: Vec<String>,
) -> Result<(), InstallerError> {
    let record = record.clone();
    let target_prefix = target_prefix.clone();
    let cached_package_dir = cached_package_dir.to_path_buf();
    let clobber_registry = driver.clobber_registry.clone();

    let (tx, rx) = tokio::sync::oneshot::channel();

    // Since we use the `Prefix` type, the conda-meta folder is guaranteed to exist
    let conda_meta_path = target_prefix.path().join("conda-meta");

    rayon::spawn_fifo(move || {
        let inner = move || {
            // Link the contents of the package into the prefix.
            let paths = crate::install::link_package_sync(
                &cached_package_dir,
                &target_prefix,
                clobber_registry,
                install_options,
            )
            .map_err(|e| InstallerError::LinkError(record.file_name.clone(), e))?;

            // Construct a PrefixRecord for the package
            let prefix_record = PrefixRecord {
                extracted_package_dir: Some(cached_package_dir.clone()),
                link: Some(Link {
                    source: cached_package_dir,
                    // TODO: compute the right value here based on the options and `can_hard_link`
                    // ...
                    link_type: Some(LinkType::HardLink),
                }),
                requested_specs,
                ..PrefixRecord::from_repodata_record(record.clone(), paths)
            };

            let pkg_meta_path = prefix_record.file_name();
            prefix_record
                .write_to_path(conda_meta_path.join(&pkg_meta_path), true)
                .map_err(|e| {
                    InstallerError::IoError(format!("failed to write {pkg_meta_path}"), e)
                })?;

            Ok(())
        };

        let _ = tx.send(inner());
    });

    rx.await.unwrap_or(Err(InstallerError::Cancelled))
}

/// Given a repodata record, fetch the package into the cache if its not already
/// there.
async fn populate_cache(
    record: &RepoDataRecord,
    downloader: LazyClient,
    cache: &PackageCache,
    reporter: Option<(Arc<dyn Reporter>, usize)>,
) -> Result<CacheLock, InstallerError> {
    struct CacheReporterBridge {
        reporter: Arc<dyn Reporter>,
        cache_index: usize,
    }

    impl CacheReporter for CacheReporterBridge {
        fn on_validate_start(&self) -> usize {
            self.reporter.on_validate_start(self.cache_index)
        }

        fn on_validate_complete(&self, index: usize) {
            self.reporter.on_validate_complete(index);
        }

        fn on_download_start(&self) -> usize {
            self.reporter.on_download_start(self.cache_index)
        }

        fn on_download_progress(&self, index: usize, progress: u64, total: Option<u64>) {
            self.reporter.on_download_progress(index, progress, total);
        }

        fn on_download_completed(&self, index: usize) {
            self.reporter.on_download_completed(index);
        }
    }

    cache
        .get_or_fetch_from_url_with_retry(
            &record.package_record,
            record.url.clone(),
            downloader,
            default_retry_policy(),
            reporter.map(|(reporter, cache_index)| {
                Arc::new(CacheReporterBridge {
                    reporter,
                    cache_index,
                }) as _
            }),
        )
        .await
        .map_err(|e| InstallerError::FailedToFetch(record.file_name.clone(), e))
}

/// Updates only the `requested_specs` fields in a conda-meta JSON file.
/// This performs a targeted update without overwriting other
/// metadata.
///
/// This method is needed as we're initially loading
/// `MinimalPrefixRecord`, which doesn't contain most of the fields.
/// Therefore direct writing could overwrite data we want to preserve.
///
/// Currently we're loading full json, but we could do that inplace without parsing whole file.
fn update_requested_specs_in_json(
    path: &Path,
    requested_specs: &[String],
    requested_spec: Option<&String>,
) -> io::Result<()> {
    use serde_json::Value;

    // Read the existing JSON file
    let content = fs_err::read_to_string(path)?;
    let mut json: Value = serde_json::from_str(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    // Update only the requested_specs fields
    if let Some(obj) = json.as_object_mut() {
        // Update requested_specs (plural)
        obj.insert(
            "requested_specs".to_string(),
            Value::Array(
                requested_specs
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );

        // Update or remove requested_spec (singular, deprecated)
        if let Some(spec) = requested_spec {
            obj.insert("requested_spec".to_string(), Value::String(spec.clone()));
        } else {
            obj.remove("requested_spec");
        }
    }

    // Write the updated JSON back to file
    let updated_content = serde_json::to_string_pretty(&json)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs_err::write(path, updated_content)?;

    Ok(())
}

/// Creates a mapping from package names to their requested spec strings.
///
/// This function takes a list of `MatchSpecs` and creates a mapping where:
/// - The key is the `PackageName` from the `MatchSpec`
/// - The value is a vector of string representations of all matching
///   `MatchSpecs`
///
/// Only `MatchSpec`s that have a `PackageNameMatcher::Exact` are included.
/// For multiple `MatchSpec`s with the same package name, all are collected.
fn create_spec_mapping(specs: &[MatchSpec]) -> std::collections::HashMap<PackageName, Vec<String>> {
    let mut mapping = std::collections::HashMap::new();

    for spec in specs {
        if let Some(PackageNameMatcher::Exact(name)) = &spec.name {
            mapping
                .entry(name.clone())
                .or_insert_with(Vec::new)
                .push(spec.to_string());
        }
    }

    mapping
}

/// Updates existing `PrefixRecord` files with their requested specs.
///
/// This function takes existing records that weren't modified by the
/// transaction and updates their `requested_specs` field based on the spec
/// mapping or clears it if requested. The updated records are then written back
/// to disk.
#[allow(deprecated)]
fn update_existing_records<'p>(
    existing_records: impl IntoParallelIterator<Item = &'p InstallationResultRecord>,
    spec_mapping: &HashMap<PackageName, Vec<String>>,
    prefix: &Prefix,
) -> Result<(), InstallerError> {
    existing_records
        .into_par_iter()
        .map(|record| -> Result<(), InstallerError> {
            let package_name = record.name();
            let mut updated_record = None;

            // First, check if we need to migrate from deprecated requested_spec to
            // requested_specs
            let current_specs = if !record.requested_specs().is_empty() {
                // Use the new field if it has data
                record.requested_specs().clone()
            } else if let Some(spec) = record.requested_spec() {
                // Migrate from deprecated field
                vec![spec.clone()]
            } else {
                // No specs at all
                Vec::new()
            };

            // Check if we need to migrate from the deprecated field
            let needs_migration = record.requested_spec().is_some();

            // Check if we have requested specs for this package
            if let Some(requested_specs) = spec_mapping.get(package_name) {
                // Check if the requested specs are different from what's currently stored
                if needs_migration || &current_specs != requested_specs {
                    // Create an updated record with the new requested specs
                    let mut new_record = record.clone();
                    *new_record.requested_specs_mut() = requested_specs.clone();
                    *new_record.requested_spec_mut() = None; // Clear deprecated field
                    updated_record = Some(new_record);
                }
            } else if !current_specs.is_empty() {
                // Clear the requested_specs if it's not in the mapping
                let mut new_record = record.clone();
                *new_record.requested_specs_mut() = Vec::new();
                *new_record.requested_spec_mut() = None; // Clear deprecated field
                updated_record = Some(new_record);
            } else if needs_migration {
                // Even if current_specs is empty, we still need to clear the deprecated field
                let mut new_record = record.clone();
                *new_record.requested_spec_mut() = None;
                updated_record = Some(new_record);
            }

            // Write the updated record back to disk if needed
            if let Some(new_record) = updated_record {
                let conda_meta_path = prefix.path().join("conda-meta");
                let pkg_meta_path = format!(
                    "{}-{}-{}.json",
                    new_record.name().as_normalized(),
                    new_record.version(),
                    new_record.build()
                );
                let full_path = conda_meta_path.join(&pkg_meta_path);

                // We need to do a targeted update of just the requested_specs fields
                // to avoid overwriting other metadata when using minimal records
                update_requested_specs_in_json(
                    &full_path,
                    new_record.requested_specs(),
                    new_record.requested_spec(),
                )
                .map_err(|e| {
                    InstallerError::IoError(
                        format!("failed to update requested_specs for {pkg_meta_path}"),
                        e,
                    )
                })?;
            }

            Ok(())
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rattler_conda_types::{
        package::IndexJson, prefix::Prefix, MatchSpec, PackageName, ParseStrictness::Strict,
    };
    use rattler_package_streaming::seek::read_package_file;
    use tempfile::TempDir;
    use url::Url;

    use super::*;

    /// Creates a test environment with a temporary directory and prefix
    fn create_test_environment() -> (TempDir, Prefix) {
        let temp_dir = TempDir::new().unwrap();
        let target_prefix = Prefix::create(temp_dir.path()).unwrap();
        (temp_dir, target_prefix)
    }

    /// Creates a `RepoDataRecord` from the dummy package for testing
    fn create_dummy_repo_record() -> rattler_conda_types::RepoDataRecord {
        let package_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/packages/empty-0.1.0-h4616a5c_0.conda")
            .canonicalize()
            .unwrap();

        let index_json: IndexJson =
            read_package_file(&package_path).expect("Failed to read package");
        RepoDataRecord {
            package_record: rattler_conda_types::PackageRecord::from_index_json(
                index_json, None, // size unknown
                None, // sha256 unknown
                None, // md5 unknown
            )
            .unwrap(),
            file_name: "empty-0.1.0-h4616a5c_0.conda".to_string(),
            url: Url::from_file_path(package_path).unwrap(),
            channel: Some("local".to_string()),
        }
    }

    /// Gets the conda-meta file path for a given `RepoDataRecord`
    fn get_meta_file_path(
        prefix: &Prefix,
        repo_record: &rattler_conda_types::RepoDataRecord,
    ) -> std::path::PathBuf {
        let conda_meta_path = prefix.path().join("conda-meta");
        let expected_filename = format!(
            "{}-{}-{}.json",
            repo_record.package_record.name.as_normalized(),
            repo_record.package_record.version,
            repo_record.package_record.build
        );
        conda_meta_path.join(&expected_filename)
    }

    /// Reads a `PrefixRecord` from its conda-meta file
    fn read_prefix_record(meta_file_path: &std::path::Path) -> rattler_conda_types::PrefixRecord {
        rattler_conda_types::PrefixRecord::from_path(meta_file_path)
            .expect("Should be able to read the prefix record")
    }

    /// Installs a package using the given installer and verifies success
    async fn install_and_verify_success(
        installer: Installer,
        prefix: &Prefix,
        repo_record: rattler_conda_types::RepoDataRecord,
    ) {
        let result = installer.install(prefix, vec![repo_record]).await;
        assert!(
            result.is_ok(),
            "Installation should succeed, but got error: {result:#?}"
        );
    }

    #[test]
    fn test_spec_mapping_helper() {
        // Test the spec mapping functionality
        let specs = vec![
            MatchSpec::from_str("python ~=3.11.0", Strict).unwrap(),
            MatchSpec::from_str("numpy >=1.20", Strict).unwrap(),
        ];

        let mapping = create_spec_mapping(&specs);

        // Should map package names to their match specs
        assert_eq!(mapping.len(), 2);
        let python_name: PackageName = "python".parse().unwrap();
        let numpy_name: PackageName = "numpy".parse().unwrap();
        assert!(mapping.contains_key(&python_name));
        assert!(mapping.contains_key(&numpy_name));

        // Should convert match specs to string format
        assert_eq!(mapping[&python_name], vec!["python ~=3.11.0"]);
        assert_eq!(mapping[&numpy_name], vec!["numpy >=1.20"]);
    }

    #[test]
    fn test_spec_mapping_with_nameless_specs() {
        // Test handling of nameless specs (should be skipped)
        let specs = vec![
            MatchSpec::from_str("python ~=3.11.0", Strict).unwrap(),
            // Create a nameless spec by removing the name
            MatchSpec {
                name: None,
                version: Some(">=1.0".parse().unwrap()),
                ..Default::default()
            },
        ];

        let mapping = create_spec_mapping(&specs);

        // Should only include the named spec
        assert_eq!(mapping.len(), 1);
        let python_name: PackageName = "python".parse().unwrap();
        assert!(mapping.contains_key(&python_name));
    }

    #[test]
    fn test_update_existing_records_logic() {
        // Test the logic for determining if records should be updated
        use std::collections::HashMap;

        // Mock spec mapping
        let mut spec_mapping: HashMap<PackageName, Vec<String>> = HashMap::new();
        spec_mapping.insert(
            "python".parse().unwrap(),
            vec!["python ~=3.11.0".to_string()],
        );

        // Test that the mapping is created correctly
        assert_eq!(spec_mapping.len(), 1);
        assert_eq!(
            spec_mapping[&"python".parse::<PackageName>().unwrap()],
            vec!["python ~=3.11.0"]
        );

        // Test that we can check if a package needs updating
        let package_name: PackageName = "python".parse().unwrap();
        let requested_spec_from_mapping = spec_mapping.get(&package_name);
        assert!(requested_spec_from_mapping.is_some());
        assert_eq!(
            requested_spec_from_mapping.unwrap(),
            &vec!["python ~=3.11.0"]
        );

        // Test missing package
        let missing_package: PackageName = "missing".parse().unwrap();
        assert!(!spec_mapping.contains_key(&missing_package));
    }

    #[test]
    fn test_spec_mapping_with_multiple_specs_same_package() {
        // Test handling of multiple specs for the same package
        let specs = vec![
            MatchSpec::from_str("python >=3.8", Strict).unwrap(),
            MatchSpec::from_str("python <3.12", Strict).unwrap(),
            MatchSpec::from_str("numpy >=1.20", Strict).unwrap(),
        ];

        let mapping = create_spec_mapping(&specs);

        // Should map package names to their match specs
        assert_eq!(mapping.len(), 2);
        let python_name: PackageName = "python".parse().unwrap();
        let numpy_name: PackageName = "numpy".parse().unwrap();
        assert!(mapping.contains_key(&python_name));
        assert!(mapping.contains_key(&numpy_name));

        // Should collect all specs for python, but single spec for numpy
        assert_eq!(mapping[&python_name], vec!["python >=3.8", "python <3.12"]);
        assert_eq!(mapping[&numpy_name], vec!["numpy >=1.20"]);
    }

    #[tokio::test]
    async fn test_install_with_requested_specs_e2e() {
        let (_temp_dir, target_prefix) = create_test_environment();
        let repo_record = create_dummy_repo_record();

        // Create a requested spec for the empty package
        let requested_spec = MatchSpec::from_str("empty >=0.1.0", Strict).unwrap();
        let requested_specs = vec![requested_spec];

        // Install using the installer with requested specs
        let installer = Installer::new().with_requested_specs(requested_specs);
        install_and_verify_success(installer, &target_prefix, repo_record.clone()).await;

        // Verify that the conda-meta file was created with the correct requested_spec
        let meta_file_path = get_meta_file_path(&target_prefix, &repo_record);
        assert!(meta_file_path.exists(), "conda-meta file should exist");

        // Read and verify the PrefixRecord
        let updated_record = read_prefix_record(&meta_file_path);

        // Verify that requested_specs is properly set
        assert!(
            !updated_record.requested_specs.is_empty(),
            "requested_specs should be populated"
        );
        assert_eq!(
            updated_record.requested_specs.first().unwrap(),
            "empty >=0.1.0",
            "requested_specs should match the original spec"
        );
    }

    #[tokio::test]
    async fn test_install_without_requested_specs_e2e() {
        let (_temp_dir, target_prefix) = create_test_environment();
        let repo_record = create_dummy_repo_record();

        // Install using the installer WITHOUT requested specs
        let installer = Installer::new();
        install_and_verify_success(installer, &target_prefix, repo_record.clone()).await;

        // Verify that the conda-meta file was created without requested_spec
        let meta_file_path = get_meta_file_path(&target_prefix, &repo_record);
        assert!(meta_file_path.exists(), "conda-meta file should exist");

        // Read and verify the PrefixRecord
        let updated_record = read_prefix_record(&meta_file_path);

        // Verify that requested_specs is empty (original behavior)
        assert!(
            updated_record.requested_specs.is_empty(),
            "requested_specs should be empty when not provided"
        );
    }

    #[tokio::test]
    async fn test_update_existing_package_requested_spec() {
        let (_temp_dir, target_prefix) = create_test_environment();
        let repo_record = create_dummy_repo_record();

        // Step 1: Install the package WITHOUT requested specs (simulating existing
        // installation)
        let installer = Installer::new();
        install_and_verify_success(installer, &target_prefix, repo_record.clone()).await;

        // Verify that initially there's no requested_specs
        let meta_file_path = get_meta_file_path(&target_prefix, &repo_record);
        let initial_record = read_prefix_record(&meta_file_path);
        assert!(
            initial_record.requested_specs.is_empty(),
            "Initial installation should have no requested_specs"
        );

        // Step 2: "Install" the same package again, but this time WITH requested specs
        // This simulates a user running install with specs on an existing environment
        let requested_spec = MatchSpec::from_str("empty >=0.1.0", Strict).unwrap();
        let requested_specs = vec![requested_spec];

        let installer_with_specs = Installer::new().with_requested_specs(requested_specs);
        install_and_verify_success(installer_with_specs, &target_prefix, repo_record.clone()).await;

        // Step 3: Verify that the existing package now has the requested_specs updated
        let updated_record = read_prefix_record(&meta_file_path);

        // The package should now have the requested_specs populated
        assert!(
            !updated_record.requested_specs.is_empty(),
            "Updated installation should have requested_specs"
        );
        assert_eq!(
            updated_record.requested_specs.first().unwrap(),
            "empty >=0.1.0",
            "requested_specs should match the newly provided spec"
        );
    }

    #[tokio::test]
    async fn test_clear_requested_spec_when_empty() {
        let (_temp_dir, target_prefix) = create_test_environment();
        let repo_record = create_dummy_repo_record();

        // Step 1: Install the package WITH requested specs
        let requested_spec = MatchSpec::from_str("empty >=0.1.0", Strict).unwrap();
        let requested_specs = vec![requested_spec];

        let installer_with_specs = Installer::new().with_requested_specs(requested_specs);
        install_and_verify_success(installer_with_specs, &target_prefix, repo_record.clone()).await;

        // Verify that the package has requested_specs populated
        let meta_file_path = get_meta_file_path(&target_prefix, &repo_record);
        let initial_record = read_prefix_record(&meta_file_path);
        assert!(
            !initial_record.requested_specs.is_empty(),
            "Initial installation should have requested_specs"
        );
        assert_eq!(
            initial_record.requested_specs.first().unwrap(),
            "empty >=0.1.0"
        );

        // Step 2: "Install" the same package again, but this time with EMPTY requested
        // specs This simulates a user running install with empty specs to clear
        // existing ones
        let installer_without_specs = Installer::new().with_requested_specs(vec![]); // Explicitly empty requested_specs
        install_and_verify_success(installer_without_specs, &target_prefix, repo_record.clone())
            .await;

        // Step 3: Verify that the existing package now has the requested_specs cleared
        let updated_record = read_prefix_record(&meta_file_path);

        // The package should now have the requested_specs cleared (set to empty)
        assert!(
            updated_record.requested_specs.is_empty(),
            "Updated installation without specs should clear requested_specs, got nonempty record requested_specs: {:#?}", updated_record.requested_specs
        );
    }

    #[tokio::test]
    async fn test_install_with_ignored_packages() {
        let (_temp_dir, target_prefix) = create_test_environment();
        let repo_record = create_dummy_repo_record();

        // Step 1: Install the package first
        let installer = Installer::new();
        install_and_verify_success(installer, &target_prefix, repo_record.clone()).await;

        // Verify the package was installed
        let meta_file_path = get_meta_file_path(&target_prefix, &repo_record);
        assert!(meta_file_path.exists(), "Package should be installed");

        // Step 2: Try to "remove" the package by installing an empty environment, but ignore the package
        let package_name = repo_record.package_record.name.clone();
        let ignored_packages = HashSet::from_iter(vec![package_name]);
        let installer_with_ignored = Installer::new().with_ignored_packages(ignored_packages);

        // Install empty environment (should remove all packages, except ignored ones)
        let result = installer_with_ignored
            .install(&target_prefix, Vec::<RepoDataRecord>::new())
            .await;

        assert!(
            result.is_ok(),
            "Installation with ignored packages should succeed"
        );

        // Verify the ignored package is still there
        assert!(
            meta_file_path.exists(),
            "Ignored package should still be installed"
        );

        // Verify transaction was empty (no operations performed)
        let installation_result = result.unwrap();
        assert!(
            installation_result.transaction.operations.is_empty(),
            "No operations should be performed on ignored packages"
        );
    }

    #[tokio::test]
    async fn test_migrate_deprecated_requested_spec() {
        let (_temp_dir, target_prefix) = create_test_environment();
        let repo_record = create_dummy_repo_record();

        // Step 1: Manually create a PrefixRecord with the deprecated requested_spec
        // field to simulate an old installation
        let meta_file_path = get_meta_file_path(&target_prefix, &repo_record);
        let conda_meta_path = target_prefix.path().join("conda-meta");
        std::fs::create_dir_all(&conda_meta_path).unwrap();

        // Create a record with the deprecated field set
        #[allow(deprecated)]
        let old_record = PrefixRecord {
            repodata_record: repo_record.clone(),
            requested_spec: Some("empty >=0.1.0".to_string()),
            requested_specs: Vec::new(), // Empty new field
            ..PrefixRecord::from_repodata_record(repo_record.clone(), Vec::new())
        };

        old_record.write_to_path(&meta_file_path, true).unwrap();

        // Verify the old record has deprecated field set
        let initial_record = read_prefix_record(&meta_file_path);
        #[allow(deprecated)]
        {
            assert!(
                initial_record.requested_spec.is_some(),
                "Initial record should have deprecated requested_spec"
            );
            assert!(
                initial_record.requested_specs.is_empty(),
                "Initial record should have empty requested_specs"
            );
        }

        // Step 2: Run installer with the same specs to trigger migration
        let requested_spec = MatchSpec::from_str("empty >=0.1.0", Strict).unwrap();
        let requested_specs = vec![requested_spec];

        let installer_with_specs = Installer::new().with_requested_specs(requested_specs);
        install_and_verify_success(installer_with_specs, &target_prefix, repo_record.clone()).await;

        // Step 3: Verify that the migration happened
        let migrated_record = read_prefix_record(&meta_file_path);

        #[allow(deprecated)]
        {
            assert!(
                migrated_record.requested_spec.is_none(),
                "Migrated record should have cleared deprecated requested_spec"
            );
        }

        assert!(
            !migrated_record.requested_specs.is_empty(),
            "Migrated record should have populated requested_specs"
        );
        assert_eq!(
            migrated_record.requested_specs.first().unwrap(),
            "empty >=0.1.0",
            "Migrated specs should match the original spec"
        );
    }

    #[tokio::test]
    async fn test_noarch_platform_rejects_platform_specific_packages() {
        use rattler_conda_types::Platform;

        let (_temp_dir, target_prefix) = create_test_environment();

        // Create a platform-specific package (with subdir != "noarch")
        let mut platform_specific_package = create_dummy_repo_record();
        platform_specific_package.package_record.subdir = "osx-arm64".to_string();

        // Try to install this platform-specific package with Platform::NoArch
        let installer = Installer::new().with_target_platform(Platform::NoArch);
        let result = installer
            .install(&target_prefix, vec![platform_specific_package.clone()])
            .await;

        // Should fail with PlatformSpecificPackagesWithNoarchPlatform error
        assert!(
            result.is_err(),
            "Installation should fail when installing platform-specific packages with noarch platform"
        );

        match result {
            Err(InstallerError::PlatformSpecificPackagesWithNoarchPlatform(packages)) => {
                assert!(
                    !packages.is_empty(),
                    "Error should list the problematic packages"
                );
                assert!(
                    packages[0].contains("osx-arm64"),
                    "Error message should include the subdir of the platform-specific package"
                );
            }
            _ => {
                panic!("Expected PlatformSpecificPackagesWithNoarchPlatform error, got: {result:?}")
            }
        }
    }

    #[tokio::test]
    async fn test_noarch_platform_accepts_noarch_packages() {
        use rattler_conda_types::{NoArchType, Platform};

        let (_temp_dir, target_prefix) = create_test_environment();

        // Create a noarch package (with subdir == "noarch")
        let mut noarch_package = create_dummy_repo_record();
        noarch_package.package_record.subdir = "noarch".to_string();
        noarch_package.package_record.noarch = NoArchType::generic();

        // Try to install this noarch package with Platform::NoArch
        let installer = Installer::new().with_target_platform(Platform::NoArch);
        let result = installer
            .install(&target_prefix, vec![noarch_package.clone()])
            .await;

        // Should succeed
        assert!(
            result.is_ok(),
            "Installation should succeed when installing noarch packages with noarch platform: {:?}",
            result.err()
        );
    }
}
