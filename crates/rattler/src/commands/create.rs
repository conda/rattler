use crate::conda::{Channel, ChannelConfig, Platform, Record, Repodata, Version};
use crate::solver::Index;
use bytes::BufMut;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use pubgrub::error::PubGrubError;
use pubgrub::report::{DefaultStringReporter, Reporter};
use pubgrub::solver::resolve;
use pubgrub::version::Version as PubGrubVersion;
use structopt::StructOpt;
use thiserror::Error;
use url::Url;

#[derive(Debug, StructOpt)]
pub struct Opt {
    #[structopt(short)]
    channels: Option<Vec<String>>,
}

#[derive(Error, Debug)]
enum DownloadError {
    #[error("error deserializing repository data: {0}")]
    DeserializeError(#[source] serde_json::Error),

    #[error("error downloading data: {0}")]
    TransportError(#[source] reqwest::Error),
}

impl From<serde_json::Error> for DownloadError {
    fn from(e: serde_json::Error) -> Self {
        DownloadError::DeserializeError(e)
    }
}

impl From<reqwest::Error> for DownloadError {
    fn from(e: reqwest::Error) -> Self {
        DownloadError::TransportError(e)
    }
}

pub async fn create(_opt: Opt) -> anyhow::Result<()> {
    let channel_config = ChannelConfig::default();

    // Get the channels to download
    let channels = vec![
        Channel::from_str("conda-forge", &channel_config)?,
        Channel::from_str("robostack", &channel_config)?,
    ];

    // Get the urls for the repodata
    let repodatas = channels
        .iter()
        .map(|channel| {
            let platforms = channel.platforms_url();
            platforms
                .into_iter()
                .map(move |(platform, url)| (channel, platform, url))
        })
        .flatten()
        .map(|(channel, platform, url)| (channel, platform, url.join("repodata.json").unwrap()));

    // Create a client
    let client = reqwest::Client::builder()
        .deflate(true)
        .gzip(true)
        .build()?;
    let multi_progress = indicatif::MultiProgress::new();

    let download_targets = repodatas
        .into_iter()
        .map(|(channel, platform, url)| {
            let progress_bar = multi_progress.add(ProgressBar::new(0));
            progress_bar.set_style(ProgressStyle::default_bar()
                .template(&format!("{{spinner:.green}} {}/{} [{{elapsed_precise}}] [{{bar:20}}] {{bytes}}/{{total_bytes}} {{msg}}", &channel.name, platform.as_str()))
                .progress_chars("=> "));
            progress_bar.enable_steady_tick(100);
            (channel, platform, url, progress_bar)
        });

    // Download them all!
    let client = &client;
    let repodata: Vec<Repodata> = futures::future::try_join_all(download_targets.into_iter().map(
        |(channel, platform, url, progress_bar)| async move {
            match download_repo_data_with_progress(client, channel, platform, url, &progress_bar)
                .await
            {
                Ok(repodata) => {
                    progress_bar.set_style(ProgressStyle::default_bar().template(&format!(
                        "{{spinner:.green}} {}/{} [{{elapsed_precise}}] {{msg}}",
                        &channel.name,
                        platform.as_str()
                    )));
                    progress_bar.finish_with_message("Done");
                    Ok(repodata)
                }
                Err(err) => {
                    progress_bar.abandon_with_message("Error");
                    Err(err)
                }
            }
        },
    ))
    .await?;

    // Construct an index
    let mut index = Index::default();

    // Add all records from the repodata
    for repo_data in repodata {
        for (package_filename, package_info) in repo_data.packages {
            if !repo_data.removed.contains(&package_filename) {
                if let Err(e) = index.add_record(&package_info) {
                    tracing::warn!("couldn't add {}: {}", package_filename, e);
                }
            }
        }
    }

    // Construct a fake package just for us
    let root_version = Version::lowest();
    let root_package = Record {
        name: "__solver".to_string(),
        build: "".to_string(),
        build_number: 0,
        depends: vec![
            String::from("ros-noetic-cob-cam3d-throttle"),
            String::from("ros-distro-mutex"),
        ],
        constrains: vec![],
        license: None,
        license_family: None,
        md5: "".to_string(),
        sha256: None,
        size: 0,
        subdir: "".to_string(),
        timestamp: None,
        version: root_version.to_string(),
    };

    index.add_record(&root_package)?;

    println!("setup the index");

    match resolve(&index, root_package.name, root_version) {
        Ok(result) => {
            let pinned_packages: Vec<_> = result.into_iter().collect();
            let longest_package_name = pinned_packages
                .iter()
                .map(|(package_name, _)| package_name.len())
                .max()
                .unwrap_or(0);

            for (package, version) in pinned_packages.iter() {
                println!(
                    "{:<longest_package_name$} {}",
                    package,
                    version,
                    longest_package_name = longest_package_name
                )
            }
        }
        Err(PubGrubError::NoSolution(mut derivation_tree)) => {
            derivation_tree.collapse_no_versions();
            eprintln!("{}", DefaultStringReporter::report(&derivation_tree));
        }
        Err(e) => eprintln!("could not find a solution!\n{}", e),
    }

    Ok(())
}

async fn download_repo_data_with_progress(
    client: &reqwest::Client,
    channel: &Channel,
    platform: Platform,
    url: Url,
    progress_bar: &ProgressBar,
) -> Result<Repodata, DownloadError> {
    let response = client.get(url).send().await.map_err(DownloadError::from)?;

    let total_size = response.content_length().unwrap_or(100);
    progress_bar.set_length(total_size);
    progress_bar.set_message("Downloading...");

    let mut downloaded = 0;
    let mut byte_stream = response.bytes_stream();
    let mut bytes = Vec::with_capacity(total_size as usize);
    while let Some(chunk) = byte_stream.next().await {
        let chunk = chunk?;
        let new = downloaded + chunk.len() as u64;
        bytes.put(chunk);
        downloaded = new;
        progress_bar.set_length(new.max(total_size));
        progress_bar.set_position(new);
    }

    progress_bar.set_style(ProgressStyle::default_bar().template(&format!(
        "{{spinner:.green}} {}/{} [{{elapsed_precise}}] {{msg}}",
        &channel.name,
        platform.as_str()
    )));
    progress_bar.set_message("Parsing...");
    serde_json::from_slice(&bytes).map_err(DownloadError::from)
}
