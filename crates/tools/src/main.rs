use clap::{Parser, Subcommand};

use tools::Mode::Overwrite;

#[derive(Parser)]
#[clap(name = "tasks", version, author)]
struct Args {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
#[allow(clippy::enum_variant_names)]
enum Commands {
    /// Generate Rust bindings for libsolv
    GenLibsolvBindings,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match args.command {
        Commands::GenLibsolvBindings => tools::libsolv_bindings::generate(Overwrite)?,
    }
    Ok(())
}
