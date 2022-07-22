use structopt::StructOpt;

mod commands;
mod libsolv;

/// Command line options available through the `rattler` cli.
#[derive(Debug, StructOpt)]
struct Opt {
    #[structopt(subcommand)]
    command: Command,
}

/// Different commands supported by `rattler`.
#[derive(Debug, StructOpt)]
enum Command {
    Create(commands::create::Opt),
}

/// Entry point of the `rattler` cli.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    let opt = Opt::from_args();
    match opt.command {
        Command::Create(opt) => commands::create::create(opt).await,
    }
}
