# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.22.11](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.22.10...rattler_package_streaming-v0.22.11) - 2024-11-04

### Other

- release ([#903](https://github.com/conda/rattler/pull/903))

## [0.22.10](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.22.9...rattler_package_streaming-v0.22.10) - 2024-10-07

### Added

- make `ExtractError` more informative ([#889](https://github.com/conda/rattler/pull/889))

## [0.22.9](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.22.8...rattler_package_streaming-v0.22.9) - 2024-10-03

### Other

- updated the following local packages: rattler_conda_types

## [0.22.8](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.22.7...rattler_package_streaming-v0.22.8) - 2024-09-23

### Other

- updated the following local packages: rattler_conda_types

## [0.22.7](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.22.6...rattler_package_streaming-v0.22.7) - 2024-09-09

### Other

- updated the following local packages: rattler_conda_types

## [0.22.6](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.22.5...rattler_package_streaming-v0.22.6) - 2024-09-05

### Fixed
- typos ([#849](https://github.com/conda/rattler/pull/849))

## [0.22.5](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.22.4...rattler_package_streaming-v0.22.5) - 2024-09-03

### Other
- make PackageCache multi-process safe ([#837](https://github.com/conda/rattler/pull/837))

## [0.22.4](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.22.3...rattler_package_streaming-v0.22.4) - 2024-09-02

### Added
- Add support for `CONDA_OVERRIDE_CUDA` ([#818](https://github.com/conda/rattler/pull/818))

### Fixed
- zip large files compression ([#838](https://github.com/conda/rattler/pull/838))

## [0.22.3](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.22.2...rattler_package_streaming-v0.22.3) - 2024-08-16

### Other
- updated the following local packages: rattler_networking

## [0.22.2](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.22.1...rattler_package_streaming-v0.22.2) - 2024-08-15

### Fixed
- move more links to the conda org from conda-incubator ([#816](https://github.com/conda/rattler/pull/816))
- use conda-incubator

### Other
- change links from conda-incubator to conda ([#813](https://github.com/conda/rattler/pull/813))
- update banner ([#808](https://github.com/conda/rattler/pull/808))

## [0.22.1](https://github.com/baszalmstra/rattler/compare/rattler_package_streaming-v0.22.0...rattler_package_streaming-v0.22.1) - 2024-08-06

### Other
- updated the following local packages: rattler_conda_types

## [0.22.0](https://github.com/baszalmstra/rattler/compare/rattler_package_streaming-v0.21.7...rattler_package_streaming-v0.22.0) - 2024-08-02

### Fixed
- redact secrets in the `canonical_name` functions ([#801](https://github.com/baszalmstra/rattler/pull/801))
- Fallback to fully reading the package stream when downloading before attempting decompression ([#797](https://github.com/baszalmstra/rattler/pull/797))

### Other
- mark some crates 1.0 ([#789](https://github.com/baszalmstra/rattler/pull/789))

## [0.21.7](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.21.6...rattler_package_streaming-v0.21.7) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.21.6](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.21.5...rattler_package_streaming-v0.21.6) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.21.5](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.21.4...rattler_package_streaming-v0.21.5) - 2024-07-15

### Other
- bump zip to 2.1.3 ([#772](https://github.com/conda/rattler/pull/772))

## [0.21.4](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.21.3...rattler_package_streaming-v0.21.4) - 2024-07-08

### Fixed
- run clippy on all targets ([#762](https://github.com/conda/rattler/pull/762))

## [0.21.3](https://github.com/baszalmstra/rattler/compare/rattler_package_streaming-v0.21.2...rattler_package_streaming-v0.21.3) - 2024-06-04

### Other
- remove lfs ([#512](https://github.com/baszalmstra/rattler/pull/512))

## [0.21.2](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.21.1...rattler_package_streaming-v0.21.2) - 2024-06-03

### Fixed
- call on_download_start also for file URLs ([#708](https://github.com/conda/rattler/pull/708))

## [0.21.1](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.21.0...rattler_package_streaming-v0.21.1) - 2024-05-28

### Other
- updated the following local packages: rattler_conda_types

## [0.21.0](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.20.10...rattler_package_streaming-v0.21.0) - 2024-05-27

### Other
- introducing the installer ([#664](https://github.com/conda/rattler/pull/664))

## [0.20.10](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.20.9...rattler_package_streaming-v0.20.10) - 2024-05-14

### Added
- exclude repodata records based on timestamp ([#654](https://github.com/conda/rattler/pull/654))

## [0.20.9](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.20.8...rattler_package_streaming-v0.20.9) - 2024-05-13

### Added
- high level repodata access ([#560](https://github.com/conda/rattler/pull/560))

### Fixed
- set last modified for zip archive ([#649](https://github.com/conda/rattler/pull/649))

### Other
- update README.md

## [0.20.8](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.20.7...rattler_package_streaming-v0.20.8) - 2024-05-06

### Other
- updated the following local packages: rattler_conda_types, rattler_networking

## [0.20.7](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.20.6...rattler_package_streaming-v0.20.7) - 2024-04-30

### Other
- updated the following local packages: rattler_networking

## [0.20.6](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.20.5...rattler_package_streaming-v0.20.6) - 2024-04-25

### Other
- updated the following local packages: rattler_networking

## [0.20.5](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.20.4...rattler_package_streaming-v0.20.5) - 2024-04-25

### Other
- updated the following local packages: rattler_conda_types

## [0.20.4](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.20.3...rattler_package_streaming-v0.20.4) - 2024-04-19

### Other
- update dependencies incl. reqwest ([#606](https://github.com/conda/rattler/pull/606))

## [0.20.3](https://github.com/baszalmstra/rattler/compare/rattler_package_streaming-v0.20.2...rattler_package_streaming-v0.20.3) - 2024-04-05

### Other
- updated the following local packages: rattler_conda_types, rattler_networking

## [0.20.2](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.20.1...rattler_package_streaming-v0.20.2) - 2024-03-30

### Other
- updated the following local packages: rattler_conda_types

## [0.20.1](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.20.0...rattler_package_streaming-v0.20.1) - 2024-03-21

### Other
- updated the following local packages: rattler_conda_types, rattler_networking

## [0.20.0](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.19.2...rattler_package_streaming-v0.20.0) - 2024-03-14

### Added
- add mirror handling and OCI mirror type ([#553](https://github.com/conda/rattler/pull/553))

### Other
- add pixi badge ([#563](https://github.com/conda/rattler/pull/563))

## [0.19.2](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.19.1...rattler_package_streaming-v0.19.2) - 2024-03-08

### Other
- update Cargo.toml dependencies

## [0.19.1](https://github.com/conda/rattler/compare/rattler_package_streaming-v0.19.0...rattler_package_streaming-v0.19.1) - 2024-03-06

### Fixed
- dont use workspace dependencies for local crates ([#546](https://github.com/conda/rattler/pull/546))

### Other
- every crate should have its own version ([#557](https://github.com/conda/rattler/pull/557))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler_package_streaming-v0.18.0...rattler_package_streaming-v0.19.0) - 2024-02-26

### Fixed
- flaky package extract error ([#535](https://github.com/baszalmstra/rattler/pull/535))
