---
name: rattler-bin
description: >-
  Use the rattler CLI (the rattler-bin Rust binary) to interact with the conda
  ecosystem: create environments, search packages, inspect and extract conda
  archives, upload to channels, and authenticate with private servers. Use when
  the user mentions the `rattler` command-line tool.
license: BSD-3-Clause
---

# rattler CLI

`rattler` is a Rust binary for common conda operations — solving and installing environments, searching channels, inspecting packages, uploading to registries, and managing authentication. It is the reference CLI built on top of the [rattler](https://github.com/conda/rattler) Rust libraries that also power [pixi](https://pixi.sh) and [rattler-build](https://rattler.build).

This skill documents the `rattler` CLI.

## When to Use This Skill

Use when the user asks to run or script the `rattler` command-line tool: creating conda environments, searching channels, inspecting remote `.conda` archives, extracting or linking packages, generating shell hooks, uploading built packages, or authenticating with prefix.dev / anaconda.org / Quetz / Artifactory / Cloudsmith / S3.

## Command Index

| Command | Purpose | Reference |
|---------|---------|-----------|
| `create` | Solve and install packages into a prefix | [references/environments.md](references/environments.md) |
| `run` | Run a command in an activated prefix | [references/environments.md](references/environments.md) |
| `list` | List packages installed in a prefix | [references/environments.md](references/environments.md) |
| `shell-hook` | Print shell activation script for a prefix | [references/environments.md](references/environments.md) |
| `install-menu` / `remove-menu` | Manage menuinst entries for installed packages | [references/environments.md](references/environments.md) |
| `search` | Search channels for packages (glob / regex) | [references/packages.md](references/packages.md) |
| `inspect` | Print metadata of a remote `.conda` package | [references/packages.md](references/packages.md) |
| `fetch-file` | Read a single file from inside a remote package | [references/packages.md](references/packages.md) |
| `download` | Download an arbitrary file (auth-aware) | [references/packages.md](references/packages.md) |
| `extract` | Extract a local or remote conda archive | [references/packages.md](references/packages.md) |
| `link` | Link an already-extracted package into a prefix | [references/packages.md](references/packages.md) |
| `upload` | Upload built packages to a registry | [references/upload.md](references/upload.md) |
| `auth login` / `auth logout` | Store or remove credentials for a host | [references/auth.md](references/auth.md) |
| `virtual-packages` | Print detected virtual packages | [references/misc.md](references/misc.md) |
| `completion` | Generate shell completion script | [references/misc.md](references/misc.md) |

## Install

`rattler` is distributed as the `rattler-bin` conda package:

```bash
pixi global install rattler
# or
pixi add rattler
```

It is also published on crates.io as `rattler-bin` (the binary is named `rattler`).

## Global Options

| Option | Description |
|--------|-------------|
| `-v`, `--verbose` | Enable debug-level logging (otherwise info-level). `RUST_LOG` is honored. |
| `-h`, `--help` | Print help for `rattler` or any subcommand. |
| `-V`, `--version` | Print version. |

## Defaults Worth Knowing

- **Default target prefix**: `.prefix` in the current working directory — used by `create`, `run`, `shell-hook`, `install-menu`, `remove-menu`. Override with `-p/--prefix` (alias `--target-prefix`).
- **Default channel for `search`**: `conda-forge`. `create` also defaults to `conda-forge` when `-c` is omitted.
- **Default platform for `search`**: the current platform. `create` also defaults to the current platform unless `--platform` is given.
- **Auth storage**: credentials are read from the system keychain or the rattler auth file; most `upload` subcommands also accept the token via environment variables (e.g. `PREFIX_API_KEY`, `ANACONDA_API_KEY`).

## Quick Start

```bash
# Create an environment
rattler create -c conda-forge -p ./env "python=3.12" "numpy>=1.26"

# Run something inside it
rattler run -p ./env python -c "import numpy; print(numpy.__version__)"

# Search conda-forge
rattler search 'polars*'

# Inspect a remote package
rattler inspect https://conda.anaconda.org/conda-forge/noarch/tqdm-4.66.5-pyhd8ed1ab_0.conda

# Upload a built package to prefix.dev
rattler auth login prefix.dev --token pfx_...
rattler upload prefix -c my-channel ./output/noarch/mypkg-1.0-py_0.conda
```
