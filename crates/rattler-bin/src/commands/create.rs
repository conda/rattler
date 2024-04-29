use crate::global_multi_progress;
use anyhow::Context;
use futures::{stream, stream::FuturesUnordered, FutureExt, StreamExt, TryFutureExt, TryStreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use rattler::{
    default_cache_dir,
    install::{
        link_package, unlink_package, InstallDriver, InstallOptions, Transaction,
        TransactionOperation,
    },
    package_cache::PackageCache,
};
use rattler_conda_types::{
    prefix_record::{Link, LinkType},
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, PackageRecord, ParseStrictness,
    Platform, PrefixRecord, RepoDataRecord, Version,
};
use rattler_networking::{
    retry_policies::default_retry_policy, AuthenticationMiddleware, AuthenticationStorage,
};
use rattler_repodata_gateway::{Gateway, RepoData};
use rattler_solve::{
    libsolv_c::{self},
    resolvo, ChannelPriority, RepoDataIter, SolverImpl, SolverTask,
};
use reqwest::Client;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use std::{
    borrow::Cow,
    env,
    future::ready,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};
use tokio::task::JoinHandle;

#[derive(Debug, clap::Parser)]
pub struct Opt {
    #[clap(short)]
    channels: Option<Vec<String>>,

    #[clap(required = true)]
    specs: Vec<String>,

    #[clap(long)]
    dry_run: bool,

    #[clap(long)]
    platform: Option<String>,

    #[clap(long)]
    virtual_package: Option<Vec<String>>,

    #[clap(long)]
    use_resolvo: bool,

    #[clap(long)]
    timeout: Option<u64>,

    #[clap(long)]
    target_prefix: Option<PathBuf>,
}

pub async fn create(opt: Opt) -> anyhow::Result<()> {
    let channel_config = ChannelConfig::default_with_root_dir(env::current_dir()?);
    let current_dir = env::current_dir()?;
    let target_prefix = opt
        .target_prefix
        .unwrap_or_else(|| current_dir.join(".prefix"));
    println!("Target prefix: {}", target_prefix.display());

    // Determine the platform we're going to install for
    let install_platform = if let Some(platform) = opt.platform {
        Platform::from_str(&platform)?
    } else {
        Platform::current()
    };

    println!("Installing for platform: {install_platform:?}");

    // Parse the specs from the command line. We do this explicitly instead of allow clap to deal
    // with this because we need to parse the `channel_config` when parsing matchspecs.
    let specs = opt
        .specs
        .iter()
        .map(|spec| MatchSpec::from_str(spec, ParseStrictness::Strict))
        .collect::<Result<Vec<_>, _>>()?;

    // Find the default cache directory. Create it if it doesnt exist yet.
    let cache_dir = default_cache_dir()?;
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| anyhow::anyhow!("could not create cache directory: {}", e))?;

    // Determine the channels to use from the command line or select the default. Like matchspecs
    // this also requires the use of the `channel_config` so we have to do this manually.
    let channels = opt
        .channels
        .unwrap_or_else(|| vec![String::from("conda-forge")])
        .into_iter()
        .map(|channel_str| Channel::from_str(channel_str, &channel_config))
        .collect::<Result<Vec<_>, _>>()?;

    // Determine the packages that are currently installed in the environment.
    let installed_packages = find_installed_packages(&target_prefix, 100)
        .await
        .context("failed to determine currently installed packages")?;

    // For each channel/subdirectory combination, download and cache the `repodata.json` that should
    // be available from the corresponding Url. The code below also displays a nice CLI progress-bar
    // to give users some more information about what is going on.
    let download_client = Client::builder()
        .no_gzip()
        .build()
        .expect("failed to create client");

    let authentication_storage = AuthenticationStorage::default();
    let download_client = reqwest_middleware::ClientBuilder::new(download_client)
        .with_arc(Arc::new(AuthenticationMiddleware::new(
            authentication_storage,
        )))
        .build();

    // Get the package names from the matchspecs so we can only load the package records that we need.
    let gateway = Gateway::builder()
        .with_cache_dir(cache_dir.join("repodata"))
        .with_client(download_client.clone())
        .finish();

    let start_load_repo_data = Instant::now();
    let repo_data = wrap_in_async_progress(
        "loading repodata",
        gateway.load_records_recursive(
            channels,
            [install_platform, Platform::NoArch],
            specs.clone(),
        ),
    )
    .await
    .context("failed to load repodata")?;

    // Determine the number of recors
    let total_records: usize = repo_data.iter().map(RepoData::len).sum();
    println!(
        "Loaded {} records in {:?}",
        total_records,
        start_load_repo_data.elapsed()
    );

    // Determine virtual packages of the system. These packages define the capabilities of the
    // system. Some packages depend on these virtual packages to indiciate compability with the
    // hardware of the system.
    let virtual_packages = wrap_in_progress("determining virtual packages", move || {
        if let Some(virtual_packages) = opt.virtual_package {
            Ok(virtual_packages
                .iter()
                .map(|virt_pkg| {
                    let elems = virt_pkg.split('=').collect::<Vec<&str>>();
                    Ok(GenericVirtualPackage {
                        name: elems[0].try_into()?,
                        version: elems
                            .get(1)
                            .map_or(Version::from_str("0"), |s| Version::from_str(s))
                            .expect("Could not parse virtual package version"),
                        build_string: (*elems.get(2).unwrap_or(&"")).to_string(),
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?)
        } else {
            rattler_virtual_packages::VirtualPackage::current()
                .map(|vpkgs| {
                    vpkgs
                        .iter()
                        .map(|vpkg| GenericVirtualPackage::from(vpkg.clone()))
                        .collect::<Vec<_>>()
                })
                .map_err(anyhow::Error::from)
        }
    })?;

    println!(
        "Virtual packages:\n{}\n",
        virtual_packages
            .iter()
            .format_with("\n", |i, f| f(&format_args!("  - {i}",)),)
    );

    // Now that we parsed and downloaded all information, construct the packaging problem that we
    // need to solve. We do this by constructing a `SolverProblem`. This encapsulates all the
    // information required to be able to solve the problem.
    let locked_packages = installed_packages
        .iter()
        .map(|record| record.repodata_record.clone())
        .collect();

    let solver_task = SolverTask {
        available_packages: repo_data.iter().map(RepoDataIter).collect::<Vec<_>>(),
        locked_packages,
        virtual_packages,
        specs,
        pinned_packages: Vec::new(),
        timeout: opt.timeout.map(Duration::from_millis),
        channel_priority: ChannelPriority::Strict,
    };

    // Next, use a solver to solve this specific problem. This provides us with all the operations
    // we need to apply to our environment to bring it up to date.
    let required_packages = wrap_in_progress("solving", move || {
        if opt.use_resolvo {
            resolvo::Solver.solve(solver_task)
        } else {
            libsolv_c::Solver.solve(solver_task)
        }
    })?;

    // sort topologically
    let required_packages = PackageRecord::sort_topologically(required_packages);

    // Construct a transaction to
    let transaction = Transaction::from_current_and_desired(
        installed_packages,
        required_packages,
        install_platform,
    )?;

    if opt.dry_run {
        if transaction.operations.is_empty() {
            println!("No operations necessary");
        }

        let format_record = |r: &RepoDataRecord| {
            format!(
                "{} {} {}",
                r.package_record.name.as_normalized(),
                r.package_record.version,
                r.package_record.build
            )
        };

        for operation in &transaction.operations {
            match operation {
                TransactionOperation::Install(r) => {
                    println!("{} {}", console::style("+").green(), format_record(r));
                }
                TransactionOperation::Change { old, new } => {
                    println!(
                        "{} {} -> {}",
                        console::style("~").yellow(),
                        format_record(&old.repodata_record),
                        format_record(new)
                    );
                }
                TransactionOperation::Reinstall(r) => {
                    println!(
                        "{} {}",
                        console::style("~").yellow(),
                        format_record(&r.repodata_record)
                    );
                }
                TransactionOperation::Remove(r) => {
                    println!(
                        "{} {}",
                        console::style("-").red(),
                        format_record(&r.repodata_record)
                    );
                }
            }
        }

        return Ok(());
    }

    if transaction.operations.is_empty() {
        println!(
            "{} Already up to date",
            console::style(console::Emoji("✔", "")).green(),
        );
    } else {
        // Execute the operations that are returned by the solver.
        execute_transaction(transaction, target_prefix, cache_dir, download_client).await?;
        println!(
            "{} Successfully updated the environment",
            console::style(console::Emoji("✔", "")).green(),
        );
    }

    Ok(())
}

/// Executes the transaction on the given environment.
async fn execute_transaction(
    transaction: Transaction<PrefixRecord, RepoDataRecord>,
    target_prefix: PathBuf,
    cache_dir: PathBuf,
    download_client: reqwest_middleware::ClientWithMiddleware,
) -> anyhow::Result<()> {
    // Open the package cache
    let package_cache = PackageCache::new(cache_dir.join("pkgs"));

    // Create an install driver which helps limit the number of concurrent filesystem operations
    let install_driver = InstallDriver::default();

    // Run pre-process (pre-unlink, mainly)
    install_driver.pre_process(&transaction, &target_prefix)?;

    // Define default installation options.
    let install_options = InstallOptions {
        python_info: transaction.python_info.clone(),
        platform: Some(transaction.platform),
        ..Default::default()
    };

    // Create a progress bars for downloads.
    let multi_progress = global_multi_progress();
    let total_packages_to_download = transaction
        .operations
        .iter()
        .filter(|op| op.record_to_install().is_some())
        .count();
    let download_pb = if total_packages_to_download > 0 {
        let pb = multi_progress.add(
            indicatif::ProgressBar::new(total_packages_to_download as u64)
                .with_style(default_progress_style())
                .with_finish(indicatif::ProgressFinish::WithMessage("Done!".into()))
                .with_prefix("downloading"),
        );
        pb.enable_steady_tick(Duration::from_millis(100));
        Some(pb)
    } else {
        None
    };

    // Create a progress bar to track all operations.
    let total_operations = transaction.operations.len();
    let link_pb = multi_progress.add(
        indicatif::ProgressBar::new(total_operations as u64)
            .with_style(default_progress_style())
            .with_finish(indicatif::ProgressFinish::WithMessage("Done!".into()))
            .with_prefix("linking"),
    );
    link_pb.enable_steady_tick(Duration::from_millis(100));

    // Perform all transactions operations in parallel.
    let operations = transaction.operations.clone();
    stream::iter(operations)
        .map(Ok)
        .try_for_each_concurrent(50, |op| {
            let target_prefix = target_prefix.clone();
            let download_client = download_client.clone();
            let package_cache = &package_cache;
            let install_driver = &install_driver;
            let download_pb = download_pb.as_ref();
            let link_pb = &link_pb;
            let install_options = &install_options;
            async move {
                execute_operation(
                    &target_prefix,
                    download_client,
                    package_cache,
                    install_driver,
                    download_pb,
                    link_pb,
                    op,
                    install_options,
                )
                .await
            }
        })
        .await?;

    // Perform any post processing that is required.
    install_driver.post_process(&transaction, &target_prefix)?;

    // Create a history file (empty) for compatibility with conda.
    let history_file = target_prefix.join("conda-meta/history");
    std::fs::write(history_file, "").context("failed to create history file")?;

    Ok(())
}

/// Executes a single operation of a transaction on the environment.
/// TODO: Move this into an object or something.
#[allow(clippy::too_many_arguments)]
async fn execute_operation(
    target_prefix: &Path,
    download_client: reqwest_middleware::ClientWithMiddleware,
    package_cache: &PackageCache,
    install_driver: &InstallDriver,
    download_pb: Option<&ProgressBar>,
    link_pb: &ProgressBar,
    op: TransactionOperation<PrefixRecord, RepoDataRecord>,
    install_options: &InstallOptions,
) -> anyhow::Result<()> {
    // Determine the package to install
    let install_record = op.record_to_install();
    let remove_record = op.record_to_remove();

    // Create a future to remove the existing package
    let remove_future = if let Some(remove_record) = remove_record {
        remove_package_from_environment(target_prefix, remove_record).left_future()
    } else {
        ready(Ok(())).right_future()
    };

    // Create a future to download the package
    let cached_package_dir_fut = if let Some(install_record) = install_record {
        async {
            // Make sure the package is available in the package cache.
            let result = package_cache
                .get_or_fetch_from_url_with_retry(
                    &install_record.package_record,
                    install_record.url.clone(),
                    download_client.clone(),
                    default_retry_policy(),
                )
                .map_ok(|cache_dir| Some((install_record.clone(), cache_dir)))
                .map_err(anyhow::Error::from)
                .await;

            // Increment the download progress bar.
            if let Some(pb) = download_pb {
                pb.inc(1);
                if pb.length() == Some(pb.position()) {
                    pb.set_style(finished_progress_style());
                }
            }

            result
        }
        .left_future()
    } else {
        ready(Ok(None)).right_future()
    };

    // Await removal and downloading concurrently
    let (_, install_package) = tokio::try_join!(remove_future, cached_package_dir_fut)?;

    // If there is a package to install, do that now.
    if let Some((record, package_dir)) = install_package {
        install_package_to_environment(
            target_prefix,
            package_dir,
            record.clone(),
            install_driver,
            install_options,
        )
        .await?;
    }

    // Increment the link progress bar since we finished a step!
    link_pb.inc(1);
    if link_pb.length() == Some(link_pb.position()) {
        link_pb.set_style(finished_progress_style());
    }

    Ok(())
}

/// Install a package into the environment and write a `conda-meta` file that contains information
/// about how the file was linked.
async fn install_package_to_environment(
    target_prefix: &Path,
    package_dir: PathBuf,
    repodata_record: RepoDataRecord,
    install_driver: &InstallDriver,
    install_options: &InstallOptions,
) -> anyhow::Result<()> {
    // Link the contents of the package into our environment. This returns all the paths that were
    // linked.
    let paths = link_package(
        &package_dir,
        target_prefix,
        install_driver,
        install_options.clone(),
    )
    .await?;

    // Construct a PrefixRecord for the package
    let prefix_record = PrefixRecord {
        repodata_record,
        package_tarball_full_path: None,
        extracted_package_dir: Some(package_dir.clone()),
        files: paths
            .iter()
            .map(|entry| entry.relative_path.clone())
            .collect(),
        paths_data: paths.into(),
        // TODO: Retrieve the requested spec for this package from the request
        requested_spec: None,

        link: Some(Link {
            source: package_dir,
            // TODO: compute the right value here based on the options and `can_hard_link` ...
            link_type: Some(LinkType::HardLink),
        }),
    };

    // Create the conda-meta directory if it doesnt exist yet.
    let target_prefix = target_prefix.to_path_buf();
    match tokio::task::spawn_blocking(move || {
        let conda_meta_path = target_prefix.join("conda-meta");
        std::fs::create_dir_all(&conda_meta_path)?;

        // Write the conda-meta information
        let pkg_meta_path = conda_meta_path.join(format!(
            "{}-{}-{}.json",
            prefix_record
                .repodata_record
                .package_record
                .name
                .as_normalized(),
            prefix_record.repodata_record.package_record.version,
            prefix_record.repodata_record.package_record.build
        ));
        prefix_record.write_to_path(pkg_meta_path, true)
    })
    .await
    {
        Ok(result) => Ok(result?),
        Err(err) => {
            if let Ok(panic) = err.try_into_panic() {
                std::panic::resume_unwind(panic);
            }
            // The operation has been cancelled, so we can also just ignore everything.
            Ok(())
        }
    }
}

/// Completely remove the specified package from the environment.
async fn remove_package_from_environment(
    target_prefix: &Path,
    package: &PrefixRecord,
) -> anyhow::Result<()> {
    unlink_package(target_prefix, package).await?;
    Ok(())
}

/// Displays a spinner with the given message while running the specified function to completion.
fn wrap_in_progress<T, F: FnOnce() -> T>(msg: impl Into<Cow<'static, str>>, func: F) -> T {
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_message(msg);
    let result = func();
    pb.finish_and_clear();
    result
}

/// Displays a spinner with the given message while running the specified function to completion.
async fn wrap_in_async_progress<T, F: Future<Output = T>>(
    msg: impl Into<Cow<'static, str>>,
    fut: F,
) -> T {
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_message(msg);
    let result = fut.await;
    pb.finish_and_clear();
    result
}
//
// /// Given a channel and platform, download and cache the `repodata.json` for it. This function
// /// reports its progress via a CLI progressbar.
// async fn fetch_repo_data_records_with_progress(
//     channel: Channel,
//     platform: Platform,
//     repodata_cache: &Path,
//     client: reqwest_middleware::ClientWithMiddleware,
//     multi_progress: indicatif::MultiProgress,
// ) -> Result<Option<SparseRepoData>, anyhow::Error> {
//     // Create a progress bar
//     let progress_bar = multi_progress.add(
//         indicatif::ProgressBar::new(1)
//             .with_finish(indicatif::ProgressFinish::AndLeave)
//             .with_prefix(format!("{}/{platform}", friendly_channel_name(&channel)))
//             .with_style(default_bytes_style()),
//     );
//     progress_bar.enable_steady_tick(Duration::from_millis(100));
//
//     // Download the repodata.json
//     let download_progress_progress_bar = progress_bar.clone();
//     let result = rattler_repodata_gateway::fetch::fetch_repo_data(
//         channel.platform_url(platform),
//         client,
//         repodata_cache.to_path_buf(),
//         FetchRepoDataOptions::default(),
//         Some(Box::new(move |DownloadProgress { total, bytes }| {
//             download_progress_progress_bar.set_length(total.unwrap_or(bytes));
//             download_progress_progress_bar.set_position(bytes);
//         })),
//     )
//     .await;
//
//     // Error out if an error occurred, but also update the progress bar
//     let result = match result {
//         Err(e) => {
//             let not_found = matches!(&e, FetchRepoDataError::NotFound(_));
//             if not_found && platform != Platform::NoArch {
//                 progress_bar.set_style(finished_progress_style());
//                 progress_bar.finish_with_message("Not Found");
//                 return Ok(None);
//             }
//
//             progress_bar.set_style(errored_progress_style());
//             progress_bar.finish_with_message("Error");
//             return Err(e.into());
//         }
//         Ok(result) => result,
//     };
//
//     // Notify that we are deserializing
//     progress_bar.set_style(deserializing_progress_style());
//     progress_bar.set_message("Deserializing..");
//
//     // Deserialize the data. This is a hefty blocking operation so we spawn it as a tokio blocking
//     // task.
//     let repo_data_json_path = result.repo_data_json_path.clone();
//     match tokio::task::spawn_blocking(move || {
//         SparseRepoData::new(
//             channel,
//             platform.to_string(),
//             repo_data_json_path,
//             Some(|record: &mut PackageRecord| {
//                 if record.name.as_normalized() == "python" {
//                     record.depends.push("pip".to_string());
//                 }
//             }),
//         )
//     })
//     .await
//     {
//         Ok(Ok(repodata)) => {
//             progress_bar.set_style(finished_progress_style());
//             let is_cache_hit = matches!(
//                 result.cache_result,
//                 CacheResult::CacheHit | CacheResult::CacheHitAfterFetch
//             );
//             progress_bar.finish_with_message(if is_cache_hit { "Using cache" } else { "Done" });
//             Ok(Some(repodata))
//         }
//         Ok(Err(err)) => {
//             progress_bar.set_style(errored_progress_style());
//             progress_bar.finish_with_message("Error");
//             Err(err.into())
//         }
//         Err(err) => {
//             if let Ok(panic) = err.try_into_panic() {
//                 std::panic::resume_unwind(panic);
//             } else {
//                 progress_bar.set_style(errored_progress_style());
//                 progress_bar.finish_with_message("Cancelled..");
//                 // Since the task was cancelled most likely the whole async stack is being cancelled.
//                 Err(anyhow::anyhow!("cancelled"))
//             }
//         }
//     }
// }

// /// Returns a friendly name for the specified channel.
// fn friendly_channel_name(channel: &Channel) -> String {
//     channel
//         .name
//         .as_ref()
//         .map_or_else(|| channel.canonical_name(), String::from)
// }
//
// /// Returns the style to use for a progressbar that is currently in progress.
// fn default_bytes_style() -> indicatif::ProgressStyle {
//     indicatif::ProgressStyle::default_bar()
//         .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{bar:40!.bright.yellow/dim.white}] {bytes:>8} @ {smoothed_bytes_per_sec:8}").unwrap()
//         .progress_chars("━━╾─")
//         .with_key(
//             "smoothed_bytes_per_sec",
//             |s: &ProgressState, w: &mut dyn Write| match (s.pos(), s.elapsed().as_millis()) {
//                 (pos, elapsed_ms) if elapsed_ms > 0 => {
//                     write!(w, "{}/s", HumanBytes((pos as f64 * 1000_f64 / elapsed_ms as f64) as u64)).unwrap();
//                 }
//                 _ => write!(w, "-").unwrap(),
//             },
//         )
// }
//
/// Returns the style to use for a progressbar that is currently in progress.
fn default_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{bar:40!.bright.yellow/dim.white}] {pos:>7}/{len:7}").unwrap()
        .progress_chars("━━╾─")
}

// /// Returns the style to use for a progressbar that is in Deserializing state.
// fn deserializing_progress_style() -> indicatif::ProgressStyle {
//     indicatif::ProgressStyle::default_bar()
//         .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] {wide_msg}")
//         .unwrap()
//         .progress_chars("━━╾─")
// }

/// Returns the style to use for a progressbar that is finished.
fn finished_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template(&format!(
            "{} {{prefix:20!}} [{{elapsed_precise}}] {{msg:.bold}}",
            console::style(console::Emoji("✔", " ")).green()
        ))
        .unwrap()
        .progress_chars("━━╾─")
}

// /// Returns the style to use for a progressbar that is in error state.
// fn errored_progress_style() -> indicatif::ProgressStyle {
//     indicatif::ProgressStyle::default_bar()
//         .template(&format!(
//             "{} {{prefix:20!}} [{{elapsed_precise}}] {{msg:.bold.red}}",
//             console::style(console::Emoji("❌", " ")).red()
//         ))
//         .unwrap()
//         .progress_chars("━━╾─")
// }

/// Returns the style to use for a progressbar that is indeterminate and simply shows a spinner.
fn long_running_progress_style() -> indicatif::ProgressStyle {
    ProgressStyle::with_template("{spinner:.green} {msg}").unwrap()
}

/// Scans the conda-meta directory of an environment and returns all the [`PrefixRecord`]s found in
/// there.
async fn find_installed_packages(
    target_prefix: &Path,
    concurrency_limit: usize,
) -> Result<Vec<PrefixRecord>, std::io::Error> {
    let mut meta_futures =
        FuturesUnordered::<JoinHandle<Result<PrefixRecord, std::io::Error>>>::new();
    let mut result = Vec::new();
    for entry in std::fs::read_dir(target_prefix.join("conda-meta"))
        .into_iter()
        .flatten()
    {
        let entry = entry?;
        let path = entry.path();
        if path.ends_with(".json") {
            continue;
        }

        // If there are too many pending entries, wait for one to be finished
        if meta_futures.len() >= concurrency_limit {
            match meta_futures
                .next()
                .await
                .expect("we know there are pending futures")
            {
                Ok(record) => result.push(record?),
                Err(e) => {
                    if let Ok(panic) = e.try_into_panic() {
                        std::panic::resume_unwind(panic);
                    }
                    // The future was cancelled, we can simply return what we have.
                    return Ok(result);
                }
            }
        }

        // Spawn loading on another thread
        let future = tokio::task::spawn_blocking(move || PrefixRecord::from_path(path));
        meta_futures.push(future);
    }

    while let Some(record) = meta_futures.next().await {
        match record {
            Ok(record) => result.push(record?),
            Err(e) => {
                if let Ok(panic) = e.try_into_panic() {
                    std::panic::resume_unwind(panic);
                }
                // The future was cancelled, we can simply return what we have.
                return Ok(result);
            }
        }
    }

    Ok(result)
}
