use miette::IntoDiagnostic;
use rattler_shell;
use std::{env, path::PathBuf};

/// Run a command in an activated conda environment.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Environment prefix to activate (defaults to `.prefix` under the current directory)
    #[clap(long)]
    target_prefix: Option<PathBuf>,

    /// Working directory for the child process
    #[clap(long)]
    cwd: Option<PathBuf>,

    /// Program and arguments to run
    #[clap(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

pub async fn run(opt: Opt) -> miette::Result<()> {
    let current_dir = env::current_dir().into_diagnostic()?;
    let target_prefix = opt
        .target_prefix
        .unwrap_or_else(|| current_dir.join(".prefix"));
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
    .map_err(|e| miette::miette!("{e}"))?;

    let code = status.code().unwrap_or(1);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
