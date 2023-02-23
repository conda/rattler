use futures::{stream, StreamExt, TryStreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use rattler::install::{link_package, InstallDriver, InstallOptions, PythonInfo};
use rattler::package_cache::PackageCache;
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, Platform, PrefixRecord, RepoData,
    RepoDataRecord,
};
use rattler_repodata_gateway::fetch::{CacheResult, DownloadProgress, FetchRepoDataOptions};
use rattler_solve::{
    PackageOperation, PackageOperationKind, RequestedAction, SolverBackend, SolverProblem,
};
use reqwest::Client;
use std::borrow::Cow;
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, clap::Parser)]
pub struct Opt {
    #[clap(short)]
    channels: Option<Vec<String>>,

    #[clap(required = true)]
    specs: Vec<String>,
}

pub async fn create(opt: Opt) -> anyhow::Result<()> {
    let channel_config = ChannelConfig::default();
    let target_prefix = env::current_dir()?.join("conda-env");

    // Parse the specs from the command line. We do this explicitly instead of allow clap to deal
    // with this because we need to parse the `channel_config` when parsing matchspecs.
    let specs = opt
        .specs
        .iter()
        .map(|spec| {
            MatchSpec::from_str(spec, &channel_config).map(|s| (s, RequestedAction::Install))
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Find the default cache directory. Create it if it doesnt exist yet.
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory for current platform"))?
        .join("rattler/cache");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| anyhow::anyhow!("could not create cache directory: {}", e))?;

    // Determine the channels to use from the command line or select the default. Like matchspecs
    // this also requires the use of the `channel_config` so we have to do this manually.
    let channels = opt
        .channels
        .unwrap_or_else(|| vec![String::from("conda-forge")])
        .into_iter()
        .map(|channel_str| Channel::from_str(&channel_str, &channel_config))
        .collect::<Result<Vec<_>, _>>()?;

    // Each channel contains multiple subdirectories. Users can specify the subdirectories they want
    // to use when specifying their channels. If the user didn't specify the default subdirectories
    // we use defaults based on the current platform.
    let channel_urls = channels
        .iter()
        .flat_map(|channel| {
            channel
                .platforms_or_default()
                .into_iter()
                .map(move |platform| (channel.clone(), *platform))
        })
        .collect::<Vec<_>>();

    // For each channel/subdirectory combination, download and cache the `repodata.json` that should
    // be available from the corresponding Url. The code below also displays a nice CLI progress-bar
    // to give users some more information about what is going on.
    let download_client = Client::builder()
        .no_gzip()
        .build()
        .expect("failed to create client");
    let repodata_cache_path = cache_dir.join("repodata");
    let channel_and_platform_len = channel_urls.len();
    let multi_progress = indicatif::MultiProgress::new();
    let repodata_download_client = download_client.clone();
    let repodatas = futures::stream::iter(channel_urls)
        .map(move |(channel, platform)| {
            let repodata_cache = repodata_cache_path.clone();
            let download_client = repodata_download_client.clone();
            let multi_progress = multi_progress.clone();
            async move {
                fetch_repo_data_records_with_progress(
                    channel,
                    platform,
                    &repodata_cache,
                    download_client.clone(),
                    multi_progress,
                )
                .await
            }
        })
        .buffer_unordered(channel_and_platform_len)
        .collect::<Vec<_>>()
        .await
        // Collect into another iterator where we extract the first errornous result
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    // Determine virtual packages of the system. These packages define the capabilities of the
    // system. Some packages depend on these virtual packages to indiciate compability with the
    // hardware of the system.
    // TODO: Progress indicator?
    let virtual_packages = wrap_in_progress("determining virtual packages", move || {
        rattler_virtual_packages::VirtualPackage::current().map(|vpkgs| {
            vpkgs
                .into_iter()
                .map(|vpkg| GenericVirtualPackage::from(vpkg.clone()))
                .collect::<Vec<_>>()
        })
    })?;

    // Now that we parsed and downloaded all information, construct the packaging problem that we
    // need to solve. We do this by constructing a `SolverProblem`. This encapsulates all the
    // information required to be able to solve the problem.
    let solver_problem = SolverProblem {
        available_packages: repodatas,
        installed_packages: vec![],
        virtual_packages,
        specs,
    };

    // Next, use a solver to solve this specific problem. This provides us with all the operations
    // we need to apply to our environment to bring it up to date.
    let result = wrap_in_progress("solving", move || {
        rattler_solve::LibsolvSolver.solve(solver_problem)
    })?;

    // Determine the platform we're going to install for
    let install_platform = Platform::current();

    // Determine the python version from the packages to install or from the currently installed
    // packages.
    let python_info = result
        .iter()
        .find_map(|op| {
            if matches!(
                op.kind,
                PackageOperationKind::Install | PackageOperationKind::Reinstall
            ) && op.package.package_record.name == "python"
            {
                Some(PythonInfo::from_version(
                    &op.package.package_record.version,
                    install_platform,
                ))
            } else {
                None
            }
        })
        .map_or(Ok(None), |info| info.map(Some))?;

    // Execute all operations in order.
    let package_cache = PackageCache::new(cache_dir.join("pkgs"));
    let install_driver = InstallDriver::default();
    let install_options = InstallOptions {
        python_info,
        platform: Some(install_platform),
        ..Default::default()
    };

    // Create a progress bars
    let multi_progress = indicatif::MultiProgress::new();
    let total_packages_to_download = result
        .iter()
        .filter(|op| {
            matches!(
                op.kind,
                PackageOperationKind::Install | PackageOperationKind::Reinstall
            )
        })
        .count();
    let download_pb = multi_progress.add(
        indicatif::ProgressBar::new(total_packages_to_download as u64)
            .with_style(default_progress_style())
            .with_finish(indicatif::ProgressFinish::WithMessage("Done!".into()))
            .with_prefix("downloading"),
    );
    download_pb.enable_steady_tick(Duration::from_millis(100));

    let total_operations = result.len();
    let link_pb = multi_progress.add(
        indicatif::ProgressBar::new(total_operations as u64)
            .with_style(default_progress_style())
            .with_finish(indicatif::ProgressFinish::WithMessage("Done!".into()))
            .with_prefix("linking"),
    );
    link_pb.enable_steady_tick(Duration::from_millis(100));

    stream::iter(result)
        .map(Ok)
        .try_for_each_concurrent(50, |op| {
            let target_prefix = target_prefix.clone();
            let download_client = download_client.clone();
            let package_cache = &package_cache;
            let install_driver = &install_driver;
            let download_pb = &download_pb;
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

    Ok(())
}

async fn execute_operation(
    target_prefix: &PathBuf,
    download_client: Client,
    package_cache: &PackageCache,
    install_driver: &InstallDriver,
    download_pb: &ProgressBar,
    link_pb: &ProgressBar,
    op: PackageOperation,
    install_options: &InstallOptions,
) -> anyhow::Result<()> {
    // Determine the type of operation to perform
    match op.kind {
        PackageOperationKind::Install => {
            // Download or cache the package from the remote
            let package_dir = package_cache
                .get_or_fetch_from_url(
                    op.package.as_ref(),
                    op.package.url.clone(),
                    download_client.clone(),
                )
                .await?;

            // Increment the download progress bar
            download_pb.inc(1);
            if download_pb.length() == Some(download_pb.position()) {
                download_pb.set_style(finished_progress_style());
            }

            install_package_to_environment(
                &target_prefix,
                package_dir,
                op.package,
                &install_driver,
                install_options,
            )
            .await?;
        }
        PackageOperationKind::Remove => {
            remove_package_from_environment(&target_prefix, &op.package, &install_driver).await?;
        }
        PackageOperationKind::Reinstall => {
            // Download or cache the package from the remote
            let package_dir = package_cache
                .get_or_fetch_from_url(
                    op.package.as_ref(),
                    op.package.url.clone(),
                    download_client.clone(),
                )
                .await?;

            // Increment the download progress bar
            download_pb.inc(1);
            if download_pb.length() == Some(download_pb.position()) {
                download_pb.set_style(finished_progress_style());
            }

            remove_package_from_environment(&target_prefix, &op.package, &install_driver).await?;
            install_package_to_environment(
                &target_prefix,
                package_dir,
                op.package,
                &install_driver,
                install_options,
            )
            .await?;
        }
    }

    // Increment the link progress bar since we finished a step!
    link_pb.inc(1);
    if link_pb.length() == Some(link_pb.position()) {
        link_pb.set_style(finished_progress_style());
    }

    Ok(())
}

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
        extracted_package_dir: Some(package_dir),
        files: paths
            .iter()
            .map(|entry| entry.relative_path.clone())
            .collect(),
        paths_data: paths.into(),
        // TODO: Retrieve the requested spec for this package from the request
        requested_spec: None,
        // TODO: What to do with this?
        link: None,
    };

    // Create the conda-meta directory if it doesnt exist yet.
    let target_prefix = target_prefix.to_path_buf();
    match tokio::task::spawn_blocking(move || {
        let conda_meta_path = target_prefix.join("conda-meta");
        std::fs::create_dir_all(&conda_meta_path)?;

        // Write the conda-meta information
        let pkg_meta_path = conda_meta_path.join(format!(
            "{}-{}-{}.json",
            prefix_record.repodata_record.package_record.name,
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

async fn remove_package_from_environment(
    _target_prefix: &Path,
    _package: &RepoDataRecord,
    _install_driver: &InstallDriver,
) -> anyhow::Result<()> {
    // println!("removing {}", package.package_record.name);
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

/// Given a channel and platform, download and cache the `repodata.json` for it. This function
/// reports its progress via a CLI progressbar.
async fn fetch_repo_data_records_with_progress(
    channel: Channel,
    platform: Platform,
    repodata_cache: &Path,
    client: Client,
    multi_progress: indicatif::MultiProgress,
) -> Result<Vec<RepoDataRecord>, anyhow::Error> {
    // Create a progress bar
    let progress_bar = multi_progress.add(
        indicatif::ProgressBar::new(1)
            .with_finish(indicatif::ProgressFinish::AndLeave)
            .with_prefix(format!("{}/{platform}", friendly_channel_name(&channel)))
            .with_style(default_bytes_style()),
    );
    progress_bar.enable_steady_tick(Duration::from_millis(100));

    // Download the repodata.json
    let download_progress_progress_bar = progress_bar.clone();
    let result = rattler_repodata_gateway::fetch::fetch_repo_data(
        channel.platform_url(platform),
        client,
        &repodata_cache,
        FetchRepoDataOptions {
            download_progress: Some(Box::new(move |DownloadProgress { total, bytes }| {
                download_progress_progress_bar.set_length(total.unwrap_or(bytes));
                download_progress_progress_bar.set_position(bytes);
            })),
            ..Default::default()
        },
    )
    .await;

    // Error out if an error occurred, but also update the progress bar
    let result = match result {
        Err(e) => {
            progress_bar.set_style(errored_progress_style());
            progress_bar.finish_with_message("Error");
            return Err(e.into());
        }
        Ok(result) => result,
    };

    // Notify that we are deserializing
    progress_bar.set_style(deserializing_progress_style());
    progress_bar.set_message("Deserializing..");

    // Deserialize the data. This is a hefty blocking operation so we spawn it as a tokio blocking
    // task.
    let repo_data_json_path = result.repo_data_json_path.clone();
    match tokio::task::spawn_blocking(move || {
        RepoData::from_path(repo_data_json_path)
            .map(move |repodata| repodata.into_repo_data_records(&channel))
    })
    .await
    {
        Ok(Ok(repodata)) => {
            progress_bar.set_style(finished_progress_style());
            let is_cache_hit = matches!(
                result.cache_result,
                CacheResult::CacheHit | CacheResult::CacheHitAfterFetch
            );
            progress_bar.finish_with_message(if is_cache_hit { "Using cache" } else { "Done" });
            Ok(repodata)
        }
        Ok(Err(err)) => {
            progress_bar.set_style(errored_progress_style());
            progress_bar.finish_with_message("Error");
            Err(err.into())
        }
        Err(err) => {
            if let Ok(panic) = err.try_into_panic() {
                std::panic::resume_unwind(panic);
            }
            progress_bar.set_style(errored_progress_style());
            progress_bar.finish_with_message("Cancelled..");
            // Since the task was cancelled most likely the whole async stack is being cancelled.
            Ok(Vec::new())
        }
    }
}

/// Returns a friendly name for the specified channel.
fn friendly_channel_name(channel: &Channel) -> String {
    channel
        .name
        .as_ref()
        .map(String::from)
        .unwrap_or_else(|| channel.canonical_name())
}

/// Returns the style to use for a progressbar that is currently in progress.
fn default_bytes_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{wide_bar:.bright.yellow/dim.white}] {bytes:>8} @ {bytes_per_sec:8}").unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is currently in progress.
fn default_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{wide_bar:.bright.yellow/dim.white}] {pos:>7}/{len:7}").unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is in Deserializing state.
fn deserializing_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] {wide_msg}")
        .unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is finished.
fn finished_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template(&format!("{} {{prefix:20!}} [{{elapsed_precise}}] {{msg:.bold}}", console::style(console::Emoji("✔", "")).green()))
        .unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is in error state.
fn errored_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template(&format!("{} {{prefix:20!}} [{{elapsed_precise}}] {{msg:.bold.red}}", console::style(console::Emoji("❌", "")).red()))
        .unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is indeterminate and simply shows a spinner.
fn long_running_progress_style() -> indicatif::ProgressStyle {
    ProgressStyle::with_template("{spinner:.green} {msg}").unwrap()
}
