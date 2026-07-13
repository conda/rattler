use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use rattler_conda_types::Platform;
use rattler_config::config::{
    concurrency::default_max_concurrent_solves, index::IndexChannelConfig,
};
use rattler_index::{
    ChannelMetadata, IndexFsConfig, PackageRevisionAssignment, index_fs_with_channel_metadata,
};
#[cfg(feature = "s3")]
use rattler_index::{IndexS3Config, PreconditionChecks, index_s3_with_channel_metadata};
#[cfg(feature = "s3")]
use rattler_networking::AuthenticationStorage;
#[cfg(feature = "s3")]
use rattler_s3::S3Credentials;
#[cfg(feature = "s3")]
use url::Url;

#[cfg(feature = "s3")]
fn parse_s3_url(value: &str) -> Result<Url, String> {
    let url: Url = Url::parse(value).map_err(|e| format!("`{value}` isn't a valid URL: {e}"))?;
    if url.scheme() == "s3" && url.host_str().is_some() {
        Ok(url)
    } else {
        Err(format!(
            "Only S3 URLs of format s3://bucket/... can be used, not `{value}`"
        ))
    }
}

/// The `rattler-index` CLI.
#[derive(Parser)]
#[command(name = "rattler-index", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[command(flatten)]
    verbosity: Verbosity,

    /// Whether to force the re-indexing of all packages.
    /// Note that this will create a new repodata.json instead of updating the
    /// existing one.
    #[arg(short, long, default_value = "false", global = true)]
    force: bool,

    /// The maximum number of packages to process in-memory simultaneously.
    /// This is necessary to limit memory usage when indexing large channels.
    #[arg(long, global = true)]
    max_parallel: Option<usize>,

    /// A specific platform to index.
    /// Defaults to all platforms available in the channel.
    #[arg(long, global = true)]
    target_platform: Option<Platform>,

    /// The name of the conda package (expected to be in the `noarch` subdir)
    /// that should be used for repodata patching. For more information, see `https://prefix.dev/blog/repodata_patching`.
    #[arg(long, global = true)]
    repodata_patch: Option<String>,

    /// Disable precondition checks (`ETags`, timestamps) during file operations.
    /// Use this flag if your S3 backend doesn't fully support conditional requests,
    /// or if you're certain no concurrent indexing processes are running.
    /// Warning: Disabling this removes protection against concurrent modifications.
    #[cfg(feature = "s3")]
    #[arg(long, default_value = "false", global = true)]
    disable_precondition_checks: bool,

    /// The path to the config file to use to configure rattler-index.
    /// Uses the same configuration format as pixi, see `https://pixi.sh/latest/reference/pixi_configuration`.
    /// Per-channel index options are read from the `index-config` section.
    #[arg(long)]
    config: Option<PathBuf>,
}

/// The subcommands for the `rattler-index` CLI.
#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Index a channel stored on the filesystem.
    #[command(name = "fs")]
    FileSystem {
        /// The path to the channel directory.
        #[arg()]
        channel: std::path::PathBuf,
    },

    /// Index a channel stored in an S3 bucket.
    #[cfg(feature = "s3")]
    S3 {
        /// The S3 channel URL, e.g. `s3://my-bucket/my-channel`.
        #[arg(value_parser = parse_s3_url)]
        channel: Url,

        #[clap(flatten)]
        credentials: rattler_s3::clap::S3CredentialsOpts,
    },
}

/// The configuration type for rattler-index - just extends rattler config and
/// can load the same TOML files as pixi.
pub type Config = rattler_config::config::ConfigBase;

/// Entry point of the `rattler-index` cli.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse the command line arguments
    let cli = Cli::parse();

    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(cli.verbosity)
        .init();

    let multi_progress = indicatif::MultiProgress::new();

    let config = if let Some(config_path) = cli.config {
        Some(Config::load_from_files(vec![config_path])?)
    } else {
        None
    };
    let max_parallel = cli
        .max_parallel
        .or(config.as_ref().map(|c| c.concurrency.downloads))
        .unwrap_or_else(default_max_concurrent_solves);

    #[cfg(feature = "s3")]
    let precondition_checks = if cli.disable_precondition_checks {
        PreconditionChecks::Disabled
    } else {
        PreconditionChecks::Enabled
    };

    match cli.command {
        Commands::FileSystem { channel } => {
            let target = channel
                .canonicalize()
                .unwrap_or_else(|_| channel.clone())
                .to_string_lossy()
                .into_owned();
            let resolved = resolve_index_channel_config(&config, &target);
            let (write_zst, write_shards, repodata_revisions, package_revision_assignment) =
                effective_index_options(&resolved);
            let channel_metadata = ChannelMetadata::from_index_config(&resolved);

            index_fs_with_channel_metadata(
                IndexFsConfig {
                    channel,
                    target_platform: cli.target_platform,
                    repodata_patch: cli.repodata_patch,
                    write_zst,
                    write_shards,
                    repodata_revisions,
                    package_revision_assignment,
                    force: cli.force,
                    max_parallel,
                    multi_progress: Some(multi_progress),
                },
                channel_metadata,
            )
            .await
        }
        #[cfg(feature = "s3")]
        Commands::S3 {
            channel,
            mut credentials,
        } => {
            let target = channel.to_string();
            let resolved = resolve_index_channel_config(&config, &target);
            let (write_zst, write_shards, repodata_revisions, package_revision_assignment) =
                effective_index_options(&resolved);
            let channel_metadata = ChannelMetadata::from_index_config(&resolved);

            let bucket = channel.host().context("Invalid S3 url")?.to_string();
            let s3_config = config
                .as_ref()
                .and_then(|config| config.s3_options.0.get(&bucket));

            // Fill in missing credentials from config file if not provided on command line
            credentials.region = credentials.region.or(s3_config.map(|c| c.region.clone()));
            credentials.endpoint_url = credentials
                .endpoint_url
                .or(s3_config.map(|c| c.endpoint_url.clone()));

            // Resolve the credentials
            let credentials = match Option::<S3Credentials>::from(credentials) {
                Some(credentials) => {
                    let auth_storage = AuthenticationStorage::from_env_and_defaults()?;
                    credentials.resolve(&channel, &auth_storage).ok_or_else(|| anyhow::anyhow!("Could not find S3 credentials in the authentication storage, and no credentials were provided via the command line."))?
                }
                None => rattler_s3::ResolvedS3Credentials::from_sdk().await?,
            };

            index_s3_with_channel_metadata(
                IndexS3Config {
                    channel,
                    credentials,
                    target_platform: cli.target_platform,
                    repodata_patch: cli.repodata_patch,
                    write_zst,
                    write_shards,
                    repodata_revisions,
                    package_revision_assignment,
                    force: cli.force,
                    max_parallel,
                    multi_progress: Some(multi_progress),
                    precondition_checks,
                },
                channel_metadata,
            )
            .await
        }
    }?;
    println!("Finished indexing channel.");
    Ok(())
}

fn resolve_index_channel_config(config: &Option<Config>, target: &str) -> IndexChannelConfig {
    config
        .as_ref()
        .map(|c| c.index_config.resolve(target))
        .unwrap_or_default()
}

fn effective_index_options(
    cfg: &IndexChannelConfig,
) -> (
    bool,
    bool,
    Vec<rattler_index::RepodataRevisionInfo>,
    PackageRevisionAssignment,
) {
    let write_zst = cfg.write_zst.unwrap_or(true);
    let write_shards = cfg.write_shards.unwrap_or(true);
    let repodata_revisions = cfg.repodata_revisions.clone().unwrap_or_default();
    let package_revision_assignment = cfg.package_revision_assignment.unwrap_or_default();
    (
        write_zst,
        write_shards,
        repodata_revisions,
        package_revision_assignment,
    )
}
