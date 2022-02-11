use indicatif::{ProgressBar, ProgressFinish, ProgressStyle};
use itertools::Itertools;
use rattler::{Channel, ChannelConfig, LoadRepoDataProgress, RepoDataLoader};
use structopt::StructOpt;
use thiserror::Error;

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

    let cache_dir = dirs::cache_dir().ok_or_else(|| {
        anyhow::anyhow!("could not determine cache directory for current platform")
    })?;

    // Create repo data loaders for
    let client = reqwest::Client::builder().build()?;
    let multi_progress = indicatif::MultiProgress::new();

    let repo_data_sources = channels
        .into_iter()
        .flat_map(|channel| {
            channel
                .platforms_or_default()
                .into_iter()
                .copied()
                .map(|platform| (channel.clone(), platform))
                .collect_vec()
        })
        .collect_vec();

    let client_ref = &client;
    let download_futures = repo_data_sources.iter()
        .map(|(channel, platform)| {
        let progress_bar = multi_progress.add(ProgressBar::new(0));
            progress_bar.set_prefix(format!("{}/{}", &channel.name, platform));
        progress_bar.set_style(ProgressStyle::default_bar()
            .on_finish(ProgressFinish::WithMessage("Done!".into()))
            .template(&format!("{{spinner:.green}} {{prefix:20!}} [{{elapsed_precise}}] [{{bar:30!.green/blue}}] {{bytes:>8}}/{{total_bytes:<8}} @ {{bytes_per_sec:8}}"))
            .progress_chars("=>-"));
        progress_bar.enable_steady_tick(100);
        let loader = RepoDataLoader::new(channel.clone(), platform.clone(), &cache_dir);
        async move {
            match loader.load(client_ref, |progress| match progress {
                LoadRepoDataProgress::Downloading { progress, total } => {
                    progress_bar.set_length(total.unwrap_or(progress) as u64);
                    progress_bar.set_position(progress as u64);
                }
                LoadRepoDataProgress::Decoding => {}
            }).await {
                Ok(repo_data) => {
                    progress_bar.set_style(ProgressStyle::default_bar()
                        .template(&format!("  {{prefix:20!}} [{{elapsed_precise}}] {{msg:.green}}")));
                    progress_bar.set_message("Done!");
                    progress_bar.finish();
                    Ok(repo_data)
                },
                Err(err) => {
                    progress_bar.set_style(ProgressStyle::default_bar()
                        .template(&format!("  {{prefix:20!}} [{{elapsed_precise}}] {{msg:.red}}"))
                        .progress_chars("=>-"));
                    progress_bar.set_message("Error!");
                    progress_bar.finish();
                    Err(err)
                }
            }
        }
    }
    );

    let results = futures::future::join_all(download_futures).await;
    for result in results.iter() {
        match result {
            Ok(_repo_data) => {}
            Err(err) => {
                log::error!("{}", err);
            }
        }
    }

    //
    // // Get the urls for the repodata
    // let repodatas = channels
    //     .iter()
    //     .map(|channel| {
    //         let platforms = channel.platforms_url();
    //         platforms
    //             .into_iter()
    //             .map(move |(platform, url)| (channel, platform, url))
    //     })
    //     .flatten()
    //     .map(|(channel, platform, url)| (channel, platform, url.join("repodata.json").unwrap()));
    //
    // // Create a client
    // let client = reqwest::Client::builder()
    //     .deflate(true)
    //     .gzip(true)
    //     .build()?;
    // let multi_progress = indicatif::MultiProgress::new();
    //
    // let download_targets = repodatas
    //     .into_iter()
    //     .map(|(channel, platform, url)| {
    //         let progress_bar = multi_progress.add(ProgressBar::new(0));
    //         progress_bar.set_style(ProgressStyle::default_bar()
    //             .template(&format!("{{spinner:.green}} {}/{} [{{elapsed_precise}}] [{{bar:20}}] {{bytes}}/{{total_bytes}} {{msg}}", &channel.name, platform.as_str()))
    //             .progress_chars("=> "));
    //         progress_bar.enable_steady_tick(100);
    //         (channel, platform, url, progress_bar)
    //     });
    //
    // // Download them all!
    // let client = &client;
    // let repodata: Vec<Repodata> = futures::future::try_join_all(download_targets.into_iter().map(
    //     |(channel, platform, url, progress_bar)| async move {
    //         match download_repo_data_with_progress(client, channel, platform, url, &progress_bar)
    //             .await
    //         {
    //             Ok(repodata) => {
    //                 progress_bar.set_style(ProgressStyle::default_bar().template(&format!(
    //                     "{{spinner:.green}} {}/{} [{{elapsed_precise}}] {{msg}}",
    //                     &channel.name,
    //                     platform.as_str()
    //                 )));
    //                 progress_bar.finish_with_message("Done");
    //                 Ok(repodata)
    //             }
    //             Err(err) => {
    //                 progress_bar.abandon_with_message("Error");
    //                 Err(err)
    //             }
    //         }
    //     },
    // ))
    // .await?;
    //
    // // Construct an index
    // let mut index = Index::default();
    //
    // // Add all records from the repodata
    // for repo_data in repodata {
    //     for (package_filename, package_info) in repo_data.packages {
    //         if !repo_data.removed.contains(&package_filename) {
    //             if let Err(e) = index.add_record(&package_info) {
    //                 tracing::warn!("couldn't add {}: {}", package_filename, e);
    //             }
    //         }
    //     }
    // }
    //
    // // Construct a fake package just for us
    // let root_version = Version::lowest();
    // let root_package = PackageRecord {
    //     name: "__solver".to_string(),
    //     build: "".to_string(),
    //     build_number: 0,
    //     depends: vec![
    //         String::from("ros-noetic-cob-cam3d-throttle"),
    //         String::from("ros-distro-mutex"),
    //     ],
    //     constrains: vec![],
    //     license: None,
    //     license_family: None,
    //     md5: "".to_string(),
    //     sha256: None,
    //     size: 0,
    //     subdir: "".to_string(),
    //     timestamp: None,
    //     version: root_version.clone(),
    // };
    //
    // index.add_record(&root_package)?;
    //
    // println!("setup the index");
    //
    // match resolve(&index, root_package.name, root_version) {
    //     Ok(result) => {
    //         let pinned_packages: Vec<_> = result.into_iter().collect();
    //         let longest_package_name = pinned_packages
    //             .iter()
    //             .map(|(package_name, _)| package_name.len())
    //             .max()
    //             .unwrap_or(0);
    //
    //         for (package, version) in pinned_packages.iter() {
    //             println!(
    //                 "{:<longest_package_name$} {}",
    //                 package,
    //                 version,
    //                 longest_package_name = longest_package_name
    //             )
    //         }
    //     }
    //     Err(PubGrubError::NoSolution(mut derivation_tree)) => {
    //         derivation_tree.collapse_no_versions();
    //         eprintln!("{}", DefaultStringReporter::report(&derivation_tree));
    //     }
    //     Err(e) => eprintln!("could not find a solution!\n{}", e),
    // }

    Ok(())
}
