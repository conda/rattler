---
name: rattler
description: Solve, create, inspect and manipulate conda environments and packages with the `rattler` CLI. Use when the user mentions rattler, conda packages (.conda or .tar.bz2 archives), conda channels or repodata, or wants to solve, install, list or inspect a conda environment without pixi or conda.
---

# rattler CLI

`rattler` (version {version}) is a small command-line frontend for the rattler
Rust crates that pixi, rattler-build and prefix.dev are built on. It talks to
plain conda channels and prefixes, has no project file, and never modifies your
shell configuration. Reach for it for one-off operations: solve a set of specs,
create a throwaway prefix, inspect or extract a package, look up reverse
dependencies, or run a tool in a temporary environment. For project-based
workflows use pixi instead.

## Conventions

- Run `rattler <command> --help` for the complete and authoritative list of
  flags. This file is a map, not the manual.
- Package specs use conda matchspec syntax: `python`, `numpy>=2`,
  `pytorch=2.*=*cuda*`, `conda-forge::xtensor`.
- Channels are passed with `-c`/`--channel` and may be repeated. Commands that
  query channels default to `conda-forge` when no channel is given.
- The prefix (environment directory) is passed with `-p`/`--prefix` and defaults
  to `.prefix` in the current directory. `list` and `run` also honour the
  `CONDA_PREFIX` environment variable, so they work inside an activated
  environment.
- Machine-readable output: `--format json` or `--format urls` on `solve`,
  `search`, `list` and `whoneeds`; `--json` on `info` and `inspect`. Progress
  bars and logs go to stderr, results go to stdout, so piping stdout is safe.
- `--offline` (global) disables all network access and only uses cached
  repodata and packages. `-v`/`--verbose` (global) enables debug logging;
  `RUST_LOG` gives finer control.
- Solving for another target: `--platform linux-64` selects the platform,
  `--virtual-package __glibc=2.28` overrides detected virtual packages.
  `rattler virtual-packages` prints what is detected on this machine.
- Reproducible solves: `--exclude-newer 2025-01-01` ignores packages published
  after a date, `--strategy lowest-direct` picks the oldest allowed versions of
  the requested packages.
- Private channels: `rattler auth login <host> --token <token>` stores
  credentials that every other command then reuses. Check with
  `rattler auth status`.

## Typical workflows

Solve, install and run:

```bash
rattler solve python numpy -c conda-forge --format json > solved.json
rattler create -c conda-forge -p ./env python numpy
rattler run -p ./env python -c "import numpy; print(numpy.__version__)"
```

Run a tool once without keeping an environment (the package is guessed from
the command name, use `--spec` to override):

```bash
rattler exec ruff check .
rattler exec --spec python=3.12 python --version
```

Look inside a package without installing it:

```bash
rattler inspect https://conda.anaconda.org/conda-forge/noarch/tzdata-2024a-h0c530f3_0.conda --json
rattler fetch-file ./numpy-2.1.0-py312h1234_0.conda info/index.json
rattler extract ./numpy-2.1.0-py312h1234_0.conda --destination ./numpy-src
rattler compare-packages ./old.conda ./new.conda
```

Understand a channel:

```bash
rattler search 'numpy*' -c conda-forge --platform osx-arm64
rattler whoneeds openssl --format urls | head
```
