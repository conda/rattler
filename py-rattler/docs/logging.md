# logging

Py-rattler does not enable Rust logging when the package is imported. To forward
Rust `tracing` events, such as `tracing::info!` and `tracing::debug!`, to
Python's `logging` module, call `setup_logging()` once during application
startup.

```python
import logging

import rattler

logging.basicConfig(level=logging.DEBUG)
rattler.setup_logging()
```

Rust tracing targets are exposed below the `rattler` logger namespace. For
example, package streaming logs are emitted as `rattler.rattler_package_streaming`.

```python
logging.getLogger("rattler.rattler_package_streaming").setLevel(logging.DEBUG)
```

`setup_logging()` installs a process-wide Rust logger. Calling it more than once,
or calling it after another Rust logger has already been installed, raises a
`RuntimeError`.

::: rattler.setup_logging
