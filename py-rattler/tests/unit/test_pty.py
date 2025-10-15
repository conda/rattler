import sys
import time
import select
import pytest

from rattler import PtySession, PtyProcess, PtyProcessOptions

# PTY functionality is only available on Unix platforms
skip_on_windows = pytest.mark.skipif(sys.platform == "win32", reason="PTY is Unix-only")


@skip_on_windows
def test_pty_process_options() -> None:
    """Test PtyProcessOptions creation and properties."""
    opts = PtyProcessOptions()
    assert opts.echo is True

    opts_no_echo = PtyProcessOptions(echo=False)
    assert opts_no_echo.echo is False

    assert "PtyProcessOptions" in repr(opts)
    assert "echo=True" in repr(opts)


@skip_on_windows
def test_pty_session_creation() -> None:
    """Test PtySession can be created with a simple command."""
    session = PtySession(["true"])
    assert session is not None
    assert "PtySession" in repr(session)


@skip_on_windows
def test_pty_session_closed_pty_error() -> None:
    """Test that sending to a closed PTY raises an error or succeeds silently."""
    # Use bash -c with exit so it exits immediately
    session = PtySession(["bash", "-c", "exit 0"])

    # Give bash time to exit
    time.sleep(0.2)

    # On macOS, writing to a closed PTY raises an error immediately
    # On Linux, it may succeed or fail depending on timing and buffering
    # We just verify that the operation completes without hanging
    try:
        session.send_line("test")
    except RuntimeError:
        pass

    # Note: We don't test interact() as it requires a real terminal
    # See tests/functional_pty_session.py for manual testing of interact()


@skip_on_windows
def test_pty_process_creation() -> None:
    """Test PtyProcess can be created with options."""
    # Create with default options
    process = PtyProcess(["true"])
    assert process is not None
    assert process.child_pid > 0
    assert "PtyProcess" in repr(process)

    # Wait for it to exit
    time.sleep(0.1)

    # Create with custom options
    opts = PtyProcessOptions(echo=False)
    process_no_echo = PtyProcess(["true"], opts)
    assert process_no_echo is not None
    assert process_no_echo.child_pid > 0


@skip_on_windows
def test_pty_process_run_command_with_args() -> None:
    """Test running a command with arguments like subprocess."""
    # Run echo with arguments
    process = PtyProcess(["echo", "hello", "world"])
    assert process.child_pid > 0

    # Get file handle and read output
    file = process.get_file_handle()

    # Use select to avoid blocking
    fd = file.fileno()
    readable, _, _ = select.select([fd], [], [], 1.0)

    if readable:
        output = file.read(100)
        assert b"hello world" in output

    # Wait for process to exit
    time.sleep(0.1)
    status = process.status()
    # Should have exited
    if status:
        assert "Exited" in status


@skip_on_windows
def test_pty_process_bash_command() -> None:
    """Test running bash -c with a command."""
    # Run a command via bash -c
    process = PtyProcess(["bash", "-c", "echo 'test output' && exit 0"])

    # Get file handle and read output
    file = process.get_file_handle()

    # Use select to read with timeout
    fd = file.fileno()
    readable, _, _ = select.select([fd], [], [], 1.0)

    if readable:
        output = file.read(100)
        assert b"test output" in output

    # Wait for bash to exit
    time.sleep(0.2)
    status = process.status()
    # Should have exited with code 0
    if status:
        assert "Exited(0)" in status


@skip_on_windows
def test_pty_process_status() -> None:
    """Test checking process status."""
    # Start a very short process
    process = PtyProcess(["sleep", "0.01"])

    # Check initial status - might still be alive
    status = process.status()
    assert status is None or "Exited" in status or "StillAlive" in status

    # Wait for it to finish
    time.sleep(0.05)

    # Should be exited now
    status = process.status()
    # Note: status might be None if we already reaped the process
    if status is not None:
        assert "Exited" in status or "Signaled" in status


@skip_on_windows
def test_pty_process_exit() -> None:
    """Test gracefully exiting a process."""
    # Start a process that would run for a while
    process = PtyProcess(["sleep", "100"])

    # Exit the process immediately
    exit_status = process.exit()
    assert exit_status is not None
    # SIGTERM will cause it to be signaled, not exited normally
    assert "Exited" in exit_status or "Signaled" in exit_status


@skip_on_windows
def test_pty_session_empty_command() -> None:
    """Test that empty command raises an error."""
    with pytest.raises(Exception):
        PtySession([])


@skip_on_windows
def test_pty_process_empty_command() -> None:
    """Test that empty command raises an error."""
    with pytest.raises(Exception):
        PtyProcess([])


@skip_on_windows
def test_pty_child_pid_property() -> None:
    """Test that child_pid property returns a valid PID."""
    process = PtyProcess(["sleep", "0.01"])
    pid = process.child_pid
    assert isinstance(pid, int)
    assert pid > 0

    # Clean up
    time.sleep(0.05)


@skip_on_windows
def test_pty_process_get_file_handle() -> None:
    """Test reading and writing using get_file_handle()."""
    # Start a simple echo process
    process = PtyProcess(["cat"])

    # Get the file handle
    file = process.get_file_handle()

    # Verify it's a file object
    assert hasattr(file, "write")
    assert hasattr(file, "read")
    assert hasattr(file, "fileno")

    # Write some data
    bytes_written = file.write(b"hello\n")
    assert bytes_written > 0

    # Read back the echoed output (cat echoes input)
    # Use a small timeout to avoid hanging
    fd = file.fileno()
    readable, _, _ = select.select([fd], [], [], 0.5)

    if readable:
        output = file.read(100)
        assert b"hello" in output

    # Clean up
    process.exit()


# Async tests
@skip_on_windows
@pytest.mark.asyncio
async def test_pty_process_async_read_write() -> None:
    """Test async read and write operations."""
    import asyncio

    process = PtyProcess(["bash", "-c", "echo 'hello async world'"])

    # Read the output asynchronously with timeout
    data = await asyncio.wait_for(process.async_read(1024), timeout=2.0)
    assert b"hello async world" in data

    # Wait for process to exit with timeout
    status = await asyncio.wait_for(process.async_wait(), timeout=2.0)
    assert "Exited(0)" in status


@skip_on_windows
@pytest.mark.asyncio
async def test_pty_process_async_wait() -> None:
    """Test async waiting for process to exit."""
    import asyncio

    process = PtyProcess(["sleep", "0.1"])

    # Check it's alive
    status = process.status()
    assert status is None or "StillAlive" in status

    # Wait for it to finish asynchronously with timeout
    exit_status = await asyncio.wait_for(process.async_wait(), timeout=2.0)
    assert "Exited(0)" in exit_status


@skip_on_windows
@pytest.mark.asyncio
async def test_pty_process_async_exit() -> None:
    """Test async process termination."""
    import asyncio

    process = PtyProcess(["sleep", "100"])

    # Exit the process asynchronously with timeout
    status = await asyncio.wait_for(process.async_exit(), timeout=2.0)
    assert "Exited" in status or "Signaled" in status

    # Verify it's no longer alive
    final_status = process.status()
    if final_status:
        assert "StillAlive" not in final_status


@skip_on_windows
@pytest.mark.asyncio
async def test_pty_process_multiple_async_operations() -> None:
    """Test multiple concurrent async operations."""
    import asyncio

    # Create multiple processes
    processes = [PtyProcess(["bash", "-c", f"echo 'process {i}'"]) for i in range(3)]

    # Read from all concurrently with timeout
    results = await asyncio.wait_for(asyncio.gather(*[proc.async_read(1024) for proc in processes]), timeout=2.0)

    # Verify each got its output
    for i, data in enumerate(results):
        assert f"process {i}".encode() in data

    # Wait for all to exit with timeout
    statuses = await asyncio.wait_for(asyncio.gather(*[proc.async_wait() for proc in processes]), timeout=2.0)

    assert all("Exited(0)" in status for status in statuses)
