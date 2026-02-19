# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

## Project Overview

Rattler is a Rust library for conda package management — installing environments, solving dependencies, fetching repodata, streaming packages — without requiring Python. It is used by pixi, rattler-build, and prefix.dev.

## Build & Development Commands

Common commands:
```sh
pixi run build                # cargo build
pixi run test                 # cargo nextest run (full workspace, see pixi.toml for feature flags)
pixi run check                # cargo check
pixi run doc                  # build docs with warnings as errors
pixi run lint                 # run all linters (cargo fmt, clippy, ruff, dprint, typos, actionlint)
pixi run lint-fast            # fast linters only (no clippy)
pixi run lint-slow            # slow linters only (clippy + native-tls check)
pixi run cargo-clippy         # clippy with -D warnings -Dclippy::dbg_macro
pixi run cargo-fmt            # rustfmt
```

Running a single test:
```sh
pixi run -- cargo nextest run -p <crate_name> <test_name>
```

The default test features used in CI: `indicatif,tokio,serde,reqwest,sparse,gateway,resolvo,libsolv_c,s3,edit,rattler_config`

Python bindings (py-rattler):
```sh
cd py-rattler
pixi run fmt                  # format Python code
pixi run test                 # run Python tests
```

## Code Quality

The project enforces strict clippy lints via `.cargo/config.toml` (70+ rules). Key ones: no `todo!()`, no `dbg!()`, no wildcard imports, no `enum_glob_use`, uninlined format args warned.

Always run `pixi run cargo-fmt` and `pixi run cargo-clippy` after doing changes.

## Workspace Architecture

Monorepo with 25+ crates in `crates/`, Python bindings in `py-rattler/`, and WASM/JS bindings in `js-rattler/`.

### Core crate dependency layers (bottom to top):

**Foundation:** `rattler_conda_types` (all conda data types: MatchSpec, Version, PackageRecord, RepoData, etc.), `rattler_digest` (hashing), `rattler_macros` (proc macros)

**Package I/O:** `rattler_package_streaming` (extract/create .conda and .tar.bz2 packages), `rattler_repodata_gateway` (fetch, cache, and merge repodata from channels — supports JLAP, sharded repodata, and OCI mirrors)

**Solving:** `rattler_solve` (SAT solver interface with two backends: `resolvo` and `libsolv_c`), `rattler_libsolv_c` (C bindings to libsolv)

**Environment management:** `rattler` (top-level crate — orchestrates install/create/remove operations, link/unlink packages), `rattler_shell` (activation scripts for bash/zsh/fish/cmd/powershell/nushell), `rattler_virtual_packages` (detect system capabilities like CUDA, glibc, etc.)

**Networking & Auth:** `rattler_networking` (authentication middleware, mirror handling, retry logic, GCS/S3/OCI support), `rattler_s3` (S3 storage backend), `rattler_config` (channel config, authentication storage)

**Other:** `rattler_lock` (conda-lock format), `rattler_index` (build local channels), `rattler_cache` (package cache management), `rattler_menuinst` (shortcut/menu entries), `rattler_sandbox` (sandboxed process execution), `rattler_upload` (upload to quetz/artifactory/prefix.dev)

### Python bindings (py-rattler)
PyO3-based, built with maturin. Async Rust functions are exposed via tokio runtime. Located in `py-rattler/` with Rust source in `py-rattler/src/` and Python wrapper modules in `py-rattler/python/rattler/`.

### JS/WASM bindings (js-rattler)
wasm-bindgen based, primarily exposes version comparison for use in mambajs. Located in `js-rattler/`.
