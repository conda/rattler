//! Helpers to run commands in an activated environment.

use rattler_conda_types::Platform;
use std::fmt::Write;
use std::process::{Command, Output};
use std::{collections::HashMap, path::Path};

use crate::activation::{ActivationError, PathModificationBehavior};
use crate::shell::ShellEnum;
use crate::{
    activation::{ActivationVariables, Activator},
    shell::{self, Shell},
};

#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("Error while activating the environment: {0}")]
    ActivationError(#[from] ActivationError),

    #[error("Error while writing the script: {0}")]
    WriteError(#[from] std::fmt::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// blast the environment with the given command
pub fn run_in_environment(
    prefix: &Path,
    args: &[&str],
    shell: ShellEnum,
    env_vars: &HashMap<String, String>,
) -> Result<Output, RunError> {
    let mut shell_script = shell::ShellScript::new(shell.clone(), Platform::current());

    for (k, v) in env_vars.iter() {
        shell_script.set_env_var(k, v);
    }

    let activator = Activator::from_path(prefix, shell.clone(), Platform::current())?;

    let current_path = std::env::var("PATH")
        .ok()
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>());
    let conda_prefix = std::env::var("CONDA_PREFIX").ok().map(Into::into);

    let activation_vars = ActivationVariables {
        conda_prefix,
        path: current_path,
        path_modification_behavior: PathModificationBehavior::default(),
    };

    let host_activation = activator.activation(activation_vars)?;

    writeln!(shell_script.contents, "{}", host_activation.script)?;

    match shell {
        ShellEnum::Bash(_) => {
            writeln!(shell_script.contents, ". {}", args.join(" "))?;
        }
        ShellEnum::CmdExe(_) => {
            writeln!(shell_script.contents, "@call {}", args.join(" "))?;
        }
        _ => unimplemented!("Unsupported shell: {:?}", shell),
    }

    let tempfile = tempfile::NamedTempFile::new()?;
    std::fs::write(tempfile.path(), shell_script.contents)?;

    Ok(Command::new(shell.executable())
        .arg(tempfile.path())
        .output()?)
}
