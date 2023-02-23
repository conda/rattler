use futures::StreamExt;
use indicatif::{MultiProgress, ProgressFinish, ProgressStyle};
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, Platform, RepoData, RepoDataRecord};
use rattler_repodata_gateway::fetch::{CacheResult, DownloadProgress, FetchRepoDataOptions};
use rattler_solve::{RequestedAction, SolverBackend, SolverProblem};
use reqwest::Client;
use std::path::Path;
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
    let multi_progress = indicatif::MultiProgress::new();
    let repodata_cache = cache_dir.join("repodata");
    let channel_and_platform_len = channel_urls.len();
    let repodatas = futures::stream::iter(channel_urls)
        .map(move |(channel, platform)| {
            let repodata_cache = repodata_cache.clone();
            let download_client = download_client.clone();
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

    // Now that we parsed and downloaded all information, construct the packaging problem that we
    // need to solve. We do this by constructing a `SolverProblem`. This encapsulates all the
    // information required to be able to solve the problem.
    let solver_problem = SolverProblem {
        available_packages: repodatas,
        installed_packages: vec![],
        virtual_packages: vec![],
        specs,
    };

    // Next, use a solver to solve this specific problem. This provides us with all the operations
    // we need to apply to our environment to bring it up to date.
    let result = rattler_solve::LibsolvSolver.solve(solver_problem)?;

    

    Ok(())
}

async fn fetch_repo_data_records_with_progress(
    channel: Channel,
    platform: Platform,
    repodata_cache: &Path,
    client: Client,
    multi_progress: MultiProgress,
) -> Result<Vec<RepoDataRecord>, anyhow::Error> {
    // Create a progress bar
    let progress_bar = multi_progress.add(
        indicatif::ProgressBar::new(1)
            .with_finish(ProgressFinish::AndLeave)
            .with_prefix(format!("{}/{platform}", friendly_channel_name(&channel)))
            .with_style(default_progress_style()),
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
            progress_bar.finish_with_message(if is_cache_hit { "No changes" } else { "Done" });
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

fn friendly_channel_name(channel: &Channel) -> String {
    channel
        .name
        .as_ref()
        .map(String::from)
        .unwrap_or_else(|| channel.canonical_name())
}

/// Returns the style to use for a progressbar that is currently in progress.
fn default_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{bar:.bright.yellow/dim.white}] {bytes:>8} @ {bytes_per_sec:8}").unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is in Deserializing state.
fn deserializing_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{bar:.bright.green/dim.white}] {wide_msg}").unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is finished.
fn finished_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("  {prefix:20!} [{elapsed_precise}] {msg:.bold}")
        .unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is in error state.
fn errored_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("  {prefix:20!} [{elapsed_precise}] {msg:.bold.red}")
        .unwrap()
        .progress_chars("━━╾─")
}
