# Environments

Commands that operate on a conda prefix (an installed environment).

The default prefix is `.prefix` in the current working directory. Pass `-p/--prefix` (alias `--target-prefix`) to override.

---

## `rattler create` — solve and install

Resolves the given match specs against the selected channels and installs the resulting packages into the target prefix. Reads packages already in the prefix and treats them as locked during solving, so running `create` repeatedly behaves like an update.

```
rattler create [OPTIONS] <SPECS>...
```

| Option | Description |
|--------|-------------|
| `<SPECS>...` | One or more match specs (e.g. `python=3.12`, `numpy>=1.26`). |
| `-c`, `--channel <CHANNEL>` | Channel to search (repeatable). Default: `conda-forge`. |
| `-p`, `--prefix <PATH>` | Target prefix. Default: `.prefix`. Alias: `--target-prefix`. |
| `--platform <PLATFORM>` | Target platform (e.g. `linux-64`, `osx-arm64`). Default: current. |
| `--virtual-package <NAME[=VERSION[=BUILD]]>` | Override detected virtual packages (repeatable). |
| `--solver <resolvo\|libsolv>` | SAT solver backend. Default: `resolvo`. |
| `--strategy <highest\|lowest\|lowest-direct>` | Version selection strategy. |
| `--timeout <MS>` | Abort the solver after this many milliseconds. |
| `--exclude-newer <TIMESTAMP>` | Ignore packages published after the timestamp / date (e.g. `2024-01-15` or `2024-01-15T00:00:00Z`). |
| `--only-deps` | Install dependencies only, not the specs themselves. |
| `--no-deps` | Install the specs only, without dependencies. |
| `--dry-run` | Print the planned transaction without installing. |

**Example:**

```bash
rattler create -c conda-forge -p ./env "python=3.12" "numpy>=1.26"
rattler create --dry-run --platform linux-64 -c conda-forge "ripgrep"
```

---

## `rattler run` — execute in an activated prefix

Activates the prefix and runs a command in that environment.

```
rattler run [OPTIONS] <COMMAND>...
```

| Option | Description |
|--------|-------------|
| `<COMMAND>...` | Program and arguments. Use `--` to pass flags to the child. |
| `-p`, `--prefix <PATH>` | Prefix to activate. Default: `.prefix`. |
| `--cwd <PATH>` | Working directory for the child process. |

The activation shell is detected from the environment (`ShellEnum::from_env`). The child's exit code is propagated.

**Example:**

```bash
rattler run -p ./env python -c "import sys; print(sys.prefix)"
```

---

## `rattler list` — list installed packages

Prints the packages present in a prefix (name, version, build, channel).

```
rattler list [OPTIONS] [NAME]
```

| Option | Description |
|--------|-------------|
| `[NAME]` | Optional substring (or exact name with `-f`) to filter by. |
| `-p`, `--prefix <PATH>` | Prefix to inspect. Falls back to `$CONDA_PREFIX` if unset. |
| `-f`, `--full-name` | Require an exact name match rather than a substring. |

If `[NAME]` is given and nothing matches, the command exits non-zero.

**Example:**

```bash
rattler list -p ./env
rattler list -p ./env numpy
```

---

## `rattler shell-hook` — print activation script

Prints the activation script for a prefix to stdout. Use it in shell init files or to activate a prefix in a script.

```
rattler shell-hook [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `-p`, `--prefix <PATH>` | Prefix to activate. Default: `.prefix`. |
| `-s`, `--shell <SHELL>` | `bash`, `zsh`, `fish`, `xonsh`, `cmd`, `nushell`, or `powershell`. Defaults to the detected shell. |

**Example:**

```bash
eval "$(rattler shell-hook -p ./env -s bash)"
rattler shell-hook -p ./env -s fish | source
```

---

## `rattler install-menu` / `remove-menu`

Install or remove OS-native menu entries (Start menu, Launchpad, etc.) declared by a package via menuinst.

```
rattler install-menu [-t <PREFIX>] <PACKAGE_NAME>
rattler remove-menu  [-t <PREFIX>] <PACKAGE_NAME>
```

| Option | Description |
|--------|-------------|
| `<PACKAGE_NAME>` | Name of an already-installed package in the prefix. |
| `-t`, `--target-prefix <PATH>` | Prefix to look in. Default: `.prefix`. |

`install-menu` fails if the package isn't present in the prefix. `remove-menu` uses the record's `installed_system_menus` to clean up entries.
