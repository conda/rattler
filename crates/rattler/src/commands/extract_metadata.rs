use anyhow::Context;
use futures::StreamExt;
use futures::TryStreamExt;
use rattler::repo_data::fetch::{terminal_progress, MultiRequestRepoDataBuilder};
use rattler::{Channel, ChannelConfig, ChannelData, PackageArchiveFormat, Platform};
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use tokio::io::{AsyncBufRead, AsyncRead};
use tokio_tar::Archive;
use tokio_util::io::StreamReader;
use tracing::instrument;
use url::Url;

#[derive(Debug, clap::Parser)]
pub struct Opts {
    channel: String,
}

/// Given a channel extract the metadata of all packages.
pub async fn extract_metadata(opts: Opts) -> anyhow::Result<()> {
    let channel = Channel::from_str(opts.channel, &ChannelConfig::default())?;

    // Construct an HTTP client
    let client = reqwest::Client::builder().gzip(true).build()?;

    // Request the channel data at the root of the channel
    let channel_data: ChannelData = client
        .get(
            channel
                .base_url()
                .join("channeldata.json")
                .expect("failed to append channelinfo.json"),
        )
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    // Get the cache directory
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory for current platform"))?
        .join("rattler/cache");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| anyhow::anyhow!("could not create cache directory: {}", e))?;

    // Fetch the repodata for all channels
    let mut repo_data_per_source = MultiRequestRepoDataBuilder::default()
        .set_cache_dir(&cache_dir)
        .set_listener(terminal_progress())
        .set_fail_fast(false);

    for subdir in channel_data.subdirs.iter() {
        let platform = match Platform::from_str(subdir.as_str()) {
            Ok(platform) => platform,
            Err(e) => {
                tracing::warn!("error parsing '{subdir}': {e}. Skipping.. ");
                continue;
            }
        };

        repo_data_per_source =
            repo_data_per_source.add_channel_and_platform(channel.clone(), platform);
    }

    // Iterate over all packages in all subdirs
    let repodatas = repo_data_per_source.request().await;
    for (subdir, platform, result) in repodatas {
        let repodata = match result {
            Err(e) => {
                tracing::error!("error fetching repodata for '{platform}': {e}");
                continue;
            }
            Ok(repodata) => repodata,
        };

        // Iterate over all packages in the subdir
        for package in repodata.packages.keys() {
            let url = subdir
                .platform_url(platform)
                .join(package)
                .expect("invalid channel file path");

            fetch_index_json(client.clone(), &url)
                .await
                .with_context(|| format!("while fetching index.json of {url}"))?;
        }
    }

    Ok(())
}

#[instrument(skip(client))]
async fn fetch_index_json(client: reqwest::Client, url: &Url) -> anyhow::Result<()> {
    // Find the package format
    let (name, format) = match PackageArchiveFormat::from_file_name(url.path()) {
        Some(result) => result,
        None => anyhow::bail!("could not determine package archive format"),
    };

    let name = name.trim_start_matches('/');

    // Create the directory
    std::fs::create_dir_all(name)?;

    // Download the file
    let bytes = client
        .get(url.clone())
        .send()
        .await?
        .error_for_status()?
        .bytes_stream();
    let byte_stream = StreamReader::new(bytes.map_err(|e| io::Error::new(io::ErrorKind::Other, e)));

    match format {
        PackageArchiveFormat::TarBz2 => {
            let decompressed_bytes = async_compression::tokio::bufread::BzDecoder::new(byte_stream);
            extract_index_json(name, decompressed_bytes).await?;
        }
        PackageArchiveFormat::TarZst => {}
        PackageArchiveFormat::Conda => {
            todo!();
        }
    }

    tracing::info!("finished");
    Ok(())
}

async fn extract_index_json(
    target: impl AsRef<Path>,
    bytes: impl AsyncRead + Send + Unpin,
) -> anyhow::Result<()> {
    let mut archive = Archive::new(bytes);
    let mut entries = archive.entries()?;
    let mut pinned = Pin::new(&mut entries);
    while let Some(entry) = pinned.next().await {
        let mut file = entry.with_context(|| "iterating archive")?;
        let path = file.path()?;
        if path == Path::new("info/index.json") {
            file.unpack_in(target).await?;
            return Ok(());
        }
    }
    Err(anyhow::anyhow!("index.json was not found in the archive"))
}
