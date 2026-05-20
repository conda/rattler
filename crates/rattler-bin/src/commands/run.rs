use miette::IntoDiagnostic;
use rattler_shell;
use std::{env, path::PathBuf};

/// Run a command in an activated conda environment.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Target prefix (environment path) to run the command in.
    /// Defaults to the active conda environment ($`CONDA_PREFIX`), then `.prefix` in the current directory.
    #[clap(short = 'p', long = "prefix", visible_alias = "target-prefix")]
    target_prefix: Option<PathBuf>,

    /// Working directory for the child process
    #[clap(long)]
    cwd: Option<PathBuf>,

    /// Program and arguments to run
    #[clap(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

pub async fn run(opt: Opt) -> miette::Result<()> {
    let prefix = if let Some(prefix) = opt.target_prefix {
        prefix
    } else if let Ok(prefix) = env::var("CONDA_PREFIX") {
        PathBuf::from(prefix)
    } else {
        env::current_dir().into_diagnostic()?.join(".prefix")
    };
    // Make the target prefix absolute
    let target_prefix = std::path::absolute(prefix).into_diagnostic()?;

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
