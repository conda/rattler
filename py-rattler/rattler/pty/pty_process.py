from __future__ import annotations

import os
from typing import BinaryIO, Optional

# Try to import PTY classes - they may not be available on Windows or without pty feature
try:
    from rattler.rattler import PyPtyProcess, PyPtyProcessOptions

    _PTY_AVAILABLE = True
except (ImportError, AttributeError):
    _PTY_AVAILABLE = False


class PtyProcessOptions:
    """
    Options for creating a PTY process.

    Controls behavior like whether input is echoed back to the terminal.
    """

    _inner: PyPtyProcessOptions

    def __init__(self, echo: bool = True) -> None:
        """
        Create options for a PTY process.

        Arguments:
            echo: Whether to echo input back to the terminal. Defaults to True.

        Examples
        --------
        ```python
        >>> from rattler.pty import PtyProcessOptions
        >>> # Create with echo enabled (default)
        >>> opts = PtyProcessOptions()
        >>> opts.echo
        True
        >>> # Create with echo disabled
        >>> opts = PtyProcessOptions(echo=False)
        >>> opts.echo
        False
        >>>
        ```
        """
        if not _PTY_AVAILABLE:
            raise ImportError("PTY functionality is not available on this platform")
        self._inner = PyPtyProcessOptions(echo)

    @classmethod
    def _from_py_pty_process_options(cls, py_pty_process_options: PyPtyProcessOptions) -> PtyProcessOptions:
        """Construct from FFI PyPtyProcessOptions object."""
        opts = cls.__new__(cls)
        opts._inner = py_pty_process_options
        return opts

    @property
    def echo(self) -> bool:
        """Whether input is echoed back to the terminal."""
        return self._inner.echo

    def __repr__(self) -> str:
        return f"PtyProcessOptions(echo={self.echo})"


class PtyProcess:
    """
    A pseudoterminal (PTY) process.

    This is the lower-level PTY API that gives you more control over the process.
    Use this when you need to:
    - Read/write to the PTY manually using file handles
    - Check process status
    - Control process termination with specific signals

    For interactive shell sessions, consider using `PtySession` instead, which
    provides higher-level conveniences like `send_line()` and `interact()`.
    """

    _inner: PyPtyProcess

    def __init__(self, command: list[str], options: Optional[PtyProcessOptions] = None) -> None:
        """
        Create a new PTY process with the given command.

        Arguments:
            command: A list of strings representing the command and its arguments.
                     The first element is the executable, subsequent elements are arguments.
            options: Optional PtyProcessOptions to configure the PTY behavior.
                     If not provided, defaults to echo=True.

        Raises:
            RuntimeError: If the PTY process could not be created.

        Examples
        --------
        ```python
        >>> from rattler.pty import PtyProcess, PtyProcessOptions
        >>> # Create with default options (echo enabled)
        >>> process = PtyProcess(["bash"])
        >>> # Create with custom options
        >>> opts = PtyProcessOptions(echo=False)
        >>> process = PtyProcess(["bash", "-l"], opts)
        >>> # Check process status
        >>> status = process.status()
        >>> print(status)
        StillAlive
        >>>
        ```
        """
        if not _PTY_AVAILABLE:
            raise ImportError("PTY functionality is not available on this platform")
        if options is None:
            self._inner = PyPtyProcess(command)
        else:
            self._inner = PyPtyProcess(command, options._inner)

    @classmethod
    def _from_py_pty_process(cls, py_pty_process: PyPtyProcess) -> PtyProcess:
        """Construct from FFI PyPtyProcess object."""
        process = cls.__new__(cls)
        process._inner = py_pty_process
        return process

    @property
    def child_pid(self) -> int:
        """
        Get the process ID (PID) of the child process.

        Returns:
            The PID as an integer.

        Examples
        --------
        ```python
        >>> process = PtyProcess(["bash"])
        >>> pid = process.child_pid
        >>> print(f"Process ID: {pid}")
        Process ID: 12345
        >>>
        ```
        """
        return self._inner.child_pid

    def status(self) -> Optional[str]:
        """
        Check the status of the child process (non-blocking).

        This runs waitpid() with WNOHANG, so it returns immediately.
        Note: If you previously called exit() or status() returned an exit status,
        subsequent calls may return None.

        Returns:
            A string representing the process status, or None if status cannot be determined.
            Possible values:
            - "StillAlive": Process is still running
            - "Exited(code)": Process exited with the given exit code
            - "Signaled(signal)": Process was terminated by a signal
            - "Stopped": Process was stopped

        Examples
        --------
        ```python
        >>> import time
        >>> process = PtyProcess(["sleep", "10"])
        >>> print(process.status())
        StillAlive
        >>> time.sleep(11)
        >>> print(process.status())
        Exited(0)
        >>>
        ```
        """
        return self._inner.status()

    def exit(self) -> str:
        """
        Exit the process gracefully by sending SIGTERM.

        This method blocks until the process has exited. If the process doesn't
        respond to SIGTERM, it will eventually be killed with SIGKILL if a
        kill_timeout was set (not currently exposed to Python).

        Returns:
            A string describing the exit status.

        Raises:
            RuntimeError: If the process could not be terminated.

        Examples
        --------
        ```python
        >>> process = PtyProcess(["bash"])
        >>> status = process.exit()
        >>> print(status)
        Exited(0)
        >>>
        ```
        """
        return self._inner.exit()

    def get_file_handle(self) -> BinaryIO:
        """
        Get a file handle for reading from and writing to the PTY.

        This returns a Python file-like object that can be used to read output
        from the process and write input to it. This is useful for non-interactive
        automation where you want to programmatically read the process output.

        Returns:
            A Python binary file object for reading/writing to the PTY.

        Raises:
            RuntimeError: If the file handle could not be created.

        Examples
        --------
        ```python
        >>> process = PtyProcess(["bash"])
        >>> file = process.get_file_handle()
        >>> # Write to the process
        >>> file.write(b"echo hello\\n")
        >>> file.flush()
        >>> # Read output (this is a blocking operation)
        >>> output = file.read(100)
        >>> print(output)
        >>>
        ```
        """
        fd = self._inner.get_file_handle()
        return os.fdopen(fd, "r+b", buffering=0)

    def __repr__(self) -> str:
        return f"PtyProcess(child_pid={self.child_pid})"
