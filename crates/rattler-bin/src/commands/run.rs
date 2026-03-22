use miette::IntoDiagnostic;
use rattler_shell;
use std::{env, path::PathBuf};

/// Run a command in an activated conda environment.
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

    let shell = rattler_shell::shell::ShellEnum::from_env().unwrap_or_default();
    let cwd = opt.cwd.as_deref();
    let env_vars = std::collections::HashMap::new();

    let status = rattler_shell::run_command_in_environment(
        &target_prefix,
        &opt.command,
        shell,
        &env_vars,
        cwd,
    )
    .await
    .into_diagnostic()?;
    let code = status.code().unwrap_or(1);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
