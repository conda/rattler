use std::path::PathBuf;

use anyhow::Context;
use clap::{arg, Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use rattler_conda_types::Platform;
use rattler_config::config::concurrency::default_max_concurrent_solves;
use rattler_index::{index_fs, index_s3, IndexFsConfig, IndexS3Config};
use url::Url;

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

    /// Whether to write repodata.json.zst.
    #[arg(long, default_value = "true", global = true)]
    write_zst: Option<bool>,

    /// Whether to write sharded repodata.
    #[arg(long, default_value = "true", global = true)]
    write_shards: Option<bool>,

    /// Whether to force the re-indexing of all packages.
    /// Note that this will create a new repodata.json instead of updating the existing one.
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

    /// The name of the conda package (expected to be in the `noarch` subdir) that should be used for repodata patching.
    /// For more information, see `https://prefix.dev/blog/repodata_patching`.
    #[arg(long, global = true)]
    repodata_patch: Option<String>,

    /// The path to the config file to use to configure rattler-index.
    /// Uses the same configuration format as pixi, see `https://pixi.sh/latest/reference/pixi_configuration`.
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
    S3 {
        /// The S3 channel URL, e.g. `s3://my-bucket/my-channel`.
        #[arg(value_parser = parse_s3_url)]
        channel: Url,

        /// The endpoint URL of the S3 backend
        #[arg(long, env = "S3_ENDPOINT_URL")]
        endpoint_url: Option<Url>,

        /// The region of the S3 backend
        #[arg(long, env = "S3_REGION")]
        region: Option<String>,

        /// Whether to use path-style S3 URLs
        #[arg(long, env = "S3_FORCE_PATH_STYLE")]
        force_path_style: Option<bool>,

        /// The access key ID for the S3 bucket.
        #[arg(long, env = "S3_ACCESS_KEY_ID", requires_all = ["secret_access_key"])]
        access_key_id: Option<String>,

        /// The secret access key for the S3 bucket.
        #[arg(long, env = "S3_SECRET_ACCESS_KEY", requires_all = ["access_key_id"])]
        secret_access_key: Option<String>,

        /// The session token for the S3 bucket.
        #[arg(long, env = "S3_SESSION_TOKEN", requires_all = ["access_key_id", "secret_access_key"])]
        session_token: Option<String>,
    },
}

/// The configuration type for rattler-index - just extends rattler config and can load the same TOML files as pixi.
pub type Config = rattler_config::config::ConfigBase<()>;

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

    match cli.command {
        Commands::FileSystem { channel } => {
            index_fs(IndexFsConfig {
                channel,
                target_platform: cli.target_platform,
                repodata_patch: cli.repodata_patch,
                write_zst: cli.write_zst.unwrap_or(true),
                write_shards: cli.write_shards.unwrap_or(true),
                force: cli.force,
                max_parallel,
                multi_progress: Some(multi_progress),
            })
            .await
        }
        Commands::S3 {
            channel,
            region,
            endpoint_url,
            force_path_style,
            access_key_id,
            secret_access_key,
            session_token,
        } => {
            let bucket = channel.host().context("Invalid S3 url")?.to_string();
            let s3_config = config
                .as_ref()
                .and_then(|config| config.s3_options.0.get(&bucket));
            let region = region
                .or(s3_config.map(|c| c.region.clone()))
                .context("S3 region not provided")?;
            let endpoint_url = endpoint_url
                .or(s3_config.map(|c| c.endpoint_url.clone()))
                .context("S3 endpoint url not provided")?;
            let force_path_style = force_path_style
                .or(s3_config.map(|c| c.force_path_style))
                .context("S3 force-path-style not provided")?;

            index_s3(IndexS3Config {
                channel,
                region,
                endpoint_url,
                force_path_style,
                access_key_id,
                secret_access_key,
                session_token,
                target_platform: cli.target_platform,
                repodata_patch: cli.repodata_patch,
                write_zst: cli.write_zst.unwrap_or(true),
                write_shards: cli.write_shards.unwrap_or(true),
                force: cli.force,
                max_parallel,
                multi_progress: Some(multi_progress),
            })
            .await
        }
    }?;
    println!("Finished indexing channel.");
    Ok(())
}
