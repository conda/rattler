use clap::{arg, Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use rattler_conda_types::Platform;
use rattler_index::{index_fs, index_s3};
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

    /// Whether to force the re-indexing of all packages.
    /// Note that this will create a new repodata.json instead of updating the existing one.
    #[arg(short, long, default_value = "false", global = true)]
    force: bool,

    /// The maximum number of packages to process in-memory simultaneously.
    /// This is necessary to limit memory usage when indexing large channels.
    #[arg(long, default_value = "32", global = true)]
    max_parallel: usize,

    /// A specific platform to index.
    /// Defaults to all platforms available in the channel.
    #[arg(long, global = true)]
    target_platform: Option<Platform>,

    /// The name of the conda package (expected to be in the `noarch` subdir) that should be used for repodata patching.
    /// For more information, see `https://prefix.dev/blog/repodata_patching`.
    #[arg(long, global = true)]
    repodata_patch: Option<String>,
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
        #[arg(
            long,
            env = "S3_ENDPOINT_URL",
            default_value = "https://s3.amazonaws.com"
        )]
        endpoint_url: Url,

        /// The region of the S3 backend
        #[arg(long, env = "S3_REGION", default_value = "eu-central-1")]
        region: String,

        /// Whether to use path-style S3 URLs
        #[arg(long, env = "S3_FORCE_PATH_STYLE", default_value = "false")]
        force_path_style: bool,

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

/// Entry point of the `rattler-index` cli.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse the command line arguments
    let cli = Cli::parse();

    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(cli.verbosity)
        .init();

    let multi_progress = indicatif::MultiProgress::new();

    match cli.command {
        Commands::FileSystem { channel } => {
            index_fs(
                channel,
                cli.target_platform,
                cli.repodata_patch,
                cli.force,
                cli.max_parallel,
                Some(multi_progress),
            )
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
            index_s3(
                channel,
                region,
                endpoint_url,
                force_path_style,
                access_key_id,
                secret_access_key,
                session_token,
                cli.target_platform,
                cli.repodata_patch,
                cli.force,
                cli.max_parallel,
                Some(multi_progress),
            )
            .await
        }
    }?;
    println!("Finished indexing channel.");
    Ok(())
}
