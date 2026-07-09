use clap::Parser;
use indicatif::{MultiProgress, ProgressDrawTarget};
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, util::SubscriberInitExt};

use crate::{commands::exec, writer::IndicatifWriter};

mod commands;
mod exclude_newer;
mod writer;

/// Returns a global instance of [`indicatif::MultiProgress`].
///
/// Although you can always create an instance yourself any logging will
/// interrupt pending progress bars. To fix this issue, logging has been
/// configured in such a way to it will not interfere if you use the
/// [`indicatif::MultiProgress`] returning by this function.
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

    /// Disable network access and use only locally cached data.
    #[clap(long, global = true)]
    offline: bool,
}

/// Different commands supported by `rattler`.
#[derive(Debug, clap::Subcommand)]
enum Command {
    Auth(Box<commands::auth::Opt>),
    Completion(commands::completion::Opt),
    Create(commands::create::Opt),
    Download(commands::download::Opt),
    FetchFile(commands::fetch_file::Opt),
    Inspect(commands::inspect::Opt),
    Search(commands::search::Opt),
    Solve(commands::solve::Opt),
    ShellHook(commands::shell_hook::Opt),
    VirtualPackages(commands::virtual_packages::Opt),
    InstallMenu(commands::menu::InstallOpt),
    RemoveMenu(commands::menu::RemoveOpt),
    Run(commands::run::Opt),
    Extract(commands::extract::Opt),
    Link(commands::link::Opt),
    InjectIntoPrefix(commands::prefix::InjectOpt),
    RemoveFromPrefix(commands::prefix::RemoveFromPrefixOpt),
    #[cfg(feature = "mount")]
    Mount(commands::mount::Opt),
    Upload(Box<rattler_upload::upload::opt::UploadOpts>),
    List(commands::list::Opt),
    Exec(commands::exec::Opt),
}

/// Entry point of the `rattler` cli.
fn main() -> miette::Result<()> {
    let num_cores = std::thread::available_parallelism()
        .map_or(2, std::num::NonZero::get)
        .max(2);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(num_cores / 2)
        .max_blocking_threads(num_cores)
        .enable_all()
        .build()
        .into_diagnostic()?;

    runtime.block_on(async_main())
}

async fn async_main() -> miette::Result<()> {
    // Parse the command line arguments
    let opt = Opt::parse();

    // Determine the logging level based on the the verbose flag and the RUST_LOG
    // environment variable.
    let default_filter = if opt.verbose {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    let env_filter = EnvFilter::builder()
        .with_default_directive(default_filter.into())
        .from_env()
        .into_diagnostic()?
        // filter logs from apple codesign because they are very noisy
        .add_directive("apple_codesign=off".parse().into_diagnostic()?);

    // Setup the tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(IndicatifWriter::new(global_multi_progress()))
        .without_time()
        .finish()
        .try_init()
        .into_diagnostic()?;

    let offline = opt.offline;

    // Dispatch the selected comment
    match opt.command {
        Command::Auth(opts) => commands::auth::auth(*opts, offline).await,
        Command::Completion(opts) => commands::completion::completion(opts),
        Command::Create(opts) => commands::create::create(opts, offline).await,
        Command::Download(opts) => commands::download::download(opts, offline).await,
        Command::FetchFile(opts) => commands::fetch_file::fetch_file(opts, offline).await,
        Command::Inspect(opts) => commands::inspect::inspect(opts, offline).await,
        Command::Search(opts) => commands::search::search(opts, offline).await,
        Command::Solve(opts) => commands::solve::solve(opts, offline).await,
        Command::List(opts) => commands::list::list(opts).await,
        Command::ShellHook(opts) => commands::shell_hook::shell_hook(opts).await,
        Command::VirtualPackages(opts) => commands::virtual_packages::virtual_packages(opts),
        Command::InstallMenu(opts) => commands::menu::install_menu(opts).await,
        Command::RemoveMenu(opts) => commands::menu::remove_menu(opts).await,
        Command::Run(opts) => commands::run::run(opts).await,
        Command::Extract(opts) => commands::extract::extract(opts, offline).await,
        Command::Link(opts) => commands::link::link(opts).await,
        Command::InjectIntoPrefix(opts) => commands::prefix::inject(opts).await,
        Command::RemoveFromPrefix(opts) => commands::prefix::remove_from_prefix(opts).await,
        #[cfg(feature = "mount")]
        Command::Mount(opts) => commands::mount::mount(opts).await,
        Command::Upload(opts) => {
            if offline {
                return Err(miette::miette!(
                    "upload cannot run because network access is disabled by --offline"
                ));
            }
            rattler_upload::upload_from_args(*opts).await
        }
        Command::Exec(opts) => exec::exec(opts, offline).await,
    }
}
