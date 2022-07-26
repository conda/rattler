use clap::Parser;

mod commands;

/// Command line options available through the `rattler` cli.
#[derive(Debug, Parser)]
#[clap(author, version, about, long_about = None)]
struct Opt {
    #[clap(subcommand)]
    command: Command,
}

/// Different commands supported by `rattler`.
#[derive(Debug, clap::Subcommand)]
enum Command {
    Create(commands::create::Opt),
}

/// Entry point of the `rattler` cli.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    let opt = Opt::parse();
    match opt.command {
        Command::Create(opt) => commands::create::create(opt).await,
    }
}
