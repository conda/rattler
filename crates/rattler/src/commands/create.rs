use futures::{StreamExt, TryFutureExt};
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache};
use indicatif::{ProgressBar, ProgressFinish, ProgressStyle};
use itertools::Itertools;
use pubgrub::error::PubGrubError;
use pubgrub::report::{DefaultStringReporter, Reporter};
use pubgrub::solver::resolve;
use rattler::{
    Channel, ChannelConfig, FetchRepoDataError, FetchRepoDataProgress, PackageIndex, PackageRecord,
    RepoData, SolverIndex, Version,
};
use reqwest_middleware::ClientBuilder;
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_retry::RetryTransientMiddleware;
use std::str::FromStr;
use structopt::StructOpt;
use thiserror::Error;

#[derive(Debug, StructOpt)]
pub struct Opt {
    #[structopt(short)]
    channels: Option<Vec<String>>,
}

pub async fn create(_opt: Opt) -> anyhow::Result<()> {
    let channel_config = ChannelConfig::default();

    // Get the channels to download
    let channels = vec![
        Channel::from_str("conda-forge", &channel_config)?,
        Channel::from_str("robostack", &channel_config)?,
    ];

    // Download all repo data from the channels and create an index
    let repo_data = load_channels(&channels).await?;
    let index = PackageIndex::from(repo_data);

    let mut solve_index = SolverIndex::new(index);

    let root_package_name = String::from("__solver");
    let root_version = Version::from_str("1").unwrap();
    let root_package = PackageRecord {
        name: root_package_name.clone(),
        version: root_version.clone(),
        build: "".to_string(),
        build_number: 0,
        subdir: "".to_string(),
        md5: None,
        sha256: None,
        arch: None,
        platform: None,
        depends: vec![String::from("ros-noetic-rviz")],
        constrains: vec![],
        track_features: None,
        features: None,
        preferred_env: None,
        license: None,
        license_family: None,
        timestamp: None,
        date: None,
        size: None,
    };

    solve_index.add(root_package.clone());

    match resolve(&solve_index, root_package_name, root_package) {
        Ok(result) => {
            let pinned_packages: Vec<_> = result.into_iter().collect();
            let longest_package_name = pinned_packages
                .iter()
                .map(|(package_name, _)| package_name.len())
                .max()
                .unwrap_or(0);

            println!("Found a solution!");
            for (package, version) in pinned_packages.iter() {
                println!(
                    "- {:<longest_package_name$} {}",
                    package,
                    version,
                    longest_package_name = longest_package_name
                )
            }
        }
        Err(PubGrubError::NoSolution(mut derivation_tree)) => {
            derivation_tree.collapse_no_versions();
            eprintln!(
                "Could not find a solution:\n{}",
                DefaultStringReporter::report(&derivation_tree)
            );
        }
        Err(e) => eprintln!("could not find a solution!\n{}", e),
    }

    Ok(())
}

#[derive(Error, Debug)]
enum LoadChannelsError {
    #[error("error fetching repodata")]
    FetchErrors(Vec<FetchRepoDataError>),
    #[error("{0}")]
    IoError(#[from] std::io::Error),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// Interactively loads the [`RepoData`] of the specified channels.
async fn load_channels<'c, I: IntoIterator<Item = &'c Channel> + 'c>(
    channels: I,
) -> Result<Vec<RepoData>, LoadChannelsError> {
    // Get the cache directory
    let http_cache_dir = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory for current platform"))?
        .join("rattler/cache");
    std::fs::create_dir_all(&http_cache_dir)
        .map_err(|e| anyhow::anyhow!("could not create cache directory: {}", e))?;

    // Construct a client with a cache and retry policy
    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
    let client = ClientBuilder::new(reqwest::Client::new())
        .with(Cache(HttpCache {
            mode: CacheMode::Default,
            manager: CACacheManager {
                path: http_cache_dir.to_string_lossy().to_string(),
            },
            options: None,
        }))
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build();

    // Setup the progress bar
    let multi_progress = indicatif::MultiProgress::new();
    let default_progress_style = ProgressStyle::default_bar()
        .template(&format!("{{spinner:.green}} {{prefix:20!}} [{{elapsed_precise}}] [{{bar:30!.green/blue}}] {{bytes:>8}}/{{total_bytes:<8}} @ {{bytes_per_sec:8}}"))
        .progress_chars("=> ");
    let finished_progress_tyle = ProgressStyle::default_bar().template(&format!(
        "  {{prefix:20!}} [{{elapsed_precise}}] {{msg:.bold}}"
    ));
    let errorred_progress_tyle = ProgressStyle::default_bar().template(&format!(
        "  {{prefix:20!}} [{{elapsed_precise}}] {{msg:.red/bold}}"
    ));

    // Iterate over all channel and platform permutations
    let (repo_datas, errors): (Vec<_>, Vec<_>) = futures::future::join_all(
        channels
            .into_iter()
            .flat_map(move |channel| {
                channel
                    .platforms_or_default()
                    .into_iter()
                    .map(move |platform| (channel, *platform))
            })
            .map(move |(channel, platform)| {
                // Create progress bar
                let progress_bar = multi_progress.add(ProgressBar::new(0));
                progress_bar.set_prefix(format!("{}/{}", &channel.name, platform));
                progress_bar.set_style(default_progress_style.clone());
                progress_bar.enable_steady_tick(100);
                let client = client.clone();
                let errorred_progress_tyle = errorred_progress_tyle.clone();
                let finished_progress_tyle = finished_progress_tyle.clone();
                async move {
                    match channel
                        .fetch_repo_data(&client, platform, |progress| match progress {
                            FetchRepoDataProgress::Downloading { progress, total } => {
                                if let Some(total) = total {
                                    progress_bar.set_length(total as u64);
                                    progress_bar.set_position(progress as u64)
                                }
                            }
                            _ => {}
                        })
                        .await
                    {
                        Ok(repo_data) => {
                            progress_bar.set_style(finished_progress_tyle.clone());
                            progress_bar.set_message("Done!");
                            progress_bar.finish();
                            Ok(repo_data)
                        }
                        Err(err) => {
                            progress_bar.set_style(errorred_progress_tyle.clone());
                            progress_bar.set_message("Error!");
                            progress_bar.finish_at_current_pos();
                            Err(err)
                        }
                    }
                }
            }),
    )
    .await
    .into_iter()
    .partition(Result::is_ok);

    if !errors.is_empty() {
        Err(LoadChannelsError::FetchErrors(
            errors.into_iter().map(Result::unwrap_err).collect(),
        ))
    } else {
        Ok(repo_datas.into_iter().map(Result::unwrap).collect())
    }
}
