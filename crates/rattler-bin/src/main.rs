use crate::writer::IndicatifWriter;
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
    Auth(commands::auth::Opt),
    Create(commands::create::Opt),
    VirtualPackages(commands::virtual_packages::Opt),
    InstallMenu(commands::menu::InstallOpt),
    RemoveMenu(commands::menu::InstallOpt),
    Upload(Box<rattler_upload::upload::opt::UploadOpts>),
}

/// Entry point of the `rattler` cli.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse the command line arguments
    let opt = Opt::parse();

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
        .with_writer(IndicatifWriter::new(global_multi_progress()))
        .without_time()
        .finish()
        .try_init()?;

    // Dispatch the selected comment
    match opt.command {
        Command::Auth(opts) => commands::auth::auth(opts).await,
        Command::Create(opts) => commands::create::create(opts).await,
        Command::VirtualPackages(opts) => commands::virtual_packages::virtual_packages(opts),
        Command::InstallMenu(opts) => commands::menu::install_menu(opts).await,
        Command::RemoveMenu(opts) => commands::menu::remove_menu(opts).await,
        Command::Upload(opts) => {
            rattler_upload::upload_from_args(*opts).await.unwrap();
            Ok(())
        }
    }
}
