//! The environment variables of an activated detector environment.
//!
//! Activation evaluates the prefix's activation scripts, including those of
//! the detector's dependencies, the same way a normal activation would, and
//! prepends the prefix's `PATH` directories in CEP 32 order to the inherited
//! `PATH`. It runs in the platform's default shell under the same time and
//! output bounds as the detector itself, and its process tree is terminated
//! when a bound is reached.

use std::{collections::HashMap, path::Path, time::Duration};

use rattler_conda_types::Subdir;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::{Shell, ShellEnum, ShellScript},
};
use thiserror::Error;

use crate::{
    limits::OUTPUT_LIMIT,
    process::{ProcessError, ProcessLimits, run_bounded},
};

const ENV_SEPARATOR: &str = "____RATTLER_DETECTOR_ENV____";

/// Why activation failed.
#[derive(Debug, Error)]
pub enum ActivationError {
    /// The activation script could not be assembled.
    #[error("failed to build the activation script")]
    Script(#[source] rattler_shell::activation::ActivationError),

    /// The environment dumps could not be added to the activation script.
    #[error("failed to assemble the activation script")]
    Assemble(#[from] std::fmt::Error),

    /// The activation script could not be rendered.
    #[error("failed to render the activation script")]
    Render(#[source] rattler_shell::shell::ShellError),

    /// The activation script could not be written.
    #[error("failed to write the activation script")]
    Io(#[from] std::io::Error),

    /// The shell could not be started or read.
    #[error("failed to run the activation script")]
    Process(#[source] ProcessError),

    /// The shell did not finish within the timeout.
    #[error("activation did not finish within {timeout:?}")]
    TimedOut {
        /// The timeout that was reached.
        timeout: Duration,
        /// What the shell wrote to standard error before termination.
        stderr: String,
    },

    /// The shell reached the combined output limit.
    #[error("activation reached the {limit}-byte output limit and was terminated")]
    OutputLimitExceeded {
        /// The limit that was exceeded.
        limit: usize,
        /// What the shell wrote to standard error before termination.
        stderr: String,
    },

    /// The shell exited unsuccessfully.
    #[error("activation exited with {status}")]
    Failed {
        /// The shell's exit status.
        status: std::process::ExitStatus,
        /// What the shell wrote to standard error.
        stderr: String,
    },

    /// The shell's output did not contain the environment dumps.
    #[error("activation produced no environment listing")]
    NoEnvironment,
}

impl ActivationError {
    /// What the shell wrote to standard error, where it ran at all.
    pub fn stderr(&self) -> Option<String> {
        match self {
            Self::TimedOut { stderr, .. }
            | Self::OutputLimitExceeded { stderr, .. }
            | Self::Failed { stderr, .. } => Some(stderr.clone()),
            Self::Process(error) => error
                .stderr()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
            Self::Script(_)
            | Self::Assemble(_)
            | Self::Render(_)
            | Self::Io(_)
            | Self::NoEnvironment => None,
        }
    }
}

/// This process's environment, with values that are not valid Unicode decoded
/// lossily rather than aborting the process.
pub fn current_environment() -> HashMap<String, String> {
    std::env::vars_os()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

/// Computes the environment of `prefix` after activation on `platform`,
/// starting from this process's environment.
///
/// The result is the complete environment to start the detector with.
pub async fn activated_environment(
    prefix: &Path,
    platform: Subdir,
    timeout: Duration,
) -> Result<HashMap<String, String>, ActivationError> {
    let current_env = current_environment();
    let shell = ShellEnum::default();
    let activator = {
        let prefix = prefix.to_path_buf();
        let shell = shell.clone();
        tokio::task::spawn_blocking(move || Activator::from_path(&prefix, shell, platform))
            .await
            .map_err(|error| ActivationError::Io(std::io::Error::other(error)))?
            .map_err(ActivationError::Script)?
    };
    let activation = activator
        .activation(ActivationVariables {
            conda_prefix: None,
            // Leaving the inherited entries out makes the script prepend the
            // prefix directories to `$PATH` as it is, instead of spelling the
            // inherited entries out a second time.
            path: None,
            path_modification_behavior: PathModificationBehavior::Prepend,
            current_env: current_env.clone(),
        })
        .map_err(ActivationError::Script)?;

    // Print the environment before and after the activation script so the
    // difference is exactly what activation changed.
    let mut script = ShellScript::new(shell.clone(), platform);
    script
        .print_env()
        .and_then(|script| script.echo(ENV_SEPARATOR))
        .map(|script| script.append_script(&activation.script))
        .and_then(|script| script.echo(ENV_SEPARATOR))
        .and_then(|script| script.print_env())?;
    let contents = script.contents().map_err(ActivationError::Render)?;

    let script_dir = tempfile::TempDir::new()?;
    let script_path = script_dir
        .path()
        .join(format!("activation.{}", shell.extension()));
    fs_err::tokio::write(&script_path, contents).await?;

    let mut command = tokio::process::Command::from(shell.create_run_script_command(&script_path));
    command.env_clear().envs(&current_env);
    let lossy = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    let output = run_bounded(
        &mut command,
        ProcessLimits {
            timeout,
            output_limit: OUTPUT_LIMIT,
        },
    )
    .await
    .map_err(|error| match error {
        ProcessError::TimedOut { timeout, stderr } => ActivationError::TimedOut {
            timeout,
            stderr: lossy(&stderr),
        },
        ProcessError::OutputLimitExceeded { limit, stderr } => {
            ActivationError::OutputLimitExceeded {
                limit,
                stderr: lossy(&stderr),
            }
        }
        other => ActivationError::Process(other),
    })?;
    if !output.status.success() {
        return Err(ActivationError::Failed {
            status: output.status,
            stderr: lossy(&output.stderr),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some((before, rest)) = stdout.split_once(ENV_SEPARATOR) else {
        return Err(ActivationError::NoEnvironment);
    };
    let Some((_, after)) = rest.rsplit_once(ENV_SEPARATOR) else {
        return Err(ActivationError::NoEnvironment);
    };
    let before = shell.parse_env(before);
    let after = shell.parse_env(after);

    let mut env = current_env;
    for key in before.keys() {
        if !after.contains_key(key) {
            remove_variable(&mut env, key);
        }
    }
    for (key, value) in after {
        if key.is_empty() || before.get(key) == Some(&value) {
            continue;
        }
        remove_variable(&mut env, key);
        env.insert(key.to_string(), value.to_string());
    }
    Ok(env)
}

/// Removes `key` from `env`. On Windows variable names are case-insensitive,
/// so every spelling goes, and the value activation set is the only one the
/// detector sees.
fn remove_variable(env: &mut HashMap<String, String>, key: &str) {
    if cfg!(windows) {
        env.retain(|existing, _| !existing.eq_ignore_ascii_case(key));
    } else {
        env.remove(key);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn activation_prepends_the_prefix_and_applies_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path();
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::create_dir_all(prefix.join("etc/conda/activate.d")).unwrap();
        std::fs::write(
            prefix.join("etc/conda/activate.d/test.sh"),
            "export DETECTOR_TEST_ACTIVATED=1\nunset DETECTOR_TEST_REMOVED\n",
        )
        .unwrap();

        // SAFETY: nextest runs every test in its own process.
        unsafe { std::env::set_var("DETECTOR_TEST_REMOVED", "present") };
        let env =
            activated_environment(prefix, Subdir::current().unwrap(), Duration::from_secs(30))
                .await
                .unwrap();
        assert_eq!(
            env.get("DETECTOR_TEST_ACTIVATED").map(String::as_str),
            Some("1")
        );
        assert!(!env.contains_key("DETECTOR_TEST_REMOVED"));

        let path = env.get("PATH").unwrap();
        let entries: Vec<_> = std::env::split_paths(path).collect();
        assert_eq!(entries[0], prefix.join("bin"));
        let inherited: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").unwrap()).collect();
        assert_eq!(&entries[1..], inherited.as_slice());
    }

    #[tokio::test]
    async fn a_failing_activation_script_keeps_its_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path();
        std::fs::create_dir_all(prefix.join("etc/conda/activate.d")).unwrap();
        std::fs::write(
            prefix.join("etc/conda/activate.d/broken.sh"),
            "echo activation broke >&2\nexit 7\n",
        )
        .unwrap();
        let err =
            activated_environment(prefix, Subdir::current().unwrap(), Duration::from_secs(30))
                .await
                .unwrap_err();
        assert!(err.stderr().unwrap().contains("activation broke"), "{err}");
    }
}
