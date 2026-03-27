use std::{collections::HashMap, env, path::PathBuf, str::FromStr};

use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::ShellEnum,
};

/// Print the shell activation hook for a conda prefix to stdout.
#[derive(Debug, Parser)]
pub struct Opt {
    /// Target prefix to generate the shell hook for
    #[clap(
        short = 'p',
        long = "prefix",
        visible_alias = "target-prefix",
        default_value = ".prefix"
    )]
    target_prefix: PathBuf,

    /// Shell to generate the hook for (bash, zsh, fish, xonsh, cmd, nushell, powershell)
    #[clap(short, long)]
    shell: Option<String>,
}

fn activation_variables_from_env() -> ActivationVariables {
    let current_env: HashMap<String, String> = env::vars().collect();

    ActivationVariables {
        conda_prefix: current_env.get("CONDA_PREFIX").map(PathBuf::from),
        path: env::var_os("PATH").map(|path| env::split_paths(&path).collect()),
        path_modification_behavior: PathModificationBehavior::Replace,
        current_env,
    }
}

fn determine_shell(shell: Option<&str>) -> miette::Result<ShellEnum> {
    shell
        .map(ShellEnum::from_str)
        .transpose()
        .into_diagnostic()?
        .or_else(ShellEnum::from_env)
        .map_or_else(|| Ok(ShellEnum::default()), Ok)
}

pub async fn shell_hook(opt: Opt) -> miette::Result<()> {
    let shell = determine_shell(opt.shell.as_deref())?;
    let target_prefix = std::path::absolute(opt.target_prefix).into_diagnostic()?;
    let activator =
        Activator::from_path(&target_prefix, shell, Platform::current()).into_diagnostic()?;
    let activation = activator
        .activation(activation_variables_from_env())
        .into_diagnostic()?;

    print!("{}", activation.script.contents().into_diagnostic()?);
    Ok(())
}
