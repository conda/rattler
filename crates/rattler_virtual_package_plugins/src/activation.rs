//! Activating a plugin's environment before running it.
//!
//! A conda environment is not just a directory of binaries: a package may ship
//! `activate.d` scripts and `state.json` environment variables that its programs
//! expect to have been applied. Skipping them would make a plugin behave
//! differently from every other program in a conda environment, which is
//! surprising in exactly the way a detection plugin cannot afford to be.
//!
//! The activation runs in a shell of its own and the plugin does not. What
//! crosses between them is the set of environment variables the activation
//! changed -- [`Activator::run_activation`] brackets the script with a separator
//! and diffs the environment on either side -- so anything an activation script
//! prints stays on that shell's stdout and can never be mistaken for part of the
//! plugin's report.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use rattler_conda_types::Platform;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::ShellEnum,
};
use tokio::time::Instant;

/// A plugin environment could not be activated.
#[derive(Debug, thiserror::Error)]
pub enum ActivationError {
    /// The activation scripts failed, or there was no shell to run them with.
    #[error("failed to activate the plugin environment at '{}'", prefix.display())]
    Failed {
        /// The prefix that was being activated.
        prefix: PathBuf,
        /// What went wrong, including the script's own output.
        #[source]
        source: rattler_shell::activation::ActivationError,
    },

    /// Activation was still running when the run's deadline passed.
    #[error("activating the plugin environment at '{}' took longer than {budget:?}", prefix.display())]
    TimedOut {
        /// The prefix that was being activated.
        prefix: PathBuf,
        /// The whole run's budget, which activation is only part of.
        budget: Duration,
    },
}

/// The environment variables activating `prefix` produces, on top of the ones
/// this process already has.
///
/// Bounded by `deadline`, which is the deadline of the run as a whole rather
/// than one of activation's own: a plugin that is given five seconds is given
/// five seconds including getting ready. `budget` is what that deadline was set
/// from, and is only used to say so in the error.
///
/// A timed-out activation leaves its shell running. There is no way to cancel a
/// blocking call, and killing a half-finished activation script would be worse
/// than letting it finish into a result nobody reads.
pub async fn activated_environment(
    prefix: &Path,
    platform: Platform,
    deadline: Instant,
    budget: Duration,
) -> Result<HashMap<String, String>, ActivationError> {
    let activator = {
        let prefix = prefix.to_path_buf();
        move || {
            let activator = Activator::from_path(&prefix, ShellEnum::default(), platform)?;
            activator.run_activation(
                ActivationVariables {
                    // The prefix's binary directories go in front of the ones
                    // this process has, so a plugin calling a helper gets its
                    // own copy rather than the host's.
                    path_modification_behavior: PathModificationBehavior::Prepend,
                    ..ActivationVariables::default()
                },
                None,
            )
        }
    };

    let activated = tokio::time::timeout_at(deadline, tokio::task::spawn_blocking(activator))
        .await
        .map_err(|_elapsed| ActivationError::TimedOut {
            prefix: prefix.to_path_buf(),
            budget,
        })?;

    match activated {
        Ok(Ok(environment)) => Ok(environment),
        Ok(Err(source)) => Err(ActivationError::Failed {
            prefix: prefix.to_path_buf(),
            source,
        }),
        // The activation panicked rather than failing, which is this crate's bug
        // rather than the channel's. Reporting it as a failed activation at
        // least names the prefix it happened for.
        Err(join) => Err(ActivationError::Failed {
            prefix: prefix.to_path_buf(),
            source: rattler_shell::activation::ActivationError::IoError(std::io::Error::other(
                join,
            )),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes an `activate.d` script that sets a variable, in whichever shell
    /// this platform activates with.
    fn write_activation_script(prefix: &Path, body_unix: &str, body_windows: &str) {
        let scripts = prefix.join("etc/conda/activate.d");
        std::fs::create_dir_all(&scripts).unwrap();
        if cfg!(windows) {
            std::fs::write(scripts.join("plugin.bat"), body_windows).unwrap();
        } else {
            std::fs::write(scripts.join("plugin.sh"), body_unix).unwrap();
        }
    }

    fn deadline(budget: Duration) -> Instant {
        Instant::now() + budget
    }

    #[tokio::test]
    async fn an_activation_script_reaches_the_environment() {
        let prefix = tempfile::tempdir().unwrap();
        write_activation_script(
            prefix.path(),
            "export PLUGIN_TEST_VARIABLE=from-activation\n",
            "set PLUGIN_TEST_VARIABLE=from-activation\r\n",
        );

        let budget = Duration::from_secs(30);
        let environment =
            activated_environment(prefix.path(), Platform::current(), deadline(budget), budget)
                .await
                .expect("a prefix with one activation script activates");

        assert_eq!(
            environment.get("PLUGIN_TEST_VARIABLE").map(String::as_str),
            Some("from-activation")
        );
    }

    #[tokio::test]
    async fn activation_prepends_the_prefix_to_path() {
        let prefix = tempfile::tempdir().unwrap();
        let budget = Duration::from_secs(30);

        let environment =
            activated_environment(prefix.path(), Platform::current(), deadline(budget), budget)
                .await
                .expect("a prefix with no activation scripts still activates");

        let path = environment
            .get("PATH")
            .expect("activation always sets PATH");
        let first = std::env::split_paths(path)
            .next()
            .expect("PATH is not empty");
        assert_eq!(
            first,
            rattler_shell::activation::prefix_path_entries(prefix.path(), &Platform::current())[0],
            "the plugin environment must precede the inherited PATH"
        );
    }

    #[tokio::test]
    async fn a_failing_activation_script_is_an_error() {
        let prefix = tempfile::tempdir().unwrap();
        write_activation_script(prefix.path(), "exit 1\n", "exit /b 1\r\n");

        let budget = Duration::from_secs(30);
        let error =
            activated_environment(prefix.path(), Platform::current(), deadline(budget), budget)
                .await
                .expect_err("a script that fails must not be ignored");

        assert!(matches!(error, ActivationError::Failed { .. }), "{error}");
    }

    #[tokio::test]
    async fn activation_is_bounded_by_the_run_deadline() {
        let prefix = tempfile::tempdir().unwrap();
        write_activation_script(prefix.path(), "sleep 10\n", "ping -n 11 127.0.0.1 >nul\r\n");

        let budget = Duration::from_secs(1);
        let error =
            activated_environment(prefix.path(), Platform::current(), deadline(budget), budget)
                .await
                .expect_err("a hanging activation must not hang detection");

        assert!(matches!(error, ActivationError::TimedOut { .. }), "{error}");
    }
}
