//! Running a detector executable within the protocol's limits.
//!
//! The executable is looked up only in the prefix's `PATH` directories, in CEP
//! 32 order, and started with no arguments, no standard input and the
//! activated environment. The clock starts at spawn. When the timeout is
//! reached, or when standard output and standard error together reach the
//! output limit, the process and its descendants are terminated and the run
//! fails. Standard error captured until then is kept for diagnostics.

use std::{
    path::{Path, PathBuf},
    process::ExitStatus,
    time::Duration,
};

use rattler_conda_types::{PackageName, Subdir};
use rattler_shell::{
    activation::prefix_path_entries,
    environment::EnvironmentSnapshot,
    process::{ProcessError, ProcessLimits, run_bounded},
};
use thiserror::Error;

use crate::limits::{DEFAULT_TIMEOUT, OUTPUT_LIMIT};

/// The bounds of one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunLimits {
    /// How long the process may run after it was spawned.
    pub timeout: Duration,
    /// How many bytes of standard output and standard error, combined, the
    /// process may write.
    pub output_limit: usize,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            output_limit: OUTPUT_LIMIT,
        }
    }
}

/// A successful run: the process exited with status `0` within the limits.
#[derive(Clone, Debug)]
pub struct DetectorRun {
    /// The executable that ran.
    pub executable: PathBuf,
    /// Everything the process wrote to standard output.
    pub stdout: Vec<u8>,
    /// Everything the process wrote to standard error, lossily decoded.
    pub stderr: String,
    /// How long the process ran.
    pub duration: Duration,
}

/// Why a run failed.
#[derive(Debug, Error)]
pub enum RunError {
    /// No executable with the detector's name exists in the prefix's `PATH`
    /// directories.
    #[error("no executable named '{name}' in {}", searched.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))]
    ExecutableNotFound {
        /// The executable name that was looked for.
        name: String,
        /// The directories that were searched, in order.
        searched: Vec<PathBuf>,
    },

    /// The process could not be started or its output could not be read.
    #[error("failed to run {executable}")]
    Io {
        /// The executable.
        executable: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
        /// Standard error captured before the failure.
        stderr: String,
    },

    /// The process did not exit within the timeout and was terminated.
    #[error("the detector did not finish within {timeout:?} and was terminated")]
    TimedOut {
        /// The timeout that was reached.
        timeout: Duration,
        /// Standard error captured before termination.
        stderr: String,
    },

    /// The process reached the combined output limit and was terminated.
    #[error("the detector reached the {limit}-byte output limit and was terminated")]
    OutputLimitExceeded {
        /// The limit that was exceeded.
        limit: usize,
        /// Standard error captured before termination.
        stderr: String,
    },

    /// The process exited with a nonzero status.
    #[error("the detector exited with {status}")]
    Exited {
        /// The exit status.
        status: ExitStatus,
        /// Everything the process wrote to standard error.
        stderr: String,
    },
}

impl RunError {
    /// The standard error captured before the run failed, where a process ran
    /// at all.
    pub fn stderr(&self) -> Option<&str> {
        match self {
            Self::Io { stderr, .. }
            | Self::TimedOut { stderr, .. }
            | Self::OutputLimitExceeded { stderr, .. }
            | Self::Exited { stderr, .. } => Some(stderr),
            Self::ExecutableNotFound { .. } => None,
        }
    }
}

/// Finds the detector executable named after `detector` in the `PATH`
/// directories of `prefix` for `platform`.
///
/// On Windows the `.exe`, `.cmd` and `.bat` extensions are tried in that order
/// within each directory; elsewhere the bare name must be an executable file.
pub fn find_executable(
    prefix: &Path,
    detector: &PackageName,
    platform: Subdir,
) -> Result<PathBuf, RunError> {
    let directories = prefix_path_entries(prefix, &platform);
    let name = detector.as_normalized();
    let candidates: Vec<String> = if platform.is_windows() {
        ["exe", "cmd", "bat"]
            .iter()
            .map(|extension| format!("{name}.{extension}"))
            .collect()
    } else {
        vec![name.to_string()]
    };
    for directory in &directories {
        for candidate in &candidates {
            let path = directory.join(candidate);
            if is_executable_file(&path, platform) {
                return Ok(path);
            }
        }
    }
    Err(RunError::ExecutableNotFound {
        name: name.to_string(),
        searched: directories,
    })
}

/// Whether `path` is a regular file the platform would execute.
fn is_executable_file(path: &Path, platform: Subdir) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    if platform.is_windows() {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Runs the detector `detector` installed in `prefix` with the activated
/// environment `env`.
pub async fn run_detector(
    prefix: &Path,
    detector: &PackageName,
    platform: Subdir,
    env: &EnvironmentSnapshot,
    limits: RunLimits,
) -> Result<DetectorRun, RunError> {
    let executable = find_executable(prefix, detector, platform)?;
    let mut command = tokio::process::Command::new(&executable);
    command.env_clear().envs(env.iter());

    let lossy = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    let output = run_bounded(
        &mut command,
        ProcessLimits {
            timeout: limits.timeout,
            output_limit: limits.output_limit,
        },
    )
    .await
    .map_err(|error| match error {
        ProcessError::Spawn(source) => RunError::Io {
            executable: executable.clone(),
            source,
            stderr: String::new(),
        },
        ProcessError::Io { source, stderr } => RunError::Io {
            executable: executable.clone(),
            source,
            stderr: lossy(&stderr),
        },
        ProcessError::TimedOut { timeout, stderr } => RunError::TimedOut {
            timeout,
            stderr: lossy(&stderr),
        },
        ProcessError::OutputLimitExceeded { limit, stderr } => RunError::OutputLimitExceeded {
            limit,
            stderr: lossy(&stderr),
        },
    })?;

    let stderr = lossy(&output.stderr);
    if !output.status.success() {
        return Err(RunError::Exited {
            status: output.status,
            stderr,
        });
    }
    Ok(DetectorRun {
        executable,
        stdout: output.stdout,
        stderr,
        duration: output.duration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = bin.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(windows)]
    fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let scripts = dir.join("Scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        let path = scripts.join(format!("{name}.bat"));
        std::fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap();
        path
    }

    fn name(name: &str) -> PackageName {
        PackageName::try_from(name).unwrap()
    }

    fn env() -> EnvironmentSnapshot {
        EnvironmentSnapshot::from_system()
    }

    #[tokio::test]
    async fn runs_a_detector_and_captures_both_streams() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let body = "echo '{\"version\": 1}'\necho diagnostic >&2";
        #[cfg(windows)]
        let body = "echo {\"version\": 1}\r\necho diagnostic 1>&2";
        let executable = write_script(dir.path(), "ok-detect", body);

        let run = run_detector(
            dir.path(),
            &name("ok-detect"),
            Subdir::current().unwrap(),
            &env(),
            RunLimits::default(),
        )
        .await
        .unwrap();
        assert_eq!(run.executable, executable);
        assert_eq!(
            String::from_utf8(run.stdout).unwrap().trim(),
            "{\"version\": 1}"
        );
        assert_eq!(run.stderr.trim(), "diagnostic");
    }

    #[tokio::test]
    async fn missing_executable() {
        let dir = tempfile::tempdir().unwrap();
        let err = run_detector(
            dir.path(),
            &name("nope"),
            Subdir::current().unwrap(),
            &env(),
            RunLimits::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RunError::ExecutableNotFound { .. }), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_without_the_execute_bit_is_not_an_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = write_script(dir.path(), "plain-detect", "echo hi");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = find_executable(
            dir.path(),
            &name("plain-detect"),
            Subdir::current().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, RunError::ExecutableNotFound { .. }), "{err}");
    }

    #[tokio::test]
    async fn nonzero_exit_keeps_stderr() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let body = "echo failing >&2\nexit 3";
        #[cfg(windows)]
        let body = "echo failing 1>&2\r\nexit /b 3";
        write_script(dir.path(), "bad-detect", body);
        let err = run_detector(
            dir.path(),
            &name("bad-detect"),
            Subdir::current().unwrap(),
            &env(),
            RunLimits::default(),
        )
        .await
        .unwrap_err();
        match err {
            RunError::Exited { status, stderr } => {
                assert_eq!(status.code(), Some(3));
                assert_eq!(stderr.trim(), "failing");
            }
            other => panic!("unexpected error {other}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_is_reported_with_the_captured_stderr() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "slow-detect", "echo starting >&2\nsleep 30");
        let started = std::time::Instant::now();
        let err = run_detector(
            dir.path(),
            &name("slow-detect"),
            Subdir::current().unwrap(),
            &env(),
            RunLimits {
                timeout: Duration::from_millis(300),
                output_limit: OUTPUT_LIMIT,
            },
        )
        .await
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        match err {
            RunError::TimedOut { stderr, .. } => assert_eq!(stderr.trim(), "starting"),
            other => panic!("unexpected error {other}"),
        }
    }

    #[test]
    fn windows_extension_order() {
        let dir = tempfile::tempdir().unwrap();
        let scripts = dir.path().join("Scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("x-detect.bat"), "").unwrap();
        std::fs::write(scripts.join("x-detect.cmd"), "").unwrap();
        // A bare file never matches on Windows.
        std::fs::write(scripts.join("x-detect"), "").unwrap();
        let found = find_executable(dir.path(), &name("x-detect"), Subdir::Win64).unwrap();
        assert_eq!(found, scripts.join("x-detect.cmd"));
        std::fs::write(scripts.join("x-detect.exe"), "").unwrap();
        let found = find_executable(dir.path(), &name("x-detect"), Subdir::Win64).unwrap();
        assert_eq!(found, scripts.join("x-detect.exe"));
        // `Library/bin` comes before `Scripts` in CEP 32 order.
        let library_bin = dir.path().join("Library/bin");
        std::fs::create_dir_all(&library_bin).unwrap();
        std::fs::write(library_bin.join("x-detect.bat"), "").unwrap();
        let found = find_executable(dir.path(), &name("x-detect"), Subdir::Win64).unwrap();
        assert_eq!(found, library_bin.join("x-detect.bat"));
    }
}
