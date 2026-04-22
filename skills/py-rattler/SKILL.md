---
name: py-rattler
description: >-
  Python bindings for the rattler conda library. Use when solving dependencies,
  installing packages, managing conda environments, working with repodata, lock
  files, or shell activation programmatically from Python.
license: BSD-3-Clause
---

# py-rattler

py-rattler provides async Python bindings for the [rattler](https://github.com/conda/rattler) Rust library for working with the conda ecosystem. It powers tools like [pixi](https://pixi.sh) and [rattler-build](https://rattler.build).

Install: `pixi add py-rattler` (preferred) or `pip install py-rattler`.

## When to Use This Skill

Use this skill when writing Python code that programmatically interacts with the conda ecosystem: solving package dependencies, installing packages into environments, reading or creating lock files, activating environments, managing repodata, or working with conda package archives.

## API Reference

Determine which reference applies based on the task:

| Task | Reference |
|------|-----------|
| Solve package dependencies | [references/solving-installing.md](references/solving-installing.md) |
| Install packages into an environment | [references/solving-installing.md](references/solving-installing.md) |
| Parse or match package specs | [references/match-spec.md](references/match-spec.md) |
| Work with version strings | [references/version.md](references/version.md) |
| Configure channels and platforms | [references/channels-platforms.md](references/channels-platforms.md) |
| Detect or override virtual packages | [references/virtual-packages.md](references/virtual-packages.md) |
| Build HTTP clients with middleware | [references/networking.md](references/networking.md) |
| Query repodata with Gateway | [references/gateway-repodata.md](references/gateway-repodata.md) |
| Work with package records | [references/package-records.md](references/package-records.md) |
| Inspect package metadata (index.json, about.json, paths.json) | [references/package-metadata.md](references/package-metadata.md) |
| Read or write lock files | [references/lock-files.md](references/lock-files.md) |
| Generate shell activation scripts | [references/shell-activation.md](references/shell-activation.md) |
| Download and extract conda packages | [references/package-streaming.md](references/package-streaming.md) |
| Index conda channels (filesystem or S3) | [references/indexing.md](references/indexing.md) |
| Inspect installed packages in a prefix | [references/prefix-records.md](references/prefix-records.md) |
| Run interactive PTY sessions (Unix only) | [references/pty.md](references/pty.md) |
| Handle errors | [references/exceptions.md](references/exceptions.md) |

---

## Quick Start

All I/O operations are **async**. The standard pattern is: solve specs, then install records.

```python
import asyncio
from rattler import solve, install, VirtualPackage

async def main():
    records = await solve(
        sources=["conda-forge"],
        specs=["python 3.12.*", "numpy >=1.26"],
        virtual_packages=VirtualPackage.detect(),
    )
    await install(records, target_prefix="/path/to/env")

asyncio.run(main())
```

## Import Reference

```python
from rattler import (
    # Solving & installing
    solve, solve_with_sparse_repodata, install, InstallerReporter,
    # Specifications
    MatchSpec, NamelessMatchSpec, Version, VersionSpec, VersionWithSource,
    # Channels & repodata
    Channel, ChannelConfig, ChannelPriority,
    Gateway, SourceConfig, RepoData, SparseRepoData,
    ChannelInfo, ChannelRelations,
    # Package records
    PackageRecord, RepoDataRecord, WhlPackageRecord, PatchInstructions,
    # Platform & virtual packages
    Platform, VirtualPackage, GenericVirtualPackage, VirtualPackageOverrides, Override,
    # Networking
    Client,
    # Lock files
    LockFile, Environment, LockChannel, PackageHashes,
    LockedPackage, CondaLockedBinaryPackage, CondaLockedSourcePackage, PypiLockedPackage,
    # Package metadata
    PackageName, IndexJson, AboutJson, PathsJson, PathsEntry, RunExportsJson,
    PathType, PrefixPlaceholder, FileMode, NoArchType,
    # Prefix records
    PrefixRecord, PrefixPaths, PrefixPathsEntry, PrefixPathType, Link, LinkType,
    # Shell activation
    activate, Shell, ActivationVariables, ActivationResult, PathModificationBehavior,
    # Package streaming
    extract, extract_tar_bz2,
    # PTY (Unix only)
    PtySession, PtyProcess, PtyProcessOptions,
)
from rattler.networking import (
    fetch_repo_data, FetchRepoDataOptions,
    RetryMiddleware, AuthenticationMiddleware, MirrorMiddleware,
    AddHeadersMiddleware, OciMiddleware, S3Middleware, GCSMiddleware,
)
from rattler.repo_data import RepoDataSource, PackageFormatSelection
from rattler.index import index_fs, index_s3, S3Credentials
```

## Key Design Principles

- **Async-first**: all I/O operations are `async` — use `asyncio.run()` or `await`
- **Thread-safe**: `Gateway` and `Client` can be shared across threads
- **Immutable**: most objects are logically immutable after construction
- **Rust-backed**: computation-heavy operations are delegated to Rust via PyO3
- **Singleton caching**: `Platform` instances are cached (same string returns same object)
