use structopt::StructOpt;

mod commands;
pub(crate) mod conda;
mod solver;

#[derive(Debug, StructOpt)]
struct Opt {
    #[structopt(subcommand)]
    command: Command,
}

#[derive(Debug, StructOpt)]
enum Command {
    Create(commands::create::Opt),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opt = Opt::from_args();
    match opt.command {
        Command::Create(opt) => commands::create::create(opt).await,
    }
}
