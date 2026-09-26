use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use rattler_conda_types::Platform;
use rattler_config::config::{
    concurrency::default_max_concurrent_solves, index::IndexChannelConfig,
};
use rattler_index::{
    ChannelMetadata, IndexFsConfig, IndexProcessingOptions, IndexStats, PackageRevisionAssignment,
    index_fs_with_channel_metadata,
};
#[cfg(feature = "s3")]
use rattler_index::{IndexS3Config, PreconditionChecks, index_s3_with_channel_metadata};
#[cfg(feature = "s3")]
use rattler_networking::AuthenticationStorage;
#[cfg(feature = "s3")]
use rattler_s3::S3Credentials;
use tokio_util::sync::CancellationToken;
#[cfg(feature = "s3")]
use url::Url;

/// Exit code used when the process was interrupted with `SIGINT`.
const EXIT_INTERRUPTED: i32 = 130;

/// Parses a byte size with an optional `K`, `M`, `G` or `T` suffix (powers of
/// 1024, an optional `i` and `B` are accepted, e.g. `2GiB`, `512M`, `1048576`).
fn parse_byte_size(value: &str) -> Result<u64, String> {
    let value = value.trim();
    let digits_end = value
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(digits_end);
    let number: f64 = number
        .parse()
        .map_err(|err| format!("`{value}` is not a valid size: {err}"))?;
    let suffix = suffix
        .trim()
        .trim_end_matches(['b', 'B'])
        .trim_end_matches(['i', 'I'])
        .to_ascii_uppercase();
    let multiplier: f64 = match suffix.as_str() {
        "" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        "T" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return Err(format!("`{value}` has an unknown size suffix")),
    };
    let bytes = number * multiplier;
    if !(bytes >= 1.0 && bytes.is_finite()) {
        return Err(format!("`{value}` must be at least one byte"));
    }
    Ok(bytes as u64)
}

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

    /// The maximum number of package bytes to hold in memory simultaneously.
    /// Accepts a suffix like `512M` or `4GiB`. Together with `--max-parallel`
    /// this bounds the memory used for packages in flight.
    #[arg(long, global = true, default_value = "2GiB", value_parser = parse_byte_size)]
    max_in_flight_bytes: u64,

    /// Directory in which parsed package metadata is cached between runs.
    /// A package is only downloaded and parsed again when it changed, and an
    /// interrupted run resumes from what was already cached.
    #[arg(long, global = true, env = "RATTLER_INDEX_CACHE_DIR")]
    cache_dir: Option<PathBuf>,

    /// When interrupted with Ctrl-C, still write repodata for the packages
    /// indexed so far instead of leaving the existing repodata untouched.
    /// The next run adds the remaining packages.
    #[arg(long, global = true, default_value = "false")]
    publish_partial: bool,

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

    let cancellation_token = CancellationToken::new();
    spawn_ctrl_c_handler(cancellation_token.clone());
    let processing = IndexProcessingOptions {
        cache_dir: cli.cache_dir,
        max_in_flight_bytes: Some(cli.max_in_flight_bytes),
        cancellation_token: Some(cancellation_token.clone()),
        publish_partial: cli.publish_partial,
    };

    let stats = match cli.command {
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
                    processing,
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
                    processing,
                },
                channel_metadata,
            )
            .await
        }
    }?;

    report(&stats);
    if stats.cancelled {
        std::process::exit(EXIT_INTERRUPTED);
    }
    if stats.has_failures() {
        std::process::exit(1);
    }
    Ok(())
}

/// Cancels `token` on the first `SIGINT` and aborts the process on the second.
fn spawn_ctrl_c_handler(token: CancellationToken) {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        eprintln!(
            "\nInterrupted. Finishing the packages in flight and saving progress; press Ctrl-C again to abort."
        );
        token.cancel();
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("Aborted.");
            std::process::exit(EXIT_INTERRUPTED);
        }
    });
}

/// Prints a summary of the indexing run.
fn report(stats: &IndexStats) {
    let added: usize = stats.subdirs.values().map(|s| s.packages_added).sum();
    let removed: usize = stats.subdirs.values().map(|s| s.packages_removed).sum();
    let skipped: usize = stats.subdirs.values().map(|s| s.packages_skipped).sum();
    let failed = stats.failed_packages().count();

    if failed > 0 {
        eprintln!("{failed} packages could not be indexed and were left out of the repodata:");
        let mut failures = stats.failed_packages().collect::<Vec<_>>();
        failures.sort_by(|a, b| (a.0.as_str(), &a.1.filename).cmp(&(b.0.as_str(), &b.1.filename)));
        for (subdir, failure) in failures {
            eprintln!("  {subdir}/{}: {}", failure.filename, failure.error);
        }
    }

    if stats.cancelled {
        let not_written = stats
            .subdirs
            .iter()
            .filter(|(_, s)| s.cancelled && !s.repodata_written)
            .map(|(subdir, _)| subdir.as_str())
            .collect::<Vec<_>>();
        println!(
            "Interrupted after indexing {added} packages ({skipped} not attempted). Re-run the same command to continue."
        );
        if !not_written.is_empty() {
            println!(
                "Repodata was not updated for: {} (use --publish-partial to publish partial results).",
                not_written.join(", ")
            );
        }
    } else {
        println!(
            "Finished indexing channel: {added} packages added, {removed} removed, {failed} failed."
        );
    }
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
    Vec<rattler_index::RepodataRevisionSelection>,
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
