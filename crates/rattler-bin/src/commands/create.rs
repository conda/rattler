use futures::StreamExt;
use indicatif::{MultiProgress, ProgressFinish, ProgressStyle};
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, Platform, RepoData, RepoDataRecord};
use rattler_repodata_gateway::fetch::{CacheResult, DownloadProgress, FetchRepoDataOptions};
use reqwest::Client;
use std::io::Error;
use std::path::Path;
use std::time::Duration;
use tokio::task::JoinHandle;

#[derive(Debug, clap::Parser)]
pub struct Opt {
    #[clap(short)]
    channels: Option<Vec<String>>,

    #[clap(required = true)]
    specs: Vec<String>,
}

pub async fn create(opt: Opt) -> anyhow::Result<()> {
    let channel_config = ChannelConfig::default();

    // Parse the match specs
    let _specs = opt
        .specs
        .iter()
        .map(|spec| MatchSpec::from_str(spec, &channel_config))
        .collect::<Result<Vec<_>, _>>()?;

    // Get the cache directory
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory for current platform"))?
        .join("rattler/cache");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| anyhow::anyhow!("could not create cache directory: {}", e))?;

    // Get the channels to download
    let channels = opt
        .channels
        .unwrap_or_else(|| vec![String::from("conda-forge")])
        .into_iter()
        .map(|channel_str| Channel::from_str(&channel_str, &channel_config))
        .collect::<Result<Vec<_>, _>>()?;

    // Get the channels in combination with the platforms
    let channel_urls = channels
        .iter()
        .flat_map(|channel| {
            channel
                .platforms_or_default()
                .into_iter()
                .map(move |platform| (channel.clone(), *platform))
        })
        .collect::<Vec<_>>();

    // Start downloading each repodata.json
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

    // // Download all repo data from the channels and create an index
    // let repo_data_per_source = MultiRequestRepoDataBuilder::default()
    //     .set_cache_dir(&cache_dir)
    //     .set_listener(terminal_progress())
    //     .set_fail_fast(false)
    //     .add_channels(channels)
    //     .request()
    //     .await;
    //
    // // Error out if fetching one of the sources resulted in an error.
    // let repo_data = repo_data_per_source
    //     .into_iter()
    //     .map(|(channel, _, result)| result.map(|data| (channel, data)))
    //     .collect::<Result<Vec<_>, _>>()?;
    //
    // let solver_problem = SolverProblem {
    //     channels: repo_data
    //         .iter()
    //         .map(|(channel, repodata)| (channel.base_url().to_string(), repodata))
    //         .collect(),
    //     specs,
    // };
    //
    // let result = solver_problem.solve()?;
    // println!("{:#?}", result);

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
