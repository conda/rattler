# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.22.0](https://github.com/conda/rattler/compare/rattler_index-v0.21.2...rattler_index-v0.22.0) - 2025-03-10

### Added

- Add support for repodata patching in rattler-index, fix silent failures ([#1129](https://github.com/conda/rattler/pull/1129))

## [0.21.2](https://github.com/conda/rattler/compare/rattler_index-v0.21.1...rattler_index-v0.21.2) - 2025-03-04

### Other

- update Cargo.lock dependencies

## [0.21.1](https://github.com/conda/rattler/compare/rattler_index-v0.21.0...rattler_index-v0.21.1) - 2025-02-28

### Other

- create reader from opendal buffer (#1123)

## [0.21.0](https://github.com/conda/rattler/compare/rattler_index-v0.20.13...rattler_index-v0.21.0) - 2025-02-27

### Added

- fix rattler-index name (#1114)
- Use `opendal` in `rattler-index` and add executable (#1076)

### Fixed

- make `menuinst` schema pub, hide utils, fix indexing for rattler-build (#1111)
- code review in rattler-index and test fix (#1109)

### Other

- remove tools/* features for rattler-index (#1112)

## [0.20.13](https://github.com/conda/rattler/compare/rattler_index-v0.20.12...rattler_index-v0.20.13) - 2025-02-25

### Other

- update Cargo.toml dependencies

## [0.20.12](https://github.com/conda/rattler/compare/rattler_index-v0.20.11...rattler_index-v0.20.12) - 2025-02-18

### Other

- update Cargo.toml dependencies

## [0.20.11](https://github.com/conda/rattler/compare/rattler_index-v0.20.10...rattler_index-v0.20.11) - 2025-02-06

### Other

- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.20.10](https://github.com/conda/rattler/compare/rattler_index-v0.20.9...rattler_index-v0.20.10) - 2025-02-06

### Other

- release (#1050)

## [0.20.9](https://github.com/conda/rattler/compare/rattler_index-v0.20.8...rattler_index-v0.20.9) - 2025-02-03

### Other

- updated the following local packages: rattler_conda_types

## [0.20.8](https://github.com/conda/rattler/compare/rattler_index-v0.20.7...rattler_index-v0.20.8) - 2025-01-23

### Added

- use tempfile and persist for repodata (#1031)

## [0.20.7](https://github.com/conda/rattler/compare/rattler_index-v0.20.6...rattler_index-v0.20.7) - 2025-01-09

### Other

- updated the following local packages: rattler_conda_types

## [0.20.6](https://github.com/conda/rattler/compare/rattler_index-v0.20.5...rattler_index-v0.20.6) - 2025-01-09

### Other

- updated the following local packages: rattler_conda_types

## [0.20.5](https://github.com/conda/rattler/compare/rattler_index-v0.20.4...rattler_index-v0.20.5) - 2025-01-08

### Other

- updated the following local packages: rattler_conda_types, rattler_digest, rattler_package_streaming

## [0.20.4](https://github.com/conda/rattler/compare/rattler_index-v0.20.3...rattler_index-v0.20.4) - 2024-12-20

### Other

- updated the following local packages: rattler_conda_types

## [0.20.3](https://github.com/conda/rattler/compare/rattler_index-v0.20.2...rattler_index-v0.20.3) - 2024-12-17

### Other

- update Cargo.toml dependencies

## [0.20.2](https://github.com/conda/rattler/compare/rattler_index-v0.20.1...rattler_index-v0.20.2) - 2024-12-13

### Other

- updated the following local packages: rattler_conda_types

## [0.20.1](https://github.com/conda/rattler/compare/rattler_index-v0.20.0...rattler_index-v0.20.1) - 2024-12-12

### Other
- updated the following local packages: rattler_conda_types

## [0.20.0](https://github.com/conda/rattler/compare/rattler_index-v0.19.37...rattler_index-v0.20.0) - 2024-12-05

### Other

- release ([#967](https://github.com/conda/rattler/pull/967))

## [0.19.37](https://github.com/conda/rattler/compare/rattler_index-v0.19.36...rattler_index-v0.19.37) - 2024-11-30

### Other

- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.19.36](https://github.com/conda/rattler/compare/rattler_index-v0.19.35...rattler_index-v0.19.36) - 2024-11-18

### Other

- updated the following local packages: rattler_conda_types

## [0.19.35](https://github.com/conda/rattler/compare/rattler_index-v0.19.34...rattler_index-v0.19.35) - 2024-11-14

### Other

- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.19.34](https://github.com/conda/rattler/compare/rattler_index-v0.19.33...rattler_index-v0.19.34) - 2024-11-04

### Added

- use python_site_packages_path field when available for installing noarch: python packages, CEP-17 ([#909](https://github.com/conda/rattler/pull/909))

## [0.19.33](https://github.com/conda/rattler/compare/rattler_index-v0.19.32...rattler_index-v0.19.33) - 2024-10-21

### Fixed

- always index noarch even if folder already exists ([#907](https://github.com/conda/rattler/pull/907))

## [0.19.32](https://github.com/conda/rattler/compare/rattler_index-v0.19.31...rattler_index-v0.19.32) - 2024-10-07

### Other

- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.19.31](https://github.com/conda/rattler/compare/rattler_index-v0.19.30...rattler_index-v0.19.31) - 2024-10-03

### Other

- updated the following local packages: rattler_conda_types

## [0.19.30](https://github.com/conda/rattler/compare/rattler_index-v0.19.29...rattler_index-v0.19.30) - 2024-10-01

### Other

- update Cargo.toml dependencies

## [0.19.29](https://github.com/conda/rattler/compare/rattler_index-v0.19.28...rattler_index-v0.19.29) - 2024-09-23

### Other

- updated the following local packages: rattler_conda_types

## [0.19.28](https://github.com/conda/rattler/compare/rattler_index-v0.19.27...rattler_index-v0.19.28) - 2024-09-09

### Other

- updated the following local packages: rattler_conda_types

## [0.19.27](https://github.com/conda/rattler/compare/rattler_index-v0.19.26...rattler_index-v0.19.27) - 2024-09-05

### Fixed
- typos ([#849](https://github.com/conda/rattler/pull/849))

## [0.19.26](https://github.com/conda/rattler/compare/rattler_index-v0.19.25...rattler_index-v0.19.26) - 2024-09-03

### Other
- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.19.25](https://github.com/conda/rattler/compare/rattler_index-v0.19.24...rattler_index-v0.19.25) - 2024-09-02

### Other
- release ([#824](https://github.com/conda/rattler/pull/824))

## [0.19.24](https://github.com/conda/rattler/compare/rattler_index-v0.19.23...rattler_index-v0.19.24) - 2024-08-15

### Fixed
- move more links to the conda org from conda-incubator ([#816](https://github.com/conda/rattler/pull/816))

### Other
- change links from conda-incubator to conda ([#813](https://github.com/conda/rattler/pull/813))
- update banner ([#808](https://github.com/conda/rattler/pull/808))

## [0.19.23](https://github.com/baszalmstra/rattler/compare/rattler_index-v0.19.22...rattler_index-v0.19.23) - 2024-08-06

### Other
- updated the following local packages: rattler_conda_types

## [0.19.22](https://github.com/baszalmstra/rattler/compare/rattler_index-v0.19.21...rattler_index-v0.19.22) - 2024-08-02

### Other
- mark some crates 1.0 ([#789](https://github.com/baszalmstra/rattler/pull/789))

## [0.19.21](https://github.com/conda/rattler/compare/rattler_index-v0.19.20...rattler_index-v0.19.21) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.19.20](https://github.com/conda/rattler/compare/rattler_index-v0.19.19...rattler_index-v0.19.20) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.19.19](https://github.com/conda/rattler/compare/rattler_index-v0.19.18...rattler_index-v0.19.19) - 2024-07-15

### Other
- updated the following local packages: rattler_conda_types, rattler_digest, rattler_package_streaming

## [0.19.18](https://github.com/conda/rattler/compare/rattler_index-v0.19.17...rattler_index-v0.19.18) - 2024-07-08

### Other
- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.19.17](https://github.com/conda/rattler/compare/rattler_index-v0.19.16...rattler_index-v0.19.17) - 2024-06-06

### Other
- make package_record_from_* functions public ([#726](https://github.com/conda/rattler/pull/726))

## [0.19.16](https://github.com/baszalmstra/rattler/compare/rattler_index-v0.19.15...rattler_index-v0.19.16) - 2024-06-04

### Other
- remove lfs ([#512](https://github.com/baszalmstra/rattler/pull/512))

## [0.19.15](https://github.com/conda/rattler/compare/rattler_index-v0.19.14...rattler_index-v0.19.15) - 2024-06-03

### Other
- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.19.14](https://github.com/conda/rattler/compare/rattler_index-v0.19.13...rattler_index-v0.19.14) - 2024-05-28

### Added
- add run exports to package data ([#671](https://github.com/conda/rattler/pull/671))

## [0.19.13](https://github.com/conda/rattler/compare/rattler_index-v0.19.12...rattler_index-v0.19.13) - 2024-05-27

### Added
- always store purls as a key in lock file ([#669](https://github.com/conda/rattler/pull/669))

## [0.19.12](https://github.com/conda/rattler/compare/rattler_index-v0.19.11...rattler_index-v0.19.12) - 2024-05-14

### Other
- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.19.11](https://github.com/conda/rattler/compare/rattler_index-v0.19.10...rattler_index-v0.19.11) - 2024-05-13

### Other
- updated the following local packages: rattler_conda_types, rattler_digest, rattler_package_streaming

## [0.19.10](https://github.com/conda/rattler/compare/rattler_index-v0.19.9...rattler_index-v0.19.10) - 2024-05-06

### Other
- updated the following local packages: rattler_conda_types

## [0.19.9](https://github.com/conda/rattler/compare/rattler_index-v0.19.8...rattler_index-v0.19.9) - 2024-04-30

### Other
- release ([#625](https://github.com/conda/rattler/pull/625))

## [0.19.8](https://github.com/conda/rattler/compare/rattler_index-v0.19.7...rattler_index-v0.19.8) - 2024-04-25

### Other
- updated the following local packages: rattler_conda_types

## [0.19.7](https://github.com/conda/rattler/compare/rattler_index-v0.19.6...rattler_index-v0.19.7) - 2024-04-19

### Other
- update Cargo.toml dependencies

## [0.19.6](https://github.com/baszalmstra/rattler/compare/rattler_index-v0.19.5...rattler_index-v0.19.6) - 2024-04-05

### Other
- updated the following local packages: rattler_conda_types

## [0.19.5](https://github.com/conda/rattler/compare/rattler_index-v0.19.4...rattler_index-v0.19.5) - 2024-03-30

### Other
- updated the following local packages: rattler_conda_types

## [0.19.4](https://github.com/conda/rattler/compare/rattler_index-v0.19.3...rattler_index-v0.19.4) - 2024-03-21

### Other
- updated the following local packages: rattler_conda_types

## [0.19.3](https://github.com/conda/rattler/compare/rattler_index-v0.19.2...rattler_index-v0.19.3) - 2024-03-14

### Other
- add pixi badge ([#563](https://github.com/conda/rattler/pull/563))

## [0.19.2](https://github.com/conda/rattler/compare/rattler_index-v0.19.1...rattler_index-v0.19.2) - 2024-03-08

### Other
- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.19.1](https://github.com/conda/rattler/compare/rattler_index-v0.19.0...rattler_index-v0.19.1) - 2024-03-06

### Fixed
- dont use workspace dependencies for local crates ([#546](https://github.com/conda/rattler/pull/546))

### Other
- every crate should have its own version ([#557](https://github.com/conda/rattler/pull/557))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler_index-v0.18.0...rattler_index-v0.19.0) - 2024-02-26

### Fixed
- Add indexed packages to the relevant section in the repodata ([#529](https://github.com/baszalmstra/rattler/pull/529))
