use crate::commands::DoctorCommand;
use clap::Parser;
use indicatif::{MultiProgress, ProgressDrawTarget};
use once_cell::sync::Lazy;
use tracing_subscriber::{filter::LevelFilter, util::SubscriberInitExt, EnvFilter};

mod commands;
mod writer;

/// Returns a global instance of [`indicatif::MultiProgress`].
///
/// Although you can always create an instance yourself any logging will interrupt pending
/// progressbars. To fix this issue, logging has been configured in such a way to it will not
/// interfere if you use the [`indicatif::MultiProgress`] returning by this function.
pub fn global_multi_progress() -> MultiProgress {
    static GLOBAL_MP: Lazy<MultiProgress> = Lazy::new(|| {
        let mp = MultiProgress::new();
        mp.set_draw_target(ProgressDrawTarget::stderr_with_hz(20));
        mp
    });
    GLOBAL_MP.clone()
}

/// Rattler CLI
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Check your rattler installation and environment for common issues
    Doctor(DoctorCommand),
}

/// Entry point of the `rattler` cli.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse the command line arguments
    let cli = Cli::parse();

    // Setup default logging level
    let default_filter = LevelFilter::INFO;
    
    let env_filter = EnvFilter::builder()
        .with_default_directive(default_filter.into())
        .from_env()?
        // filter logs from apple codesign because they are very noisy
        .add_directive("apple_codesign=off".parse()?);

    // Setup the tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(writer::IndicatifWriter::new(global_multi_progress()))
        .without_time()
        .finish()
        .try_init()?;

    // Dispatch the selected command
    match cli.command {
        Commands::Doctor(cmd) => cmd.run().await,
    }
}
