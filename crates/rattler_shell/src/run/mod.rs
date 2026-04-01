//! Helpers to run commands in an activated environment.

use rattler_conda_types::Platform;
use std::process::{Command, ExitStatus, Output};
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

/// Run a subprocess in an activated environment (inherited stdio, `command` non-empty).
/// Uses the full process environment for activation; [`run_in_environment`] seeds from `env_vars` only.
pub async fn run_command_in_environment(
    prefix: &Path,
    command: &[String],
    shell: ShellEnum,
    env_vars: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> Result<ExitStatus, RunError> {
    let activator = Activator::from_path(prefix, shell, Platform::current())?;

    let current_path = std::env::var("PATH")
        .ok()
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>());
    let conda_prefix = std::env::var("CONDA_PREFIX").ok().map(Into::into);

    let activation_vars = ActivationVariables {
        conda_prefix,
        path: current_path,
        path_modification_behavior: PathModificationBehavior::default(),
        // Full process environment. `run_in_environment` uses only the caller `env_vars` map.
        current_env: std::env::vars().collect(),
    };

    let activated_env =
        tokio::task::spawn_blocking(move || activator.run_activation(activation_vars, None))
            .await
            .expect("Activated environment panicked")?;

    let cmd = &command[0];
    let args = &command[1..];
    let mut child_cmd = tokio::process::Command::new(cmd);
    child_cmd.args(args);
    child_cmd.envs(&activated_env);
    child_cmd.envs(env_vars);
    if let Some(cwd) = cwd {
        child_cmd.current_dir(cwd);
    }

    Ok(child_cmd.status().await?)
}

/// Execute a script in an activated environment.
pub fn run_in_environment(
    prefix: &Path,
    script: &Path,
    shell: ShellEnum,
    env_vars: &HashMap<String, String>,
) -> Result<Output, RunError> {
    let mut shell_script = shell::ShellScript::new(shell.clone(), Platform::current());

    for (k, v) in env_vars.iter() {
        shell_script
            .set_env_var(k, v)
            .map_err(ActivationError::from)?;
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
        current_env: env_vars.clone(),
    };

    let host_activation = activator.activation(activation_vars)?;

    shell_script.append_script(&host_activation.script);

    shell_script
        .run_script(script)
        .map_err(ActivationError::from)?;
    let file = tempfile::Builder::new()
        .suffix(&format!(".{}", shell.extension()))
        .tempfile()?;
    fs_err::write(
        file.path(),
        shell_script.contents().map_err(ActivationError::from)?,
    )?;

    match shell {
        ShellEnum::Bash(_) => Ok(Command::new(shell.executable()).arg(file.path()).output()?),
        ShellEnum::CmdExe(_) => Ok(Command::new(shell.executable())
            .arg("/c")
            .arg(file.path())
            .output()?),
        _ => unimplemented!("Unsupported shell: {:?}", shell),
    }
}
