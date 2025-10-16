//! Python bindings for rattler_pty::unix::PtySession
//!
//! This module provides the higher-level interactive PTY session API.

use pyo3::{exceptions::PyRuntimeError, pyclass, pymethods, PyResult};
use std::process::Command;

/// A pseudoterminal (PTY) session for interactive shell use.
///
/// This is the higher-level PTY API built on top of PtyProcess.
/// It provides convenient methods for interactive shell sessions with:
/// - Easy command sending via send_line()
/// - Interactive mode with wait_until pattern matching
/// - Automatic buffering and flushing
///
/// Use this for interactive shell sessions where you want to send commands
/// and optionally hand over control to the user.
#[pyclass]
#[repr(transparent)]
pub struct PyPtySession {
    inner: rattler_pty::unix::PtySession,
}

#[pymethods]
impl PyPtySession {
    /// Create a new PTY session with the given command.
    ///
    /// The PTY session is created with echo enabled by default, which is
    /// appropriate for interactive shell use.
    ///
    /// Arguments:
    ///     command: A list of strings representing the command and its arguments.
    ///              The first element is the executable, subsequent elements are arguments.
    ///
    /// Returns:
    ///     A new PtySession instance.
    ///
    /// Raises:
    ///     RuntimeError: If the PTY session could not be created.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> from rattler import PtySession
    /// >>> # Start bash
    /// >>> session = PtySession(["bash"])
    /// >>> # Start bash with specific arguments
    /// >>> session = PtySession(["bash", "-l"])
    /// >>> # Start a different shell
    /// >>> session = PtySession(["zsh"])
    /// ```
    #[new]
    #[pyo3(signature = (command))]
    pub fn new(command: Vec<String>) -> PyResult<Self> {
        if command.is_empty() {
            return Err(PyRuntimeError::new_err(
                "Command cannot be empty. Please provide at least an executable name.",
            ));
        }

        // Build the command
        let mut cmd = Command::new(&command[0]);
        if command.len() > 1 {
            cmd.args(&command[1..]);
        }

        // Create the PTY session
        let session = rattler_pty::unix::PtySession::new(cmd)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create PTY session: {}", e)))?;

        Ok(PyPtySession { inner: session })
    }

    /// Send a string followed by a newline to the PTY session.
    ///
    /// This is like typing a command and pressing Enter. The command is flushed
    /// immediately, so the shell will receive it right away.
    ///
    /// Arguments:
    ///     line: The string to send (newline will be added automatically).
    ///
    /// Returns:
    ///     The number of bytes written.
    ///
    /// Raises:
    ///     RuntimeError: If the write operation fails.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> session = PtySession(["bash"])
    /// >>> session.send_line("export MY_VAR=hello")
    /// >>> session.send_line("echo $MY_VAR")
    /// ```
    pub fn send_line(&mut self, line: &str) -> PyResult<usize> {
        self.inner
            .send_line(line)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to send line: {}", e)))
    }

    /// Send a string to the PTY session without adding a newline.
    ///
    /// Note: You'll need to call flush() to ensure the data is sent.
    ///
    /// Arguments:
    ///     data: The string to send.
    ///
    /// Returns:
    ///     The number of bytes written.
    ///
    /// Raises:
    ///     RuntimeError: If the write operation fails.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> session = PtySession(["bash"])
    /// >>> session.send("echo")
    /// >>> session.send(" hello")
    /// >>> session.send("\n")
    /// >>> session.flush()
    /// ```
    pub fn send(&mut self, data: &str) -> PyResult<usize> {
        self.inner
            .send(data)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to send data: {}", e)))
    }

    /// Flush any pending output to the PTY.
    ///
    /// This is automatically called by send_line(), but can be called manually
    /// if you use send().
    ///
    /// Raises:
    ///     RuntimeError: If the flush operation fails.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> session = PtySession(["bash"])
    /// >>> session.send("echo hello\n")
    /// >>> session.flush()  # Make sure the command is sent
    /// ```
    pub fn flush(&mut self) -> PyResult<()> {
        self.inner
            .flush()
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to flush PTY: {}", e)))
    }

    /// Exit the process gracefully by sending SIGTERM.
    ///
    /// This method blocks until the process has exited. Useful for cleaning up
    /// PTY sessions when you're done sending commands but don't want to use interact().
    ///
    /// Returns:
    ///     A string describing the exit status.
    ///
    /// Raises:
    ///     RuntimeError: If the process could not be terminated.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> session = PtySession(["bash"])
    /// >>> session.send_line("echo hello")
    /// >>> status = session.exit()
    /// >>> print(status)
    /// Exited(0)
    /// ```
    pub fn exit(&mut self) -> PyResult<String> {
        let status = self
            .inner
            .exit()
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to exit PTY session: {}", e)))?;
        Ok(format!("{:?}", status))
    }

    /// Start an interactive session, optionally waiting for a pattern first.
    ///
    /// This method:
    /// 1. Sets the terminal to raw mode
    /// 2. If wait_until is provided, buffers output until that pattern appears
    /// 3. Then forwards all I/O between the user's terminal and the PTY
    /// 4. Returns when the shell process exits
    ///
    /// Arguments:
    ///     wait_until: Optional pattern to wait for before showing output.
    ///                 Useful for waiting for shell initialization to complete.
    ///                 If not found within 1 second, a warning is shown and
    ///                 interaction begins anyway.
    ///
    /// Returns:
    ///     The exit code of the shell process, or None if terminated by signal.
    ///
    /// Raises:
    ///     RuntimeError: If interaction fails.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> session = PtySession(["bash"])
    /// >>> # Send some setup commands
    /// >>> session.send_line("export MY_VAR=hello")
    /// >>> session.send_line("echo 'READY'")
    /// >>> # Wait for "READY" before handing control to user
    /// >>> exit_code = session.interact(wait_until="READY")
    /// >>> print(f"Session exited with code: {exit_code}")
    /// ```
    ///
    /// ```python
    /// >>> session = PtySession(["bash"])
    /// >>> # Interact immediately without waiting
    /// >>> exit_code = session.interact()
    /// ```
    #[pyo3(signature = (wait_until=None))]
    pub fn interact(&mut self, wait_until: Option<&str>) -> PyResult<Option<i32>> {
        self.inner
            .interact(wait_until)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to interact with PTY: {}", e)))
    }
}
