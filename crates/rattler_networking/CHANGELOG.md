# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.25.6](https://github.com/conda/rattler/compare/rattler_networking-v0.25.5...rattler_networking-v0.25.6) - 2025-07-14

### Other

- updated the following local packages: rattler_config

## [0.25.5](https://github.com/conda/rattler/compare/rattler_networking-v0.25.4...rattler_networking-v0.25.5) - 2025-07-09

### Other

- updated the following local packages: rattler_config

## [0.25.4](https://github.com/conda/rattler/compare/rattler_networking-v0.25.3...rattler_networking-v0.25.4) - 2025-07-01

### Fixed

- *(ci)* run pre-commit-run for all files ([#1481](https://github.com/conda/rattler/pull/1481))

### Other

- *(ci)* Update Rust crate google-cloud-auth to 0.21.0 ([#1461](https://github.com/conda/rattler/pull/1461))

## [0.25.3](https://github.com/conda/rattler/compare/rattler_networking-v0.25.2...rattler_networking-v0.25.3) - 2025-06-26

### Other

- updated the following local packages: rattler_config

## [0.25.2](https://github.com/conda/rattler/compare/rattler_networking-v0.25.1...rattler_networking-v0.25.2) - 2025-06-25

### Added

- *(rattler_index)* Use rattler_config ([#1466](https://github.com/conda/rattler/pull/1466))

## [0.25.1](https://github.com/conda/rattler/compare/rattler_networking-v0.25.0...rattler_networking-v0.25.1) - 2025-06-23

### Added

- add `rattler_config` crate (derived from `pixi_config`) ([#1389](https://github.com/conda/rattler/pull/1389))
- make rattler_networking system integration optional ([#1381](https://github.com/conda/rattler/pull/1381))

### Fixed

- reduce s3 into to trace ([#1395](https://github.com/conda/rattler/pull/1395))

### Other

- update npm name ([#1368](https://github.com/conda/rattler/pull/1368))
- update readme ([#1364](https://github.com/conda/rattler/pull/1364))

## [0.25.0](https://github.com/conda/rattler/compare/rattler_networking-v0.24.0...rattler_networking-v0.25.0) - 2025-05-23

### Fixed

- consistent usage of rustls-tls / native-tls feature ([#1324](https://github.com/conda/rattler/pull/1324))

## [0.24.0](https://github.com/conda/rattler/compare/rattler_networking-v0.23.0...rattler_networking-v0.24.0) - 2025-05-16

### Other

- update dependencies of js-rattler and py-rattler as well ([#1317](https://github.com/conda/rattler/pull/1317))
- update GCS authentication ([#1314](https://github.com/conda/rattler/pull/1314))

## [0.23.0](https://github.com/conda/rattler/compare/rattler_networking-v0.22.12...rattler_networking-v0.23.0) - 2025-05-03

### Added

- Add MemoryStorage as authentication backend ([#1265](https://github.com/conda/rattler/pull/1265))

## [0.22.12](https://github.com/conda/rattler/compare/rattler_networking-v0.22.11...rattler_networking-v0.22.12) - 2025-04-10

### Other

- update Cargo.toml dependencies

## [0.22.11](https://github.com/conda/rattler/compare/rattler_networking-v0.22.10...rattler_networking-v0.22.11) - 2025-04-04

### Other

- add the remove_from_backup function and update the prefix ([#1155](https://github.com/conda/rattler/pull/1155))
- fix js bindings ([#1203](https://github.com/conda/rattler/pull/1203))

## [0.22.10](https://github.com/conda/rattler/compare/rattler_networking-v0.22.9...rattler_networking-v0.22.10) - 2025-03-14

### Other

- update Cargo.toml dependencies

## [0.22.9](https://github.com/conda/rattler/compare/rattler_networking-v0.22.8...rattler_networking-v0.22.9) - 2025-03-10

### Other

- update Cargo.toml dependencies

## [0.22.8](https://github.com/conda/rattler/compare/rattler_networking-v0.22.7...rattler_networking-v0.22.8) - 2025-03-04

### Added

- *(js)* compile `rattler_solve` and `rattler_repodata_gateway` ([#1108](https://github.com/conda/rattler/pull/1108))

## [0.22.7](https://github.com/conda/rattler/compare/rattler_networking-v0.22.6...rattler_networking-v0.22.7) - 2025-02-28

### Fixed

- R2 key names in tests (#1115)

## [0.22.6](https://github.com/conda/rattler/compare/rattler_networking-v0.22.5...rattler_networking-v0.22.6) - 2025-02-27

### Fixed

- clippy lint (#1105)

## [0.22.5](https://github.com/conda/rattler/compare/rattler_networking-v0.22.4...rattler_networking-v0.22.5) - 2025-02-25

### Other

- update Cargo.toml dependencies

## [0.22.4](https://github.com/conda/rattler/compare/rattler_networking-v0.22.3...rattler_networking-v0.22.4) - 2025-02-18

### Other

- update dependencies (#1069)

## [0.22.3](https://github.com/conda/rattler/compare/rattler_networking-v0.22.2...rattler_networking-v0.22.3) - 2025-02-06

### Fixed

- use atomic tempfile to persist file credentials instead of locking (#1055)

### Other

- bump rust 1.84.1 (#1053)

## [0.22.2](https://github.com/conda/rattler/compare/rattler_networking-v0.22.1...rattler_networking-v0.22.2) - 2025-02-06

### Fixed

- create parent directories for file storage (#1045)

## [0.22.1](https://github.com/conda/rattler/compare/rattler_networking-v0.22.0...rattler_networking-v0.22.1) - 2025-02-03

### Added

- add S3 support (#1008)

## [0.22.0](https://github.com/conda/rattler/compare/rattler_networking-v0.21.10...rattler_networking-v0.22.0) - 2025-01-23

### Other

- Improve AuthenticationStorage (#1026)

## [0.21.10](https://github.com/conda/rattler/compare/rattler_networking-v0.21.9...rattler_networking-v0.21.10) - 2025-01-08

### Other

- update Cargo.toml dependencies

## [0.21.9](https://github.com/conda/rattler/compare/rattler_networking-v0.21.8...rattler_networking-v0.21.9) - 2024-12-17

### Other

- update Cargo.toml dependencies

## [0.21.8](https://github.com/conda/rattler/compare/rattler_networking-v0.21.7...rattler_networking-v0.21.8) - 2024-12-05

### Fixed

- GCS channels and add test ([#968](https://github.com/conda/rattler/pull/968))

## [0.21.7](https://github.com/conda/rattler/compare/rattler_networking-v0.21.6...rattler_networking-v0.21.7) - 2024-11-30

### Other

- update Cargo.toml dependencies

## [0.21.6](https://github.com/conda/rattler/compare/rattler_networking-v0.21.5...rattler_networking-v0.21.6) - 2024-11-18

### Added

- more setters for about.json and index.json  ([#939](https://github.com/conda/rattler/pull/939))

## [0.21.5](https://github.com/conda/rattler/compare/rattler_networking-v0.21.4...rattler_networking-v0.21.5) - 2024-11-04

### Other

- root constraint shouldnt crash ([#916](https://github.com/conda/rattler/pull/916))
- update all versions of packages ([#886](https://github.com/conda/rattler/pull/886))

## [0.21.4](https://github.com/conda/rattler/compare/rattler_networking-v0.21.3...rattler_networking-v0.21.4) - 2024-09-05

### Fixed
- typos ([#849](https://github.com/conda/rattler/pull/849))

## [0.21.3](https://github.com/conda/rattler/compare/rattler_networking-v0.21.2...rattler_networking-v0.21.3) - 2024-09-03

### Other
- make PackageCache multi-process safe ([#837](https://github.com/conda/rattler/pull/837))

## [0.21.2](https://github.com/conda/rattler/compare/rattler_networking-v0.21.1...rattler_networking-v0.21.2) - 2024-08-16

### Other
- bump keyring to 3.x to bump syn to 2.x ([#823](https://github.com/conda/rattler/pull/823))

## [0.21.1](https://github.com/conda/rattler/compare/rattler_networking-v0.21.0...rattler_networking-v0.21.1) - 2024-08-15

### Fixed
- move more links to the conda org from conda-incubator ([#816](https://github.com/conda/rattler/pull/816))
- use conda-incubator

### Other
- change links from conda-incubator to conda ([#813](https://github.com/conda/rattler/pull/813))
- update banner ([#808](https://github.com/conda/rattler/pull/808))

## [0.21.0](https://github.com/baszalmstra/rattler/compare/rattler_networking-v0.20.10...rattler_networking-v0.21.0) - 2024-08-02

### Fixed
- redact secrets in the `canonical_name` functions ([#801](https://github.com/baszalmstra/rattler/pull/801))

## [0.20.10](https://github.com/conda/rattler/compare/rattler_networking-v0.20.9...rattler_networking-v0.20.10) - 2024-07-15

### Other
- bump dependencies and remove unused ones ([#771](https://github.com/conda/rattler/pull/771))

## [0.20.9](https://github.com/conda/rattler/compare/rattler_networking-v0.20.8...rattler_networking-v0.20.9) - 2024-07-08

### Fixed
- errors should not contain trailing punctuation ([#763](https://github.com/conda/rattler/pull/763))

## [0.20.8](https://github.com/conda/rattler/compare/rattler_networking-v0.20.7...rattler_networking-v0.20.8) - 2024-05-27

### Other
- introducing the installer ([#664](https://github.com/conda/rattler/pull/664))

## [0.20.7](https://github.com/conda/rattler/compare/rattler_networking-v0.20.6...rattler_networking-v0.20.7) - 2024-05-14

### Added
- exclude repodata records based on timestamp ([#654](https://github.com/conda/rattler/pull/654))

## [0.20.6](https://github.com/conda/rattler/compare/rattler_networking-v0.20.5...rattler_networking-v0.20.6) - 2024-05-13

### Added
- high level repodata access ([#560](https://github.com/conda/rattler/pull/560))
- add AuthenticationStorage::from_file() ([#645](https://github.com/conda/rattler/pull/645))

### Other
- update README.md

## [0.20.5](https://github.com/conda/rattler/compare/rattler_networking-v0.20.4...rattler_networking-v0.20.5) - 2024-05-06

### Added
- respect `RATTLER_AUTH_FILE` when using AuthenticationStorage::default() ([#636](https://github.com/conda/rattler/pull/636))

## [0.20.4](https://github.com/conda/rattler/compare/rattler_networking-v0.20.3...rattler_networking-v0.20.4) - 2024-04-30

### Other
- bump py-rattler 0.5.0 ([#629](https://github.com/conda/rattler/pull/629))

## [0.20.3](https://github.com/conda/rattler/compare/rattler_networking-v0.20.2...rattler_networking-v0.20.3) - 2024-04-25

### Added
- Add GCS support for rattler auth ([#605](https://github.com/conda/rattler/pull/605))

## [0.20.2](https://github.com/conda/rattler/compare/rattler_networking-v0.20.1...rattler_networking-v0.20.2) - 2024-04-19

### Added
- enable zst support for OCI registry ([#601](https://github.com/conda/rattler/pull/601))

### Other
- update dependencies incl. reqwest ([#606](https://github.com/conda/rattler/pull/606))

## [0.20.1](https://github.com/baszalmstra/rattler/compare/rattler_networking-v0.20.0...rattler_networking-v0.20.1) - 2024-04-05

### Fixed
- run post-link scripts ([#574](https://github.com/baszalmstra/rattler/pull/574))
- properly fall back to netrc file ([#592](https://github.com/baszalmstra/rattler/pull/592))

## [0.20.0](https://github.com/conda/rattler/compare/rattler_networking-v0.19.2...rattler_networking-v0.20.0) - 2024-03-21

### Fixed
- implement cache for authentication filestorage backend ([#573](https://github.com/conda/rattler/pull/573))

## [0.19.2](https://github.com/conda/rattler/compare/rattler_networking-v0.19.1...rattler_networking-v0.19.2) - 2024-03-14

### Added
- add mirror handling and OCI mirror type ([#553](https://github.com/conda/rattler/pull/553))

### Other
- add pixi badge ([#563](https://github.com/conda/rattler/pull/563))

## [0.19.1](https://github.com/conda/rattler/compare/rattler_networking-v0.19.0...rattler_networking-v0.19.1) - 2024-03-06

### Fixed
- add snapshot test and use btreemap in file backend ([#543](https://github.com/conda/rattler/pull/543))

### Other
- every crate should have its own version ([#557](https://github.com/conda/rattler/pull/557))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler_networking-v0.18.0...rattler_networking-v0.19.0) - 2024-02-26

### Fixed
- redaction ([#539](https://github.com/baszalmstra/rattler/pull/539))
