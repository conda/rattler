use anyhow::Context;
use futures::stream;
use futures::FutureExt;
use futures::StreamExt;
use futures::TryStreamExt;
use indicatif::ProgressState;
use rattler::repo_data::fetch::{terminal_progress, MultiRequestRepoDataBuilder};
use rattler::{Channel, ChannelConfig, ChannelData, PackageArchiveFormat, Platform};
use std::fmt::Write;
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;
use tokio::io::AsyncRead;
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
    let mut index_json_futures = Vec::new();
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

            let fetch_index_future =
                fetch_index_json(client.clone(), url.clone()).map(move |result| {
                    result.with_context(move || format!("while fetching index.json of {url}"))
                });

            index_json_futures.push(fetch_index_future);
        }
    }

    // Convert the futures into a stream of futures which we can then buffer
    let num_futures = index_json_futures.len();
    let mut index_json_futures_stream =
        stream::iter(index_json_futures.into_iter()).buffer_unordered(50);

    // Iterate over all futures and update a progress bar
    let progress_bar = indicatif::ProgressBar::with_draw_target(
        Some(num_futures as u64),
        indicatif::ProgressDrawTarget::stdout_with_hz(2),
    );
    progress_bar.set_style(indicatif::ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] [{bar:40.bright.yellow/dim.white}] [{pos:>7}/{len:7}][{per_sec:3}] {eta_precise}")
        .unwrap()
        // .with_key("per_sec", |state: &ProgressState, w: &mut dyn Write| write!(w, "{:.0}", state.pos() as f64/state.elapsed().as_secs_f64()).unwrap())
        .progress_chars("━━╾─"));
    // progress_bar.enable_steady_tick(Duration::from_millis(500));
    while let Some(result) = index_json_futures_stream.next().await {
        let _index = result?;
        progress_bar.inc(1);
    }

    Ok(())
}

#[instrument(skip_all, err, fields(url=%url))]
async fn fetch_index_json(client: reqwest::Client, url: Url) -> anyhow::Result<serde_json::Value> {
    // Find the package format
    let (name, format) = match PackageArchiveFormat::from_file_name(url.path()) {
        Some(result) => result,
        None => anyhow::bail!("could not determine package archive format"),
    };

    let name = name.trim_start_matches('/');

    // If the file already exists we're done
    let package_path = Path::new(name);
    let index_path = package_path.join("info/index.json");
    if index_path.is_file() {
        let index_path = index_path.clone();
        match tokio::task::spawn_blocking(move || {
            std::fs::read_to_string(index_path)
                .map_err(Into::<anyhow::Error>::into)
                .and_then(|str| serde_json::from_str(&str).map_err(Into::<anyhow::Error>::into))
        })
        .await
        {
            Ok(Ok(value)) => return Ok(value),
            Err(err) => {
                if let Ok(reason) = err.try_into_panic() {
                    // Resume the panic on the main task
                    std::panic::resume_unwind(reason);
                }
            }
            _ => tracing::warn!("failed to read cached index.json as json. Redownloading.."),
        }
    }

    // Download the file, retry 4 times
    let mut retry_count = 0;
    loop {
        match download_index_json(name, client.clone(), &url, format).await {
            Err(e) => {
                if retry_count < 4 {
                    tracing::error!("failed to download: {e}. Retrying in 500ms..");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    retry_count += 1;
                    continue;
                }
                return Err(e);
            }
            Ok(_) => break,
        }
    }

    match tokio::task::spawn_blocking(move || {
        std::fs::read_to_string(index_path)
            .map_err(Into::<anyhow::Error>::into)
            .and_then(|str| serde_json::from_str(&str).map_err(Into::<anyhow::Error>::into))
    })
    .await
    {
        Ok(v) => v,
        Err(err) => match err.try_into_panic() {
            Ok(reason) => std::panic::resume_unwind(reason),
            Err(err) => Err(err.into()),
        },
    }
}

/// Downloads the archive but only really fetch the index.json file from the archive and extracts
/// it to the package_path. This is done by simply streaming the contents, decompressing on the
/// fly, and stopping when we have the file we need.
async fn download_index_json(
    package_path: impl AsRef<Path>,
    client: reqwest::Client,
    url: &Url,
    format: PackageArchiveFormat,
) -> anyhow::Result<()> {
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
            extract_index_json(package_path, decompressed_bytes).await?;
        }
        PackageArchiveFormat::TarZst => {}
        PackageArchiveFormat::Conda => {
            todo!();
        }
    }

    Ok(())
}

/// Given a stream of bytes, reads the content and extracts the index.json file only. Stops
/// when the file has been extracted.
async fn extract_index_json(
    target: impl AsRef<Path>,
    bytes: impl AsyncRead + Send + Unpin,
) -> anyhow::Result<()> {
    let target = target.as_ref();
    let mut archive = Archive::new(bytes);
    let mut entries = archive.entries()?;
    let mut pinned = Pin::new(&mut entries);
    while let Some(entry) = pinned.next().await {
        let mut file = entry.with_context(|| "iterating archive")?;
        let path = file.path()?;
        if path == Path::new("info/index.json") {
            // Create the directory
            std::fs::create_dir_all(target)?;
            file.unpack_in(target).await?;
            return Ok(());
        }
    }
    Err(anyhow::anyhow!("index.json was not found in the archive"))
}
