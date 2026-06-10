# PTY Support (Unix Only)

Interactive pseudoterminal sessions. **Only available on Unix** (Linux, macOS). Raises `ImportError` on Windows.

```python
from rattler import PtyProcess, PtySession, PtyProcessOptions  # ImportError on Windows
```

## PtyProcess

Low-level PTY API for spawning and controlling a process in a pseudoterminal.

### Constructor

```python
PtyProcess(
    command: list[str],
    options: PtyProcessOptions | None = None,
)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `command` | `list[str]` | required | Command and arguments to execute |
| `options` | `PtyProcessOptions \| None` | `None` | PTY options (defaults to `PtyProcessOptions(echo=True)`) |

Raises `RuntimeError` if the PTY could not be created.

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `child_pid` | `int` | Process ID of the child process |
| `kill_timeout` | `float \| None` | Timeout in seconds before SIGKILL after SIGTERM (get/set) |

### Synchronous Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `status` | `status() -> str \| None` | Non-blocking status check. Returns `"StillAlive"`, `"Exited(N)"`, `"Signaled(N)"`, `"Stopped"`, or `None` |
| `exit` | `exit() -> str` | Graceful exit with SIGTERM (blocking). Returns status string |
| `get_file_handle` | `get_file_handle() -> BinaryIO` | Get a binary file handle for direct read/write |

### Async Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `async_read` | `async_read(size: int = 8192) -> bytes` | Read from PTY asynchronously |
| `async_write` | `async_write(data: bytes) -> int` | Write to PTY asynchronously. Returns bytes written |
| `async_wait` | `async_wait() -> str` | Wait for process to exit asynchronously. Returns status string |
| `async_exit` | `async_exit() -> str` | Graceful async exit with SIGTERM. Returns status string |

**Example:**

```python
from rattler import PtyProcess

proc = PtyProcess(["python3", "-c", "print('hello')"])
output = await proc.async_read()
print(output.decode())
status = await proc.async_wait()
print(status)  # "Exited(0)"
```

---

## PtySession

Higher-level PTY API for interactive shell sessions. Provides line-oriented I/O and interactive mode.

### Constructor

```python
PtySession(command: list[str])
```

Raises `RuntimeError` if the session could not be created.

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `send_line` | `send_line(line: str) -> int` | Send string + newline. Returns bytes written |
| `send` | `send(data: str) -> int` | Send string without newline. Returns bytes written |
| `flush` | `flush() -> None` | Flush pending output |
| `exit` | `exit() -> str` | Graceful exit with SIGTERM. Returns status string |
| `interact` | `interact(wait_until: str \| None = None) -> int \| None` | Start interactive mode. Returns exit code or `None` if terminated by signal |

The `interact()` method sets the terminal to raw mode, forwards I/O between user and PTY. If `wait_until` is specified, buffers output until that pattern appears (1 second timeout).

**Example:**

```python
from rattler import PtySession

session = PtySession(["/bin/bash"])
session.send_line("echo hello")
session.send_line("exit")
exit_code = session.interact(wait_until="hello")
```

---

## PtyProcessOptions

Configuration for PTY process creation.

```python
PtyProcessOptions(echo: bool = True)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `echo` | `bool` | `True` | Whether input is echoed back to the terminal |
