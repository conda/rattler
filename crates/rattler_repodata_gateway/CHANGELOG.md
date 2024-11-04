# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.21.19](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.18...rattler_repodata_gateway-v0.21.19) - 2024-11-04

### Other

- update Cargo.toml dependencies

## [0.21.18](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.17...rattler_repodata_gateway-v0.21.18) - 2024-10-21

### Other

- updated the following local packages: file_url, rattler_cache

## [0.21.17](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.16...rattler_repodata_gateway-v0.21.17) - 2024-10-07

### Other

- stream JLAP repodata writes ([#891](https://github.com/conda/rattler/pull/891))

## [0.21.16](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.15...rattler_repodata_gateway-v0.21.16) - 2024-10-03

### Other

- updated the following local packages: rattler_conda_types

## [0.21.15](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.14...rattler_repodata_gateway-v0.21.15) - 2024-10-01

### Other

- start using fs-err in repodata_gateway ([#877](https://github.com/conda/rattler/pull/877))

## [0.21.14](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.13...rattler_repodata_gateway-v0.21.14) - 2024-09-23

### Other

- updated the following local packages: rattler_conda_types

## [0.21.13](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.12...rattler_repodata_gateway-v0.21.13) - 2024-09-09

### Other

- updated the following local packages: rattler_conda_types

## [0.21.12](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.11...rattler_repodata_gateway-v0.21.12) - 2024-09-05

### Fixed
- *(gateway)* clear subdir cache based on `base_url` ([#852](https://github.com/conda/rattler/pull/852))
- typos ([#849](https://github.com/conda/rattler/pull/849))

## [0.21.11](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.10...rattler_repodata_gateway-v0.21.11) - 2024-09-03

### Fixed
- allow `gcs://` and `oci://` in gateway ([#845](https://github.com/conda/rattler/pull/845))

## [0.21.10](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.9...rattler_repodata_gateway-v0.21.10) - 2024-09-03

### Other
- make PackageCache multi-process safe ([#837](https://github.com/conda/rattler/pull/837))

## [0.21.9](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.8...rattler_repodata_gateway-v0.21.9) - 2024-09-02

### Other
- updated the following local packages: rattler_conda_types

## [0.21.8](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.7...rattler_repodata_gateway-v0.21.8) - 2024-08-16

### Other
- updated the following local packages: rattler_networking

## [0.21.7](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.6...rattler_repodata_gateway-v0.21.7) - 2024-08-16

### Added
- add package names api for gateway ([#819](https://github.com/conda/rattler/pull/819))

## [0.21.6](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.5...rattler_repodata_gateway-v0.21.6) - 2024-08-15

### Fixed
- move more links to the conda org from conda-incubator ([#816](https://github.com/conda/rattler/pull/816))

### Other
- change links from conda-incubator to conda ([#813](https://github.com/conda/rattler/pull/813))
- update banner ([#808](https://github.com/conda/rattler/pull/808))

## [0.21.5](https://github.com/baszalmstra/rattler/compare/rattler_repodata_gateway-v0.21.4...rattler_repodata_gateway-v0.21.5) - 2024-08-06

### Other
- updated the following local packages: rattler_conda_types

## [0.21.4](https://github.com/baszalmstra/rattler/compare/rattler_repodata_gateway-v0.21.3...rattler_repodata_gateway-v0.21.4) - 2024-08-02

### Fixed
- redact secrets in the `canonical_name` functions ([#801](https://github.com/baszalmstra/rattler/pull/801))

### Other
- mark some crates 1.0 ([#789](https://github.com/baszalmstra/rattler/pull/789))

## [0.21.3](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.2...rattler_repodata_gateway-v0.21.3) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.21.2](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.1...rattler_repodata_gateway-v0.21.2) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.21.1](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.21.0...rattler_repodata_gateway-v0.21.1) - 2024-07-15

### Other
- bump dependencies and remove unused ones ([#771](https://github.com/conda/rattler/pull/771))

## [0.21.0](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.20.5...rattler_repodata_gateway-v0.21.0) - 2024-07-08

### Added
- improve error message when parsing file name ([#757](https://github.com/conda/rattler/pull/757))
- add direct url repodata building ([#725](https://github.com/conda/rattler/pull/725))
- add shards_base_url and write shards atomically ([#747](https://github.com/conda/rattler/pull/747))

### Fixed
- direct_url query for windows ([#768](https://github.com/conda/rattler/pull/768))
- Fix GatewayQuery.query to filter records based on provided specs ([#756](https://github.com/conda/rattler/pull/756))
- run clippy on all targets ([#762](https://github.com/conda/rattler/pull/762))
- allow empty json repodata ([#745](https://github.com/conda/rattler/pull/745))

### Other
- document gateway features ([#737](https://github.com/conda/rattler/pull/737))

## [0.20.5](https://github.com/baszalmstra/rattler/compare/rattler_repodata_gateway-v0.20.4...rattler_repodata_gateway-v0.20.5) - 2024-06-04

### Other
- remove lfs ([#512](https://github.com/baszalmstra/rattler/pull/512))

## [0.20.4](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.20.3...rattler_repodata_gateway-v0.20.4) - 2024-06-03

### Other
- updated the following local packages: rattler_conda_types, rattler_conda_types

## [0.20.3](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.20.2...rattler_repodata_gateway-v0.20.3) - 2024-05-28

### Other
- updated the following local packages: rattler_conda_types, rattler_conda_types

## [0.20.2](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.20.1...rattler_repodata_gateway-v0.20.2) - 2024-05-27

### Fixed
- result grouped by subdir instead of channel ([#666](https://github.com/conda/rattler/pull/666))

### Other
- introducing the installer ([#664](https://github.com/conda/rattler/pull/664))

## [0.20.1](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.20.0...rattler_repodata_gateway-v0.20.1) - 2024-05-14

### Added
- exclude repodata records based on timestamp ([#654](https://github.com/conda/rattler/pull/654))

## [0.20.0](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.19.11...rattler_repodata_gateway-v0.20.0) - 2024-05-13

### Added
- add clear subdir cache function to repodata gateway ([#650](https://github.com/conda/rattler/pull/650))
- high level repodata access ([#560](https://github.com/conda/rattler/pull/560))

### Other
- update README.md

## [0.19.11](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.19.10...rattler_repodata_gateway-v0.19.11) - 2024-05-06

### Other
- updated the following local packages: rattler_conda_types, rattler_networking

## [0.19.10](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.19.9...rattler_repodata_gateway-v0.19.10) - 2024-04-30

### Added
- create SparseRepoData from byte slices ([#624](https://github.com/conda/rattler/pull/624))

## [0.19.9](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.19.8...rattler_repodata_gateway-v0.19.9) - 2024-04-25

### Other
- updated the following local packages: rattler_networking

## [0.19.8](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.19.7...rattler_repodata_gateway-v0.19.8) - 2024-04-25

### Other
- updated the following local packages: rattler_conda_types

## [0.19.7](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.19.6...rattler_repodata_gateway-v0.19.7) - 2024-04-19

### Added
- make root dir configurable in channel config ([#602](https://github.com/conda/rattler/pull/602))

### Other
- update dependencies incl. reqwest ([#606](https://github.com/conda/rattler/pull/606))

## [0.19.6](https://github.com/baszalmstra/rattler/compare/rattler_repodata_gateway-v0.19.5...rattler_repodata_gateway-v0.19.6) - 2024-04-05

### Other
- updated the following local packages: rattler_conda_types, rattler_networking

## [0.19.5](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.19.4...rattler_repodata_gateway-v0.19.5) - 2024-03-30

### Other
- updated the following local packages: rattler_conda_types

## [0.19.4](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.19.3...rattler_repodata_gateway-v0.19.4) - 2024-03-21

### Other
- updated the following local packages: rattler_conda_types, rattler_networking

## [0.19.3](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.19.2...rattler_repodata_gateway-v0.19.3) - 2024-03-14

### Other
- add pixi badge ([#563](https://github.com/conda/rattler/pull/563))

## [0.19.2](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.19.1...rattler_repodata_gateway-v0.19.2) - 2024-03-08

### Fixed
- chrono deprecation warnings ([#558](https://github.com/conda/rattler/pull/558))

## [0.19.1](https://github.com/conda/rattler/compare/rattler_repodata_gateway-v0.19.0...rattler_repodata_gateway-v0.19.1) - 2024-03-06

### Fixed
- correct condition to downweigh track-feature packages ([#545](https://github.com/conda/rattler/pull/545))
- dont use workspace dependencies for local crates ([#546](https://github.com/conda/rattler/pull/546))

### Other
- every crate should have its own version ([#557](https://github.com/conda/rattler/pull/557))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler_repodata_gateway-v0.18.0...rattler_repodata_gateway-v0.19.0) - 2024-02-26
