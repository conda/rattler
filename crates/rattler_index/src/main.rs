use clap::{arg, Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use opendal::services::FsConfig;
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
}

/// The subcommands for the pixi-pack CLI.
#[derive(Subcommand)]
enum Commands {
    /// Index a channel stored on the filesystem.
    FileSystem {
        /// The path to the channel directory.
        #[arg(long)]
        channel: std::path::PathBuf,
    },

    /// Index a channel stored in an S3 bucket.
    S3 {
        /// The S3 channel URL, e.g. `s3://my-bucket/my-channel`.
        #[arg(value_parser = parse_s3_url)]
        channel: Url,

        /// The endpoint URL of the S3 backend
        #[arg(short, long, env = "S3_ENDPOINT_URL", requires_all = ["region", "force_path_style"])]
        endpoint_url: Option<Url>,

        /// The region of the S3 backend
        #[arg(short, long, env = "S3_REGION", requires_all = ["endpoint_url", "force_path_style"])]
        region: Option<String>,

        /// Whether to use path-style S3 URLs
        #[arg(short, long, env = "S3_FORCE_PATH_STYLE", requires_all = ["endpoint_url", "region"])]
        force_path_style: Option<bool>,
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
            let channel = &channel.to_string_lossy().to_string();
            let mut fs_config = FsConfig::default();
            fs_config.root = Some(channel.clone());
            index(&channel, None, fs_config).await?;
            Ok(())
        }
        Commands::S3 {
            channel,
            region,
            endpoint_url,
            force_path_style,
        } => {
            todo!();
            Ok(())
        }
    }
}
