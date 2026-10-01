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
    activation::{
        ActivationVariables, Activator, BoundedActivationError, PathModificationBehavior,
    },
    environment::EnvironmentSnapshot,
    process::{ProcessError, ProcessLimits},
    shell::ShellEnum,
};
use thiserror::Error;

use crate::limits::OUTPUT_LIMIT;

/// Why activation failed.
#[derive(Debug, Error)]
pub enum ActivationError {
    /// The activation script could not be assembled.
    #[error("failed to build the activation script")]
    Script(#[source] rattler_shell::activation::ActivationError),

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
            Self::Script(_) | Self::Io(_) | Self::NoEnvironment => None,
        }
    }
}

/// Computes the environment of `prefix` after activation on `platform`,
/// starting from an explicit native-string environment snapshot.
///
/// The result is the complete environment to start the detector with.
pub async fn activated_environment(
    prefix: &Path,
    platform: Subdir,
    timeout: Duration,
    inherited: &EnvironmentSnapshot,
) -> Result<EnvironmentSnapshot, ActivationError> {
    let shell = ShellEnum::default();
    let activator = {
        let prefix = prefix.to_path_buf();
        let shell = shell.clone();
        tokio::task::spawn_blocking(move || Activator::from_path(&prefix, shell, platform))
            .await
            .map_err(|error| ActivationError::Io(std::io::Error::other(error)))?
            .map_err(ActivationError::Script)?
    };
    activator
        .run_activation_bounded(
            ActivationVariables {
                conda_prefix: None,
                path: None,
                path_modification_behavior: PathModificationBehavior::Prepend,
                current_env: HashMap::default(),
            },
            inherited,
            ProcessLimits {
                timeout,
                output_limit: OUTPUT_LIMIT,
            },
        )
        .await
        .map_err(|error| {
            let lossy = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
            match error {
                BoundedActivationError::Script(error) => ActivationError::Script(error),
                BoundedActivationError::Process(ProcessError::TimedOut { timeout, stderr }) => {
                    ActivationError::TimedOut {
                        timeout,
                        stderr: lossy(&stderr),
                    }
                }
                BoundedActivationError::Process(ProcessError::OutputLimitExceeded {
                    limit,
                    stderr,
                }) => ActivationError::OutputLimitExceeded {
                    limit,
                    stderr: lossy(&stderr),
                },
                BoundedActivationError::Process(error) => ActivationError::Process(error),
                BoundedActivationError::Failed { status, stderr } => ActivationError::Failed {
                    status,
                    stderr: lossy(&stderr),
                },
                BoundedActivationError::NoEnvironment => ActivationError::NoEnvironment,
            }
        })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

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

        let mut inherited = EnvironmentSnapshot::from_system();
        inherited.insert("DETECTOR_TEST_REMOVED", "present");
        let env = activated_environment(
            prefix,
            Subdir::current().unwrap(),
            Duration::from_secs(30),
            &inherited,
        )
        .await
        .unwrap();
        assert_eq!(
            env.get("DETECTOR_TEST_ACTIVATED")
                .and_then(|value| value.to_str()),
            Some("1")
        );
        assert!(env.get("DETECTOR_TEST_REMOVED").is_none());

        let path = env.get("PATH").unwrap();
        let entries: Vec<_> = std::env::split_paths(path).collect();
        assert_eq!(entries[0], prefix.join("bin"));
        let inherited: Vec<_> = std::env::split_paths(inherited.get("PATH").unwrap()).collect();
        assert_eq!(&entries[1..], inherited.as_slice());
    }

    #[tokio::test]
    async fn activation_preserves_native_values_and_removes_unset_variables() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("etc/conda/activate.d")).unwrap();
        std::fs::write(
            dir.path().join("etc/conda/activate.d/native.sh"),
            "export COPIED=\"$NATIVE\"\nexport CHANGED=\"${NATIVE}suffix\"\nunset REMOVED\nexport EMPTY=\nexport COLLISION=____RATTLER_ENV_START____after\n",
        ).unwrap();
        let native = OsString::from_vec(b"native-\xff".to_vec());
        let inherited: EnvironmentSnapshot = [
            ("PATH", OsString::from("/usr/bin:/bin")),
            ("NATIVE", native.clone()),
            ("REMOVED", native.clone()),
            (
                "COLLISION",
                OsString::from("____RATTLER_ENV_START____before"),
            ),
        ]
        .into_iter()
        .collect();
        let result = activated_environment(
            dir.path(),
            Subdir::current().unwrap(),
            Duration::from_secs(30),
            &inherited,
        )
        .await
        .unwrap();
        assert_eq!(result.get("NATIVE"), Some(native.as_os_str()));
        assert_eq!(result.get("COPIED"), Some(native.as_os_str()));
        let mut changed = native.clone();
        changed.push("suffix");
        assert_eq!(result.get("CHANGED"), Some(changed.as_os_str()));
        assert!(result.get("REMOVED").is_none());
        assert_eq!(result.get("EMPTY"), Some(std::ffi::OsStr::new("")));
        assert_eq!(
            result.get("COLLISION"),
            Some(std::ffi::OsStr::new("____RATTLER_ENV_START____after"))
        );
        assert_eq!(inherited.get("REMOVED"), Some(native.as_os_str()));
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
        let err = activated_environment(
            prefix,
            Subdir::current().unwrap(),
            Duration::from_secs(30),
            &EnvironmentSnapshot::from_system(),
        )
        .await
        .unwrap_err();
        assert!(err.stderr().unwrap().contains("activation broke"), "{err}");
    }
}
