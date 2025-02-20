use clap::{arg, Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use opendal::services::{FsConfig, S3Config};
use rattler_conda_types::Platform;
use rattler_index::index;
use tracing_log::AsTrace;
use url::Url;

fn parse_s3_url(value: &str) -> Result<Url, String> {
    let url: Url = Url::parse(value).map_err(|_| format!("`{}` isn't a valid URL", value))?;
    if url.scheme() == "s3" && url.host_str().is_some() {
        Ok(url)
    } else {
        Err(format!(
            "Only S3 URLs of format s3://bucket/... can be used, not `{}`",
            value
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
    #[arg(short, long, default_value = "false")]
    force: bool,
}

/// The subcommands for the pixi-pack CLI.
#[derive(Subcommand)]
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
            short,
            long,
            env = "S3_ENDPOINT_URL",
            default_value = "https://s3.amazonaws.com"
        )]
        endpoint_url: Url,

        /// The region of the S3 backend
        #[arg(short, long, env = "S3_REGION", default_value = "eu-central-1")]
        region: String,

        /// Whether to use path-style S3 URLs
        #[arg(short, long, env = "S3_FORCE_PATH_STYLE", default_value = "false")]
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
            index(None, fs_config, cli.force).await?;
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
            if let (Some(access_key_id), Some(secret_access_key)) =
                (access_key_id, secret_access_key)
            {
                s3_config.secret_access_key = Some(secret_access_key);
                s3_config.access_key_id = Some(access_key_id);
                s3_config.session_token = session_token;
            }
            // TODO: Read from rattler auth store
            index(None, s3_config, cli.force).await?;
            Ok(())
        }
    }
}
