use std::{env, path::PathBuf};
use rattler_shell;
use miette::IntoDiagnostic;

/// Run a command in a workspace
#[derive(Debug, clap::Parser)]
pub struct Opt {
    #[clap(long)]
    target_prefix: Option<PathBuf>,

    #[clap(long)]
    cwd: Option<PathBuf>,

    #[clap(required = true, last = true)]
    command: Vec<String>,
}

pub async fn run(opt: Opt) -> miette::Result<()> {
    let current_dir = env::current_dir().into_diagnostic()?;
    let target_prefix = opt
        .target_prefix
        .unwrap_or_else(|| current_dir.join(".prefix"));

    // Make the target prefix absolute
    let target_prefix = std::path::absolute(target_prefix).into_diagnostic()?;
    println!("Target prefix: {}", target_prefix.display());
    let cwd = opt.cwd.unwrap_or_else(|| current_dir);

    rattler_shell::run_command_in_environment(target_prefix, command, shell, env_vars, cwd);
    Ok(())
}