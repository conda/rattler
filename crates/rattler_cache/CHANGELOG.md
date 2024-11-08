# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.8](https://github.com/conda/rattler/compare/rattler_cache-v0.2.7...rattler_cache-v0.2.8) - 2024-11-04

### Other

- update Cargo.toml dependencies

## [0.2.7](https://github.com/conda/rattler/compare/rattler_cache-v0.2.6...rattler_cache-v0.2.7) - 2024-10-21

### Added

- use sha in cache lock file, needed for source builds. ([#901](https://github.com/conda/rattler/pull/901))

## [0.2.6](https://github.com/conda/rattler/compare/rattler_cache-v0.2.5...rattler_cache-v0.2.6) - 2024-10-07

### Other

- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.2.5](https://github.com/conda/rattler/compare/rattler_cache-v0.2.4...rattler_cache-v0.2.5) - 2024-10-03

### Other

- updated the following local packages: rattler_conda_types

## [0.2.4](https://github.com/conda/rattler/compare/rattler_cache-v0.2.3...rattler_cache-v0.2.4) - 2024-09-23

### Other

- updated the following local packages: rattler_conda_types

## [0.2.3](https://github.com/conda/rattler/compare/rattler_cache-v0.2.2...rattler_cache-v0.2.3) - 2024-09-09

### Other

- updated the following local packages: rattler_conda_types

## [0.2.2](https://github.com/conda/rattler/compare/rattler_cache-v0.2.1...rattler_cache-v0.2.2) - 2024-09-05

### Fixed
- typos ([#849](https://github.com/conda/rattler/pull/849))

## [0.2.1](https://github.com/conda/rattler/compare/rattler_cache-v0.2.0...rattler_cache-v0.2.1) - 2024-09-03

### Fixed
- allow `gcs://` and `oci://` in gateway ([#845](https://github.com/conda/rattler/pull/845))

## [0.2.0](https://github.com/conda/rattler/compare/rattler_cache-v0.1.9...rattler_cache-v0.2.0) - 2024-09-03

### Other
- make PackageCache multi-process safe ([#837](https://github.com/conda/rattler/pull/837))

## [0.1.9](https://github.com/conda/rattler/compare/rattler_cache-v0.1.8...rattler_cache-v0.1.9) - 2024-09-02

### Other
- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.1.8](https://github.com/conda/rattler/compare/rattler_cache-v0.1.7...rattler_cache-v0.1.8) - 2024-08-16

### Other
- updated the following local packages: rattler_networking

## [0.1.7](https://github.com/conda/rattler/compare/rattler_cache-v0.1.6...rattler_cache-v0.1.7) - 2024-08-15

### Fixed
- move more links to the conda org from conda-incubator ([#816](https://github.com/conda/rattler/pull/816))

### Other
- change links from conda-incubator to conda ([#813](https://github.com/conda/rattler/pull/813))
- update banner ([#808](https://github.com/conda/rattler/pull/808))

## [0.1.6](https://github.com/baszalmstra/rattler/compare/rattler_cache-v0.1.5...rattler_cache-v0.1.6) - 2024-08-06

### Other
- updated the following local packages: rattler_conda_types

## [0.1.5](https://github.com/baszalmstra/rattler/compare/rattler_cache-v0.1.4...rattler_cache-v0.1.5) - 2024-08-02

### Other
- mark some crates 1.0 ([#789](https://github.com/baszalmstra/rattler/pull/789))

## [0.1.4](https://github.com/conda/rattler/compare/rattler_cache-v0.1.3...rattler_cache-v0.1.4) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.1.3](https://github.com/conda/rattler/compare/rattler_cache-v0.1.2...rattler_cache-v0.1.3) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.1.2](https://github.com/conda/rattler/compare/rattler_cache-v0.1.1...rattler_cache-v0.1.2) - 2024-07-15

### Other
- bump dependencies and remove unused ones ([#771](https://github.com/conda/rattler/pull/771))

## [0.1.1](https://github.com/conda/rattler/compare/rattler_cache-v0.1.0...rattler_cache-v0.1.1) - 2024-07-08

### Added
- add direct url repodata building ([#725](https://github.com/conda/rattler/pull/725))

### Fixed
- run clippy on all targets ([#762](https://github.com/conda/rattler/pull/762))

## [0.1.0](https://github.com/baszalmstra/rattler/releases/tag/rattler_cache-v0.1.0) - 2024-06-04

### Other
- remove lfs ([#512](https://github.com/baszalmstra/rattler/pull/512))
- move the cache tooling into its own crate for reuse downstream ([#721](https://github.com/baszalmstra/rattler/pull/721))
