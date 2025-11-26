# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.34](https://github.com/conda/rattler/compare/rattler_cache-v0.3.33...rattler_cache-v0.3.34) - 2025-09-05

### Other

- updated the following local packages: rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.3.33](https://github.com/conda/rattler/compare/rattler_cache-v0.3.32...rattler_cache-v0.3.33) - 2025-09-04

### Other

- updated the following local packages: rattler_networking, rattler_package_streaming

## [0.3.32](https://github.com/conda/rattler/compare/rattler_cache-v0.3.31...rattler_cache-v0.3.32) - 2025-09-02

### Other

- updated the following local packages: rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.3.31](https://github.com/conda/rattler/compare/rattler_cache-v0.3.30...rattler_cache-v0.3.31) - 2025-08-15

### Other

- updated the following local packages: rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.3.30](https://github.com/conda/rattler/compare/rattler_cache-v0.3.29...rattler_cache-v0.3.30) - 2025-08-12

### Added

- Redact token in url when reporting a HashMismatch error ([#1579](https://github.com/conda/rattler/pull/1579))
- Provide more details when hash mismatch occurs ([#1577](https://github.com/conda/rattler/pull/1577))

### Fixed

- improve hash mismatch warning to include package path ([#1573](https://github.com/conda/rattler/pull/1573))
# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.3](https://github.com/conda/rattler/compare/rattler_cache-v0.6.2...rattler_cache-v0.6.3) - 2025-11-25

### Other

- updated the following local packages: rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.6.2](https://github.com/conda/rattler/compare/rattler_cache-v0.6.1...rattler_cache-v0.6.2) - 2025-11-22

### Other

- updated the following local packages: rattler_conda_types, rattler_package_streaming, rattler_networking

## [0.6.1](https://github.com/conda/rattler/compare/rattler_cache-v0.6.0...rattler_cache-v0.6.1) - 2025-11-20

### Other

- updated the following local packages: rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.6.0](https://github.com/conda/rattler/compare/rattler_cache-v0.5.0...rattler_cache-v0.6.0) - 2025-11-19

### Added

- cache and reuse `paths.json` and `index.json` from package validation ([#1837](https://github.com/conda/rattler/pull/1837))

## [0.5.0](https://github.com/conda/rattler/compare/rattler_cache-v0.4.1...rattler_cache-v0.5.0) - 2025-11-13

### Added

- add validation mode argument to PackageCache layers ([#1834](https://github.com/conda/rattler/pull/1834))

### Other

- remove cache lock mutex ([#1809](https://github.com/conda/rattler/pull/1809))
- use global cache lock to reduce per-package lock overhead ([#1818](https://github.com/conda/rattler/pull/1818))

## [0.4.1](https://github.com/conda/rattler/compare/rattler_cache-v0.4.0...rattler_cache-v0.4.1) - 2025-10-28

### Other

- Replace fxhash with ahash ([#1674](https://github.com/conda/rattler/pull/1674))

## [0.4.0](https://github.com/conda/rattler/compare/rattler_cache-v0.3.41...rattler_cache-v0.4.0) - 2025-10-25

### Added

- add support for layered package cache ([#1003](https://github.com/conda/rattler/pull/1003))

## [0.3.41](https://github.com/conda/rattler/compare/rattler_cache-v0.3.40...rattler_cache-v0.3.41) - 2025-10-18

### Other

- updated the following local packages: rattler_conda_types, rattler_package_streaming, rattler_networking

## [0.3.40](https://github.com/conda/rattler/compare/rattler_cache-v0.3.39...rattler_cache-v0.3.40) - 2025-10-17

### Other

- updated the following local packages: rattler_digest, rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.3.39](https://github.com/conda/rattler/compare/rattler_cache-v0.3.38...rattler_cache-v0.3.39) - 2025-10-14

### Other

- updated the following local packages: rattler_digest, rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.3.38](https://github.com/conda/rattler/compare/rattler_cache-v0.3.37...rattler_cache-v0.3.38) - 2025-10-13

### Other

- updated the following local packages: rattler_networking, rattler_package_streaming

## [0.3.37](https://github.com/conda/rattler/compare/rattler_cache-v0.3.36...rattler_cache-v0.3.37) - 2025-10-07

### Fixed

- ignore md5 hash if sha256 already matches ([#1703](https://github.com/conda/rattler/pull/1703))

## [0.3.36](https://github.com/conda/rattler/compare/rattler_cache-v0.3.35...rattler_cache-v0.3.36) - 2025-10-03

### Other

- updated the following local packages: rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.3.35](https://github.com/conda/rattler/compare/rattler_cache-v0.3.34...rattler_cache-v0.3.35) - 2025-09-30

### Other

- add LazyClient to late initialize the reqwest client ([#1687](https://github.com/conda/rattler/pull/1687))

## [0.3.29](https://github.com/conda/rattler/compare/rattler_cache-v0.3.28...rattler_cache-v0.3.29) - 2025-07-28

### Other

- updated the following local packages: rattler_package_streaming

## [0.3.28](https://github.com/conda/rattler/compare/rattler_cache-v0.3.27...rattler_cache-v0.3.28) - 2025-07-23

### Other

- update Cargo.toml dependencies

## [0.3.27](https://github.com/conda/rattler/compare/rattler_cache-v0.3.26...rattler_cache-v0.3.27) - 2025-07-21

### Other

- updated the following local packages: rattler_digest, rattler_conda_types, rattler_package_streaming, rattler_networking

## [0.3.26](https://github.com/conda/rattler/compare/rattler_cache-v0.3.25...rattler_cache-v0.3.26) - 2025-07-14

### Other

- updated the following local packages: rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.3.25](https://github.com/conda/rattler/compare/rattler_cache-v0.3.24...rattler_cache-v0.3.25) - 2025-07-09

### Other

- updated the following local packages: rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.3.24](https://github.com/conda/rattler/compare/rattler_cache-v0.3.23...rattler_cache-v0.3.24) - 2025-07-01

### Fixed

- *(ci)* run pre-commit-run for all files ([#1481](https://github.com/conda/rattler/pull/1481))

## [0.3.23](https://github.com/conda/rattler/compare/rattler_cache-v0.3.22...rattler_cache-v0.3.23) - 2025-06-26

### Other

- updated the following local packages: rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.3.22](https://github.com/conda/rattler/compare/rattler_cache-v0.3.21...rattler_cache-v0.3.22) - 2025-06-25

### Other

- updated the following local packages: rattler_conda_types, rattler_networking, rattler_package_streaming

## [0.3.21](https://github.com/conda/rattler/compare/rattler_cache-v0.3.20...rattler_cache-v0.3.21) - 2025-06-23

### Other

- update npm name ([#1368](https://github.com/conda/rattler/pull/1368))
- update readme ([#1364](https://github.com/conda/rattler/pull/1364))

## [0.3.20](https://github.com/conda/rattler/compare/rattler_cache-v0.3.19...rattler_cache-v0.3.20) - 2025-05-23

### Fixed

- consistent usage of rustls-tls / native-tls feature ([#1324](https://github.com/conda/rattler/pull/1324))

## [0.3.19](https://github.com/conda/rattler/compare/rattler_cache-v0.3.18...rattler_cache-v0.3.19) - 2025-05-16

### Other

- make sure that md5 also works as `CacheKey` ([#1293](https://github.com/conda/rattler/pull/1293))
- Bump zip to 3.0.0 ([#1310](https://github.com/conda/rattler/pull/1310))

## [0.3.18](https://github.com/conda/rattler/compare/rattler_cache-v0.3.17...rattler_cache-v0.3.18) - 2025-05-03

### Other

- lock workspace member dependencies ([#1279](https://github.com/conda/rattler/pull/1279))

## [0.3.17](https://github.com/conda/rattler/compare/rattler_cache-v0.3.16...rattler_cache-v0.3.17) - 2025-04-10

### Fixed

- Add location to the cache key ([#1143](https://github.com/conda/rattler/pull/1143))

## [0.3.16](https://github.com/conda/rattler/compare/rattler_cache-v0.3.15...rattler_cache-v0.3.16) - 2025-04-04

### Other

- update Cargo.toml dependencies

## [0.3.15](https://github.com/conda/rattler/compare/rattler_cache-v0.3.14...rattler_cache-v0.3.15) - 2025-03-14

### Other

- updated the following local packages: rattler_conda_types, rattler_networking

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
