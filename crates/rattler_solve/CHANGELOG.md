# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.0](https://github.com/conda/rattler/compare/rattler_solve-v1.1.0...rattler_solve-v1.2.0) - 2024-11-04

### Added

- use python_site_packages_path field when available for installing noarch: python packages, CEP-17 ([#909](https://github.com/conda/rattler/pull/909))
- Add `PackageRecord::validate` function ([#911](https://github.com/conda/rattler/pull/911))

### Other

- root constraint shouldnt crash ([#916](https://github.com/conda/rattler/pull/916))
- release ([#903](https://github.com/conda/rattler/pull/903))

## [1.1.0](https://github.com/conda/rattler/compare/rattler_solve-v1.0.10...rattler_solve-v1.1.0) - 2024-10-07

### Added

- add sorting bench and makes test same as feature test ([#897](https://github.com/conda/rattler/pull/897))

### Fixed

- sorting test should also load dependencies ([#896](https://github.com/conda/rattler/pull/896))

### Other

- add snapshot tests to verify solver sorting order ([#895](https://github.com/conda/rattler/pull/895))

## [1.0.10](https://github.com/conda/rattler/compare/rattler_solve-v1.0.9...rattler_solve-v1.0.10) - 2024-10-03

### Other

- updated the following local packages: rattler_conda_types

## [1.0.9](https://github.com/conda/rattler/compare/rattler_solve-v1.0.8...rattler_solve-v1.0.9) - 2024-10-01

### Other

- update resolvo ([#881](https://github.com/conda/rattler/pull/881))

## [1.0.8](https://github.com/conda/rattler/compare/rattler_solve-v1.0.7...rattler_solve-v1.0.8) - 2024-09-23

### Other

- updated the following local packages: rattler_conda_types

## [1.0.7](https://github.com/conda/rattler/compare/rattler_solve-v1.0.6...rattler_solve-v1.0.7) - 2024-09-09

### Other

- bump resolvo 0.8.0 ([#857](https://github.com/conda/rattler/pull/857))

## [1.0.6](https://github.com/conda/rattler/compare/rattler_solve-v1.0.5...rattler_solve-v1.0.6) - 2024-09-05

### Fixed
- remaining typos ([#854](https://github.com/conda/rattler/pull/854))
- typos ([#849](https://github.com/conda/rattler/pull/849))

## [1.0.5](https://github.com/conda/rattler/compare/rattler_solve-v1.0.4...rattler_solve-v1.0.5) - 2024-09-03

### Other
- make PackageCache multi-process safe ([#837](https://github.com/conda/rattler/pull/837))

## [1.0.4](https://github.com/conda/rattler/compare/rattler_solve-v1.0.3...rattler_solve-v1.0.4) - 2024-09-02

### Fixed
- Redact spec channel before comparing it with repodata channel  ([#831](https://github.com/conda/rattler/pull/831))

### Other
- Remove note that only libsolv is supported ([#832](https://github.com/conda/rattler/pull/832))

## [1.0.3](https://github.com/conda/rattler/compare/rattler_solve-v1.0.2...rattler_solve-v1.0.3) - 2024-08-15

### Fixed
- move more links to the conda org from conda-incubator ([#816](https://github.com/conda/rattler/pull/816))

### Other
- change links from conda-incubator to conda ([#813](https://github.com/conda/rattler/pull/813))
- bump resolvo to 0.7.0 ([#805](https://github.com/conda/rattler/pull/805))
- update banner ([#808](https://github.com/conda/rattler/pull/808))

## [1.0.2](https://github.com/baszalmstra/rattler/compare/rattler_solve-v1.0.1...rattler_solve-v1.0.2) - 2024-08-06

### Other
- updated the following local packages: rattler_conda_types

## [1.0.1](https://github.com/baszalmstra/rattler/compare/rattler_solve-v1.0.0...rattler_solve-v1.0.1) - 2024-08-02

### Fixed
- constraints on virtual packages were ignored ([#795](https://github.com/baszalmstra/rattler/pull/795))

## [0.25.3](https://github.com/conda/rattler/compare/rattler_solve-v0.25.2...rattler_solve-v0.25.3) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.25.2](https://github.com/conda/rattler/compare/rattler_solve-v0.25.1...rattler_solve-v0.25.2) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.25.1](https://github.com/conda/rattler/compare/rattler_solve-v0.25.0...rattler_solve-v0.25.1) - 2024-07-15

### Other
- update Cargo.toml dependencies

## [0.25.0](https://github.com/conda/rattler/compare/rattler_solve-v0.24.2...rattler_solve-v0.25.0) - 2024-07-08

### Added
- add direct url repodata building ([#725](https://github.com/conda/rattler/pull/725))
- add tool to generate resolvo snapshots ([#741](https://github.com/conda/rattler/pull/741))

### Fixed
- run clippy on all targets ([#762](https://github.com/conda/rattler/pull/762))
- This fixes parsing of `ray[default,data] >=2.9.0,<3.0.0` ([#732](https://github.com/conda/rattler/pull/732))

### Other
- bump resolvo to 0.6.0 ([#733](https://github.com/conda/rattler/pull/733))
- Document all features on docs.rs ([#734](https://github.com/conda/rattler/pull/734))

## [0.24.2](https://github.com/conda/rattler/compare/rattler_solve-v0.24.1...rattler_solve-v0.24.2) - 2024-06-06

### Added
- serialize packages from lock file individually ([#728](https://github.com/conda/rattler/pull/728))

## [0.24.1](https://github.com/baszalmstra/rattler/compare/rattler_solve-v0.24.0...rattler_solve-v0.24.1) - 2024-06-04

### Other
- updated the following local packages: rattler_conda_types

## [0.24.0](https://github.com/conda/rattler/compare/rattler_solve-v0.23.2...rattler_solve-v0.24.0) - 2024-06-03

### Added
- add constraints to solve ([#713](https://github.com/conda/rattler/pull/713))

## [0.23.2](https://github.com/conda/rattler/compare/rattler_solve-v0.23.1...rattler_solve-v0.23.2) - 2024-05-28

### Fixed
- ChannelPriority implements Debug ([#701](https://github.com/conda/rattler/pull/701))

## [0.23.1](https://github.com/conda/rattler/compare/rattler_solve-v0.23.0...rattler_solve-v0.23.1) - 2024-05-28

### Added
- add run exports to package data ([#671](https://github.com/conda/rattler/pull/671))

### Other
- enable serialization of enums ([#698](https://github.com/conda/rattler/pull/698))

## [0.23.0](https://github.com/conda/rattler/compare/rattler_solve-v0.22.0...rattler_solve-v0.23.0) - 2024-05-27

### Added
- removed Ord and more ([#673](https://github.com/conda/rattler/pull/673))
- always store purls as a key in lock file ([#669](https://github.com/conda/rattler/pull/669))
- add solve strategies ([#660](https://github.com/conda/rattler/pull/660))

### Fixed
- result grouped by subdir instead of channel ([#666](https://github.com/conda/rattler/pull/666))

### Other
- introducing the installer ([#664](https://github.com/conda/rattler/pull/664))

## [0.22.0](https://github.com/conda/rattler/compare/rattler_solve-v0.21.2...rattler_solve-v0.22.0) - 2024-05-14

### Added
- exclude repodata records based on timestamp ([#654](https://github.com/conda/rattler/pull/654))

## [0.21.2](https://github.com/conda/rattler/compare/rattler_solve-v0.21.1...rattler_solve-v0.21.2) - 2024-05-13

### Added
- high level repodata access ([#560](https://github.com/conda/rattler/pull/560))

### Other
- update README.md

## [0.21.1](https://github.com/conda/rattler/compare/rattler_solve-v0.21.0...rattler_solve-v0.21.1) - 2024-05-06

### Other
- updated the following local packages: rattler_conda_types

## [0.21.0](https://github.com/conda/rattler/compare/rattler_solve-v0.20.7...rattler_solve-v0.21.0) - 2024-04-25

### Added
- add channel priority to solve task and expose to python solve ([#598](https://github.com/conda/rattler/pull/598))

## [0.20.7](https://github.com/conda/rattler/compare/rattler_solve-v0.20.6...rattler_solve-v0.20.7) - 2024-04-25

### Other
- updated the following local packages: rattler_conda_types

## [0.20.6](https://github.com/conda/rattler/compare/rattler_solve-v0.20.5...rattler_solve-v0.20.6) - 2024-04-19

### Added
- make root dir configurable in channel config ([#602](https://github.com/conda/rattler/pull/602))

### Other
- update dependencies incl. reqwest ([#606](https://github.com/conda/rattler/pull/606))

## [0.20.5](https://github.com/baszalmstra/rattler/compare/rattler_solve-v0.20.4...rattler_solve-v0.20.5) - 2024-04-05

### Other
- updated the following local packages: rattler_conda_types

## [0.20.4](https://github.com/conda/rattler/compare/rattler_solve-v0.20.3...rattler_solve-v0.20.4) - 2024-03-30

### Other
- updated the following local packages: rattler_conda_types

## [0.20.3](https://github.com/conda/rattler/compare/rattler_solve-v0.20.2...rattler_solve-v0.20.3) - 2024-03-21

### Other
- updated the following local packages: rattler_conda_types

## [0.20.2](https://github.com/conda/rattler/compare/rattler_solve-v0.20.1...rattler_solve-v0.20.2) - 2024-03-14

### Other
- add pixi badge ([#563](https://github.com/conda/rattler/pull/563))

## [0.20.1](https://github.com/conda/rattler/compare/rattler_solve-v0.20.0...rattler_solve-v0.20.1) - 2024-03-08

### Other
- update Cargo.toml dependencies

## [0.20.0](https://github.com/conda/rattler/compare/rattler_solve-v0.19.0...rattler_solve-v0.20.0) - 2024-03-06

### Added
- [**breaking**] optional strict parsing of matchspec and versionspec ([#552](https://github.com/conda/rattler/pull/552))

### Fixed
- removal of multiple packages that clobber each other ([#556](https://github.com/conda/rattler/pull/556))
- correct condition to downweigh track-feature packages ([#545](https://github.com/conda/rattler/pull/545))
- dont use workspace dependencies for local crates ([#546](https://github.com/conda/rattler/pull/546))

### Other
- every crate should have its own version ([#557](https://github.com/conda/rattler/pull/557))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler_solve-v0.18.0...rattler_solve-v0.19.0) - 2024-02-26
