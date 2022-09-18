use anyhow::Context;
use rattler::repo_data::fetch::{terminal_progress, MultiRequestRepoDataBuilder};
use rattler::{Channel, ChannelConfig, ChannelData, Platform};
use std::str::FromStr;

#[derive(Debug, clap::Parser)]
pub struct Opts {
    channel: String,
}

/// Given a channel extract the metadata of all packages.
pub async fn extract_metadata(opts: Opts) -> anyhow::Result<()> {
    let channel = Channel::from_str(opts.channel, &ChannelConfig::default())?;

    // Construct an HTTP client
    let client = reqwest::Client::new();

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
            println!("fetching {url}");
        }
    }

    Ok(())
}
