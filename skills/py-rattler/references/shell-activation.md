# Shell Activation

Generate shell scripts to activate and deactivate conda environments.

## activate()

```python
def activate(
    prefix: Path,
    activation_variables: ActivationVariables,
    shell: Shell | None = None,
    platform: Platform | PlatformLiteral | None = None,
) -> ActivationResult
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `prefix` | `Path` | required | Path to the conda environment to activate |
| `activation_variables` | `ActivationVariables` | required | Current environment state |
| `shell` | `Shell \| None` | `None` | Target shell (defaults to `bash`) |
| `platform` | `Platform \| PlatformLiteral \| None` | `None` | Target platform (defaults to current) |

**Returns:** `ActivationResult`

**Example:**

```python
from rattler.shell import Shell, activate, ActivationVariables, PathModificationBehavior
from pathlib import Path
import os

activation_vars = ActivationVariables(
    current_prefix=os.environ.get("CONDA_PREFIX"),
    current_path=os.environ.get("PATH", "").split(os.pathsep),
    path_modification_behavior=PathModificationBehavior.Prepend,
)

result = activate(
    Path("/opt/envs/myenv"),
    activation_vars,
    shell=Shell.bash,
)

print(result.script)  # Shell code to source/eval
print(result.path)    # New PATH value
```

---

## Shell

Enum of supported shells.

| Value | Shell |
|-------|-------|
| `Shell.bash` | Bash |
| `Shell.zsh` | Zsh |
| `Shell.fish` | Fish |
| `Shell.xonsh` | Xonsh |
| `Shell.powershell` | PowerShell |
| `Shell.cmd_exe` | Windows Command Prompt |

---

## ActivationVariables

Describes the current environment state before activation.

### Constructor

```python
ActivationVariables(
    current_prefix: os.PathLike[str] | None = None,
    current_path: Iterable[str] | Iterable[os.PathLike[str]] | None = None,
    path_modification_behavior: PathModificationBehavior = PathModificationBehavior.Prepend,
)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `current_prefix` | `os.PathLike[str] \| None` | `None` | Currently active conda prefix (e.g., `os.environ["CONDA_PREFIX"]`) |
| `current_path` | `Iterable[str] \| None` | `None` | Current PATH entries (e.g., `os.environ["PATH"].split(os.pathsep)`) |
| `path_modification_behavior` | `PathModificationBehavior` | `Prepend` | How to modify PATH |

---

## PathModificationBehavior

Enum controlling how the PATH environment variable is modified.

| Value | Description |
|-------|-------------|
| `PathModificationBehavior.Prepend` | Add environment paths to the beginning of PATH |
| `PathModificationBehavior.Append` | Add environment paths to the end of PATH |
| `PathModificationBehavior.Replace` | Replace PATH entirely with environment paths |

---

## ActivationResult

Result of activating a conda environment.

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `script` | `str` | Shell code to execute for activation (source/eval this) |
| `path` | `Path` | The new PATH value after activation |
