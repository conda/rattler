use clap::Parser;
use rattler_index_s3::{rattler_index_s3, IndexS3Config};
use tracing_subscriber::{filter::LevelFilter, util::SubscriberInitExt, EnvFilter};
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

/// Command line options available through the `rattler-index-s3` cli.
#[derive(Debug, Parser)]
#[clap(author, version, about, long_about = None, after_help = "Examples:
    $ rattler-index-s3 s3://my-bucket/my-channel
    $ AWS_CONFIG_FILE=aws.config rattler-index-s3 s3://my-bucket/my-channel
    $ rattler-index-s3 --endpoint-url my-custom-aws.com --region us-east-1 --force-path-style s3://my-bucket/my-channel")]
struct Args {
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

    /// Log verbose
    #[clap(short, long, global = true)]
    verbose: bool,
}

/// Entry point of the `rattler-index-s3` cli.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse the command line arguments
    let opt = Args::parse();

    // Determine the logging level based on the the verbose flag and the RUST_LOG environment
    // variable.
    let default_filter = if opt.verbose {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    let env_filter = EnvFilter::builder()
        .with_default_directive(default_filter.into())
        .from_env()?
        // filter logs from apple codesign because they are very noisy
        .add_directive("apple_codesign=off".parse()?);

    // Setup the tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .without_time()
        .finish()
        .try_init()?;

    rattler_index_s3(IndexS3Config {
        channel: opt.channel,
        endpoint_url: opt.endpoint_url,
        region: opt.region,
        force_path_style: opt.force_path_style,
    })
    .await
}
