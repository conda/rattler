# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.14](https://github.com/conda/rattler/compare/rattler_cache-v0.3.13...rattler_cache-v0.3.14) - 2025-03-10

### Other

- update Cargo.toml dependencies

## [0.3.13](https://github.com/conda/rattler/compare/rattler_cache-v0.3.12...rattler_cache-v0.3.13) - 2025-03-04

### Added

- *(js)* compile `rattler_solve` and `rattler_repodata_gateway` ([#1108](https://github.com/conda/rattler/pull/1108))

## [0.3.12](https://github.com/conda/rattler/compare/rattler_cache-v0.3.11...rattler_cache-v0.3.12) - 2025-02-28

### Other

- update Cargo.toml dependencies

## [0.3.11](https://github.com/conda/rattler/compare/rattler_cache-v0.3.10...rattler_cache-v0.3.11) - 2025-02-27

### Other

- updated the following local packages: rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.3.10](https://github.com/conda/rattler/compare/rattler_cache-v0.3.9...rattler_cache-v0.3.10) - 2025-02-25

### Added

- add run_exports cache (#1060)

### Fixed

- support file URL for run exports cache (#1081)

### Other

- use run-exports (#1077)

## [0.3.9](https://github.com/conda/rattler/compare/rattler_cache-v0.3.8...rattler_cache-v0.3.9) - 2025-02-18

### Other

- update Cargo.toml dependencies

## [0.3.8](https://github.com/conda/rattler/compare/rattler_cache-v0.3.7...rattler_cache-v0.3.8) - 2025-02-06

### Other

- bump rust 1.84.1 (#1053)

## [0.3.7](https://github.com/conda/rattler/compare/rattler_cache-v0.3.6...rattler_cache-v0.3.7) - 2025-02-06

### Other

- updated the following local packages: rattler_networking

## [0.3.6](https://github.com/conda/rattler/compare/rattler_cache-v0.3.5...rattler_cache-v0.3.6) - 2025-02-03

### Other

- updated the following local packages: rattler_conda_types, rattler_networking

## [0.3.5](https://github.com/conda/rattler/compare/rattler_cache-v0.3.4...rattler_cache-v0.3.5) - 2025-01-23

### Other

- updated the following local packages: rattler_conda_types, rattler_networking

## [0.3.4](https://github.com/conda/rattler/compare/rattler_cache-v0.3.3...rattler_cache-v0.3.4) - 2025-01-09

### Other

- updated the following local packages: rattler_conda_types

## [0.3.3](https://github.com/conda/rattler/compare/rattler_cache-v0.3.2...rattler_cache-v0.3.3) - 2025-01-09

### Other

- updated the following local packages: rattler_conda_types

## [0.3.2](https://github.com/conda/rattler/compare/rattler_cache-v0.3.1...rattler_cache-v0.3.2) - 2025-01-08

### Fixed

- retry failed repodata streaming on io error (#1017)

### Other

- update dependencies (#1009)

## [0.3.1](https://github.com/conda/rattler/compare/rattler_cache-v0.3.0...rattler_cache-v0.3.1) - 2024-12-20

### Other

- reflink directories at once on macOS (#995)

## [0.3.0](https://github.com/conda/rattler/compare/rattler_cache-v0.2.15...rattler_cache-v0.3.0) - 2024-12-17

### Added

- speed up `PrefixRecord` loading (#984)
- improve performance when linking files using `rayon` (#985)

## [0.2.15](https://github.com/conda/rattler/compare/rattler_cache-v0.2.14...rattler_cache-v0.2.15) - 2024-12-13

### Other

- updated the following local packages: rattler_conda_types

## [0.2.14](https://github.com/conda/rattler/compare/rattler_cache-v0.2.13...rattler_cache-v0.2.14) - 2024-12-12

### Fixed
- package cache lock file_name was incorrect ([#977](https://github.com/conda/rattler/pull/977))

## [0.2.13](https://github.com/conda/rattler/compare/rattler_cache-v0.2.12...rattler_cache-v0.2.13) - 2024-12-05

### Other

- updated the following local packages: rattler_networking

## [0.2.12](https://github.com/conda/rattler/compare/rattler_cache-v0.2.11...rattler_cache-v0.2.12) - 2024-11-30

### Added

- use `fs-err` also for tokio ([#958](https://github.com/conda/rattler/pull/958))

## [0.2.11](https://github.com/conda/rattler/compare/rattler_cache-v0.2.10...rattler_cache-v0.2.11) - 2024-11-18

### Other

- updated the following local packages: rattler_networking

## [0.2.10](https://github.com/conda/rattler/compare/rattler_cache-v0.2.9...rattler_cache-v0.2.10) - 2024-11-18

### Other

- updated the following local packages: rattler_conda_types

## [0.2.9](https://github.com/conda/rattler/compare/rattler_cache-v0.2.8...rattler_cache-v0.2.9) - 2024-11-14

### Added

- set cache directory ([#934](https://github.com/conda/rattler/pull/934))

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
