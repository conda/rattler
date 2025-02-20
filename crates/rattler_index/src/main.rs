use clap::{arg, Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use opendal::services::{FsConfig, S3Config};
use rattler_conda_types::Platform;
use rattler_index::index;
use rattler_networking::{Authentication, AuthenticationStorage};
use tracing_log::AsTrace;
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
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[command(flatten)]
    verbose: Verbosity,

    /// Whether to force the re-indexing of all packages.
    /// Note that this will create a new repodata.json instead of updating the existing one.
    #[arg(short, long, default_value = "false", global = true)]
    force: bool,

    /// The maximum number of packages to process in-memory simultaneously.
    /// This is necessary to limit memory usage when indexing large channels.
    #[arg(long, default_value = "128", global = true)]
    max_parallel: usize,

    /// A specific platform to index.
    /// Defaults to all platforms available in the channel.
    #[arg(long, global = true)]
    target_platform: Option<Platform>,
}

/// The subcommands for the pixi-pack CLI.
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
        .with_max_level(cli.verbose.log_level_filter().as_trace())
        .init();

    match cli.command {
        Commands::FileSystem { channel } => {
            let channel = &channel.canonicalize()?.to_string_lossy().to_string();
            let mut fs_config = FsConfig::default();
            fs_config.root = Some(channel.clone());
            tracing::info!("Indexing channel at {}", channel);
            index(cli.target_platform, fs_config, cli.force, cli.max_parallel).await?;
            Ok(())
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
            let mut s3_config = S3Config::default();
            s3_config.root = Some(channel.path().to_string());
            s3_config.bucket = channel
                .host_str()
                .ok_or(anyhow::anyhow!("No bucket in S3 URL"))?
                .to_string();
            s3_config.region = Some(region);
            s3_config.endpoint = Some(endpoint_url.to_string());
            s3_config.enable_virtual_host_style = !force_path_style;
            // Use credentials from the CLI if they are provided.
            if let (Some(access_key_id), Some(secret_access_key)) =
                (access_key_id, secret_access_key)
            {
                s3_config.secret_access_key = Some(secret_access_key);
                s3_config.access_key_id = Some(access_key_id);
                s3_config.session_token = session_token;
            } else {
                // If they're not provided, check rattler authentication storage for credentials.
                let auth_storage = AuthenticationStorage::from_env_and_defaults()?;
                let auth = auth_storage.get_by_url(channel)?;
                if let (
                    _,
                    Some(Authentication::S3Credentials {
                        access_key_id,
                        secret_access_key,
                        session_token,
                    }),
                ) = auth
                {
                    s3_config.access_key_id = Some(access_key_id);
                    s3_config.secret_access_key = Some(secret_access_key);
                    s3_config.session_token = session_token;
                }
            }
            index(cli.target_platform, s3_config, cli.force, cli.max_parallel).await?;
            Ok(())
        }
    }
}
