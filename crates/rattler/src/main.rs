use clap::Parser;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

mod commands;

/// Command line options available through the `rattler` cli.
#[derive(Debug, Parser)]
#[clap(author, version, about, long_about = None)]
struct Opt {
    /// The subcommand to execute
    #[clap(subcommand)]
    command: Command,

    /// Log verbose
    #[clap(short, long, global = true)]
    verbose: bool,
}

/// Different commands supported by `rattler`.
#[derive(Debug, clap::Subcommand)]
enum Command {
    Create(commands::create::Opt),
}

/// Entry point of the `rattler` cli.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse the command line arguments
    let opt = Opt::parse();

    // Determine the logging level based on the the verbose flag and the RUST_LOG environment
    // variable.
    let default_filter = opt
        .verbose
        .then_some(LevelFilter::DEBUG)
        .unwrap_or(LevelFilter::INFO);
    let env_filter = EnvFilter::builder()
        .with_default_directive(default_filter.into())
        .from_env()?;

    // Setup the tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .without_time()
        .finish()
        .try_init()?;

    // Dispatch the selected comment
    match opt.command {
        Command::Create(opts) => commands::create::create(opts).await,
    }
}
