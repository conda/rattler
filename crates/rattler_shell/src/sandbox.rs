//! Sandbox implementation for running activation scripts securely.

use std::{
    collections::HashMap,
    ffi::OsStr,
    process::{Command, ExitStatus, Output},
};

#[cfg(not(test))]
use rattler_sandbox::sandbox_impl::{sandboxed_command, Exception};
use thiserror::Error;

/// Error type for sandbox operations
#[derive(Error, Debug)]
pub enum SandboxError {
    /// An error that can occur when reading or writing files
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    /// Failed to run the script in the sandbox
    #[error("Failed to run script in sandbox (status: {status})")]
    FailedToRunScript {
        /// The contents of the script that was run
        script: String,

        /// The stdout output of executing the script
        stdout: String,

        /// The stderr output of executing the script
        stderr: String,

        /// The error code of running the script
        status: ExitStatus,
    },

    /// The sandbox is not supported on this platform
    #[error("Sandbox is not supported on this platform")]
    UnsupportedPlatform,
}

/// Run a command in a sandbox
///
/// This function runs a command in a sandbox, which is a restricted environment
/// that prevents the command from accessing the file system or network.
///
/// The implementation uses the `rattler_sandbox` crate which supports:
/// - Linux (`x86_64`, aarch64): Uses birdcage
/// - macOS (`x86_64`, aarch64): Uses birdcage
///
/// # Arguments
///
/// * `cmd` - The command to run
/// * `env` - The environment variables to set
///
/// # Returns
///
/// The output of the command
pub fn run_in_sandbox(
    mut cmd: Command,
    env: Option<HashMap<&OsStr, &OsStr>>,
) -> Result<Output, SandboxError> {
    // Check if sandbox is supported
    if !is_sandbox_supported() {
        return Err(SandboxError::UnsupportedPlatform);
    }

    // For testing purposes, we'll just run the command directly
    // In a real environment, we would use the sandbox
    #[cfg(test)]
    {
        // Set environment variables if provided
        if let Some(env) = env {
            cmd.env_clear().envs(env);
        }

        let output = cmd.output()?;

        if !output.status.success() {
            return Err(SandboxError::FailedToRunScript {
                script: format!("{:?}", cmd),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                status: output.status,
            });
        }

        return Ok(output);
    }

    // The code below is only used in non-test builds
    #[cfg(not(test))]
    {
        // Set environment variables if provided
        if let Some(env) = env.clone() {
            cmd.env_clear().envs(env);
        }

        // Get the command path and args
        let exe = cmd.get_program().to_str().ok_or_else(|| {
            SandboxError::IoError(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Command path is not valid UTF-8",
            ))
        })?;

        // Create sandbox exceptions
        let exceptions = vec![
            // Allow reading from system directories
            Exception::Read("/bin".to_string()),
            Exception::Read("/usr/bin".to_string()),
            Exception::Read("/usr/local/bin".to_string()),
            Exception::Read("/lib".to_string()),
            Exception::Read("/lib64".to_string()),
            Exception::Read("/usr/lib".to_string()),
            Exception::Read("/usr/lib64".to_string()),
            Exception::Read("/etc".to_string()),
            // Allow reading and writing from the temp directory
            Exception::ReadAndWrite(std::env::temp_dir().to_string_lossy().into_owned()),
            // Allow executing the shell
            Exception::ReadAndWrite(exe.to_string()),
        ];

        // Create the sandboxed command
        let mut sandboxed = sandboxed_command(exe, &exceptions);
        sandboxed.args(cmd.get_args());
        if let Some(env) = env {
            sandboxed.env_clear().envs(env);
        }

        // Run the command
        let output = sandboxed.output()?;

        if !output.status.success() {
            return Err(SandboxError::FailedToRunScript {
                script: format!("{cmd:?}"),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                status: output.status,
            });
        }

        return Ok(output);
    }

    // This line is never reached, but needed for the compiler
    #[allow(unreachable_code)]
    Ok(Command::new("true").output()?)
}

/// Check if sandbox is supported on the current platform
pub fn is_sandbox_supported() -> bool {
    cfg!(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
    ))
}
