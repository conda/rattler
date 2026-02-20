//! Python bindings for `rattler_pty::unix::PtyProcess`
//!
//! This module provides the lower-level PTY process control API.

use pyo3::{exceptions::PyRuntimeError, pyclass, pymethods, Bound, PyAny, PyResult, Python};
use pyo3_async_runtimes::tokio::future_into_py;
use std::os::fd::{AsRawFd, IntoRawFd};
use std::process::Command;

/// Options for creating a PTY process.
///
/// Controls behavior like whether input is echoed back to the terminal.
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPtyProcessOptions {
    pub(crate) inner: rattler_pty::unix::PtyProcessOptions,
}

#[pymethods]
impl PyPtyProcessOptions {
    /// Create options for a PTY process.
    ///
    /// Arguments:
    ///     echo: Whether to echo input back to the terminal. Defaults to True.
    ///
    /// Returns:
    ///     A new `PtyProcessOptions` instance.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> from rattler import PtyProcessOptions
    /// >>> # Create with echo enabled (default)
    /// >>> opts = PtyProcessOptions()
    /// >>> # Create with echo disabled
    /// >>> opts = PtyProcessOptions(echo=False)
    /// ```
    #[new]
    #[pyo3(signature = (echo=true))]
    pub fn new(echo: bool) -> Self {
        PyPtyProcessOptions {
            inner: rattler_pty::unix::PtyProcessOptions {
                echo,
                window_size: None,
            },
        }
    }

    /// Whether input is echoed back to the terminal.
    #[getter]
    pub fn echo(&self) -> bool {
        self.inner.echo
    }
}

/// A pseudoterminal (PTY) process.
///
/// This is the lower-level PTY API that gives you more control over the process.
/// Use this when you need to:
/// - Read/write to the PTY manually using file handles
/// - Check process status
/// - Control process termination with specific signals
///
/// For interactive shell sessions, consider using `PtySession` instead, which
/// provides higher-level conveniences like `send_line()` and `interact()`.
#[pyclass]
#[repr(transparent)]
pub struct PyPtyProcess {
    pub(crate) inner: rattler_pty::unix::PtyProcess,
}

#[pymethods]
impl PyPtyProcess {
    /// Create a new PTY process with the given command.
    ///
    /// Arguments:
    ///     command: A list of strings representing the command and its arguments.
    ///              The first element is the executable, subsequent elements are arguments.
    ///     options: Optional `PtyProcessOptions` to configure the PTY behavior.
    ///              If not provided, defaults to echo=True.
    ///
    /// Returns:
    ///     A new `PtyProcess` instance.
    ///
    /// Raises:
    ///     `RuntimeError`: If the PTY process could not be created.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> from rattler import PtyProcess, PtyProcessOptions
    /// >>> # Create with default options (echo enabled)
    /// >>> process = PtyProcess(["bash"])
    /// >>> # Create with custom options
    /// >>> opts = PtyProcessOptions(echo=False)
    /// >>> process = PtyProcess(["bash", "-l"], opts)
    /// >>> # Check process status
    /// >>> status = process.status()
    /// >>> print(status)
    /// StillAlive
    /// ```
    #[new]
    #[pyo3(signature = (command, options=None))]
    pub fn new(command: Vec<String>, options: Option<PyPtyProcessOptions>) -> PyResult<Self> {
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

        // Use provided options or default
        let opts = options.map_or(
            rattler_pty::unix::PtyProcessOptions {
                echo: true,
                window_size: None,
            },
            |o| o.inner,
        );

        // Create the PTY process
        let process = rattler_pty::unix::PtyProcess::new(cmd, opts)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create PTY process: {e}")))?;

        Ok(PyPtyProcess { inner: process })
    }

    /// Get the process ID (PID) of the child process.
    ///
    /// Returns:
    ///     The PID as an integer.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> process = PtyProcess(["bash"])
    /// >>> pid = process.child_pid
    /// >>> print(f"Process ID: {pid}")
    /// Process ID: 12345
    /// ```
    #[getter]
    pub fn child_pid(&self) -> i32 {
        self.inner.child_pid.as_raw()
    }

    /// Check the status of the child process (non-blocking).
    ///
    /// This runs `waitpid()` with WNOHANG, so it returns immediately.
    /// Note: If you previously called `exit()` or `status()` returned an exit status,
    /// subsequent calls may return None.
    ///
    /// Returns:
    ///     A string representing the process status, or None if status cannot be determined.
    ///     Possible values:
    ///     - "StillAlive": Process is still running
    ///     - "Exited(code)": Process exited with the given exit code
    ///     - "Signaled(signal)": Process was terminated by a signal
    ///     - "Stopped": Process was stopped
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> import time
    /// >>> process = PtyProcess(["sleep", "10"])
    /// >>> print(process.status())
    /// StillAlive
    /// >>> time.sleep(11)
    /// >>> print(process.status())
    /// Exited(0)
    /// ```
    pub fn status(&self) -> Option<String> {
        use nix::sys::wait::WaitStatus;

        self.inner.status().map(|status| match status {
            WaitStatus::Exited(_, code) => format!("Exited({code})"),
            WaitStatus::Signaled(_, signal, _) => format!("Signaled({signal:?})"),
            WaitStatus::Stopped(_, _) => "Stopped".to_string(),
            WaitStatus::StillAlive => "StillAlive".to_string(),
            WaitStatus::Continued(_) => "Continued".to_string(),
            #[allow(clippy::match_wildcard_for_single_variants)]
            _ => "Unknown".to_string(),
        })
    }

    /// Exit the process gracefully by sending SIGTERM.
    ///
    /// This method blocks until the process has exited. If the process doesn't
    /// respond to SIGTERM, it will eventually be killed with SIGKILL if a
    /// `kill_timeout` was set (not currently exposed to Python).
    ///
    /// Returns:
    ///     A string describing the exit status.
    ///
    /// Raises:
    ///     `RuntimeError`: If the process could not be terminated.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> process = PtyProcess(["bash"])
    /// >>> status = process.exit()
    /// >>> print(status)
    /// Exited(0)
    /// ```
    pub fn exit(&mut self) -> PyResult<String> {
        use nix::sys::wait::WaitStatus;

        let status = self
            .inner
            .exit()
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to exit process: {e}")))?;

        Ok(match status {
            WaitStatus::Exited(_, code) => format!("Exited({code})"),
            WaitStatus::Signaled(_, signal, _) => format!("Signaled({signal:?})"),
            _ => format!("Unknown({status:?})"),
        })
    }

    /// Get a file descriptor for reading from and writing to the PTY.
    ///
    /// This returns a raw file descriptor (integer) that can be converted to a
    /// Python file object using `os.fdopen()`. This is useful for non-interactive
    /// automation where you want to programmatically read the process output.
    ///
    /// Returns:
    ///     A file descriptor (integer) for the PTY.
    ///
    /// Raises:
    ///     `RuntimeError`: If the file descriptor could not be created.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> import os
    /// >>> process = PtyProcess(["bash"])
    /// >>> fd = process.get_file_handle()
    /// >>> file = os.fdopen(fd, 'r+b', buffering=0)
    /// >>> # Write to the process
    /// >>> file.write(b"echo hello\\n")
    /// >>> file.flush()
    /// >>> # Read output (this is a blocking operation)
    /// >>> output = file.read(100)
    /// >>> print(output)
    /// ```
    pub fn get_file_handle(&self) -> PyResult<i32> {
        let file = self
            .inner
            .get_file_handle()
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to get file handle: {e}")))?;

        Ok(file.into_raw_fd())
    }

    /// Get the kill timeout in seconds.
    ///
    /// Returns:
    ///     The timeout in seconds as a float, or None if no timeout is set.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> process = PtyProcess(["bash"])
    /// >>> timeout = process.get_kill_timeout()
    /// >>> print(timeout)
    /// None
    /// ```
    pub fn get_kill_timeout(&self) -> Option<f64> {
        self.inner.kill_timeout().map(|d| d.as_secs_f64())
    }

    /// Set the kill timeout in seconds.
    ///
    /// When calling `exit()` or `async_exit()`, if the process doesn't respond to
    /// SIGTERM within this timeout, it will be forcefully killed with SIGKILL.
    ///
    /// Arguments:
    ///     timeout: Timeout in seconds, or None to disable timeout.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> process = PtyProcess(["bash"])
    /// >>> process.set_kill_timeout(5.0)  # 5 second timeout
    /// >>> process.set_kill_timeout(None)  # Disable timeout
    /// ```
    pub fn set_kill_timeout(&mut self, timeout: Option<f64>) {
        use std::time::Duration;
        self.inner
            .set_kill_timeout(timeout.map(Duration::from_secs_f64));
    }

    /// Read from the PTY asynchronously.
    ///
    /// This is the async version of reading via `get_file_handle()`. It performs
    /// non-blocking I/O using tokio, which is bridged to Python's asyncio.
    ///
    /// Arguments:
    ///     size: Maximum number of bytes to read. Defaults to 8192.
    ///
    /// Returns:
    ///     A coroutine that resolves to bytes read from the PTY.
    ///
    /// Raises:
    ///     `RuntimeError`: If the read operation fails.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> import asyncio
    /// >>> async def main():
    /// ...     process = PtyProcess(["bash", "-c", "echo hello"])
    /// ...     data = await process.async_read(1024)
    /// ...     print(data)
    /// >>> asyncio.run(main())
    /// ```
    #[pyo3(signature = (size=8192))]
    pub fn async_read<'py>(&self, py: Python<'py>, size: usize) -> PyResult<Bound<'py, PyAny>> {
        let pty_fd = self.inner.pty.as_raw_fd();

        future_into_py(py, async move {
            use nix::unistd::dup;
            use std::os::fd::{BorrowedFd, FromRawFd};
            use tokio::io::AsyncReadExt;

            let borrowed_fd = unsafe { BorrowedFd::borrow_raw(pty_fd) };
            let fd = dup(borrowed_fd).map_err(|e| {
                PyRuntimeError::new_err(format!("Failed to duplicate file descriptor: {e}"))
            })?;

            let mut file = unsafe { tokio::fs::File::from_raw_fd(fd.into_raw_fd()) };

            let mut buf = vec![0u8; size];
            let n = file
                .read(&mut buf)
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Failed to read from PTY: {e}")))?;

            buf.truncate(n);
            Ok(buf)
        })
    }

    /// Write to the PTY asynchronously.
    ///
    /// This is the async version of writing via `get_file_handle()`. It performs
    /// non-blocking I/O using tokio.
    ///
    /// Arguments:
    ///     data: Bytes to write to the PTY.
    ///
    /// Returns:
    ///     A coroutine that resolves to the number of bytes written.
    ///
    /// Raises:
    ///     `RuntimeError`: If the write operation fails.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> import asyncio
    /// >>> async def main():
    /// ...     process = PtyProcess(["cat"])
    /// ...     n = await process.async_write(b"hello\\n")
    /// ...     print(f"Wrote {n} bytes")
    /// >>> asyncio.run(main())
    /// ```
    pub fn async_write<'py>(&self, py: Python<'py>, data: Vec<u8>) -> PyResult<Bound<'py, PyAny>> {
        let pty_fd = self.inner.pty.as_raw_fd();

        future_into_py(py, async move {
            use nix::unistd::dup;
            use std::os::fd::{BorrowedFd, FromRawFd};
            use tokio::io::AsyncWriteExt;

            let borrowed_fd = unsafe { BorrowedFd::borrow_raw(pty_fd) };
            let fd = dup(borrowed_fd).map_err(|e| {
                PyRuntimeError::new_err(format!("Failed to duplicate file descriptor: {e}"))
            })?;

            let mut file = unsafe { tokio::fs::File::from_raw_fd(fd.into_raw_fd()) };

            let n = file
                .write(&data)
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Failed to write to PTY: {e}")))?;

            Ok(n)
        })
    }

    /// Wait for the process to exit asynchronously.
    ///
    /// This is the async version of polling `status()` in a loop. It polls the
    /// process status periodically without blocking the async runtime.
    ///
    /// Returns:
    ///     A coroutine that resolves to a string describing the exit status.
    ///
    /// Raises:
    ///     `RuntimeError`: If waiting fails.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> import asyncio
    /// >>> async def main():
    /// ...     process = PtyProcess(["sleep", "1"])
    /// ...     status = await process.async_wait()
    /// ...     print(status)
    /// >>> asyncio.run(main())
    /// Exited(0)
    /// ```
    pub fn async_wait<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let child_pid = self.inner.child_pid;

        future_into_py(py, async move {
            use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
            use tokio::time::{sleep, Duration};

            loop {
                match waitpid(child_pid, Some(WaitPidFlag::WNOHANG)) {
                    Ok(WaitStatus::StillAlive | WaitStatus::Continued(_)) => {
                        sleep(Duration::from_millis(100)).await;
                    }
                    Ok(status) => {
                        return Ok(match status {
                            WaitStatus::Exited(_, code) => format!("Exited({code})"),
                            WaitStatus::Signaled(_, signal, _) => format!("Signaled({signal:?})"),
                            WaitStatus::Stopped(_, _) => "Stopped".to_string(),
                            _ => "Unknown".to_string(),
                        });
                    }
                    Err(e) => {
                        return Err(PyRuntimeError::new_err(format!(
                            "Failed to wait for process: {e}"
                        )));
                    }
                }
            }
        })
    }

    /// Exit the process gracefully asynchronously by sending SIGTERM.
    ///
    /// This is the async version of `exit()`. It sends SIGTERM and waits for
    /// the process to terminate without blocking the async runtime.
    ///
    /// Returns:
    ///     A coroutine that resolves to a string describing the exit status.
    ///
    /// Raises:
    ///     `RuntimeError`: If the process could not be terminated.
    ///
    /// Examples
    /// --------
    /// ```python
    /// >>> import asyncio
    /// >>> async def main():
    /// ...     process = PtyProcess(["bash"])
    /// ...     status = await process.async_exit()
    /// ...     print(status)
    /// >>> asyncio.run(main())
    /// ```
    pub fn async_exit<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        use nix::sys::signal::Signal;
        let child_pid = self.inner.child_pid;
        let kill_timeout = self.inner.kill_timeout();

        future_into_py(py, async move {
            use nix::sys::{
                signal,
                wait::{waitpid, WaitPidFlag, WaitStatus},
            };
            use tokio::time::{sleep, Duration, Instant};

            let sig = Signal::SIGTERM;
            let start = Instant::now();

            loop {
                match signal::kill(child_pid, sig) {
                    Ok(_) => {}
                    // Process was already killed
                    Err(nix::errno::Errno::ESRCH) => {
                        return Ok("Exited(0)".to_string());
                    }
                    Err(e) => {
                        return Err(PyRuntimeError::new_err(format!(
                            "Failed to kill process: {e}"
                        )));
                    }
                }

                match waitpid(child_pid, Some(WaitPidFlag::WNOHANG)) {
                    Ok(WaitStatus::StillAlive | WaitStatus::Continued(_)) => {
                        sleep(Duration::from_millis(100)).await;
                    }
                    Ok(status) => {
                        return Ok(match status {
                            WaitStatus::Exited(_, code) => format!("Exited({code})"),
                            WaitStatus::Signaled(_, signal, _) => format!("Signaled({signal:?})"),
                            _ => format!("Unknown({status:?})"),
                        });
                    }
                    Err(e) => {
                        return Err(PyRuntimeError::new_err(format!(
                            "Failed to wait for process: {e}"
                        )));
                    }
                }

                // Kill -9 if timeout is reached
                if let Some(timeout) = kill_timeout {
                    if start.elapsed() > timeout {
                        signal::kill(child_pid, signal::Signal::SIGKILL).map_err(|e| {
                            PyRuntimeError::new_err(format!("Failed to SIGKILL process: {e}"))
                        })?;
                    }
                }
            }
        })
    }
}
