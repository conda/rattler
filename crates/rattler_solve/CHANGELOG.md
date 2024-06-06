# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.24.2](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.24.1...rattler_solve-v0.24.2) - 2024-06-06

### Added
- serialize packages from lock file individually ([#728](https://github.com/mamba-org/rattler/pull/728))

## [0.24.1](https://github.com/baszalmstra/rattler/compare/rattler_solve-v0.24.0...rattler_solve-v0.24.1) - 2024-06-04

### Other
- updated the following local packages: rattler_conda_types

## [0.24.0](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.23.2...rattler_solve-v0.24.0) - 2024-06-03

### Added
- add constraints to solve ([#713](https://github.com/mamba-org/rattler/pull/713))

## [0.23.2](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.23.1...rattler_solve-v0.23.2) - 2024-05-28

### Fixed
- ChannelPriority implements Debug ([#701](https://github.com/mamba-org/rattler/pull/701))

## [0.23.1](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.23.0...rattler_solve-v0.23.1) - 2024-05-28

### Added
- add run exports to package data ([#671](https://github.com/mamba-org/rattler/pull/671))

### Other
- enable serialization of enums ([#698](https://github.com/mamba-org/rattler/pull/698))

## [0.23.0](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.22.0...rattler_solve-v0.23.0) - 2024-05-27

### Added
- removed Ord and more ([#673](https://github.com/mamba-org/rattler/pull/673))
- always store purls as a key in lock file ([#669](https://github.com/mamba-org/rattler/pull/669))
- add solve strategies ([#660](https://github.com/mamba-org/rattler/pull/660))

### Fixed
- result grouped by subdir instead of channel ([#666](https://github.com/mamba-org/rattler/pull/666))

### Other
- introducing the installer ([#664](https://github.com/mamba-org/rattler/pull/664))

## [0.22.0](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.21.2...rattler_solve-v0.22.0) - 2024-05-14

### Added
- exclude repodata records based on timestamp ([#654](https://github.com/mamba-org/rattler/pull/654))

## [0.21.2](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.21.1...rattler_solve-v0.21.2) - 2024-05-13

### Added
- high level repodata access ([#560](https://github.com/mamba-org/rattler/pull/560))

### Other
- update README.md

## [0.21.1](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.21.0...rattler_solve-v0.21.1) - 2024-05-06

### Other
- updated the following local packages: rattler_conda_types

## [0.21.0](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.20.7...rattler_solve-v0.21.0) - 2024-04-25

### Added
- add channel priority to solve task and expose to python solve ([#598](https://github.com/mamba-org/rattler/pull/598))

## [0.20.7](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.20.6...rattler_solve-v0.20.7) - 2024-04-25

### Other
- updated the following local packages: rattler_conda_types

## [0.20.6](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.20.5...rattler_solve-v0.20.6) - 2024-04-19

### Added
- make root dir configurable in channel config ([#602](https://github.com/mamba-org/rattler/pull/602))

### Other
- update dependencies incl. reqwest ([#606](https://github.com/mamba-org/rattler/pull/606))

## [0.20.5](https://github.com/baszalmstra/rattler/compare/rattler_solve-v0.20.4...rattler_solve-v0.20.5) - 2024-04-05

### Other
- updated the following local packages: rattler_conda_types

## [0.20.4](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.20.3...rattler_solve-v0.20.4) - 2024-03-30

### Other
- updated the following local packages: rattler_conda_types

## [0.20.3](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.20.2...rattler_solve-v0.20.3) - 2024-03-21

### Other
- updated the following local packages: rattler_conda_types

## [0.20.2](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.20.1...rattler_solve-v0.20.2) - 2024-03-14

### Other
- add pixi badge ([#563](https://github.com/mamba-org/rattler/pull/563))

## [0.20.1](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.20.0...rattler_solve-v0.20.1) - 2024-03-08

### Other
- update Cargo.toml dependencies

## [0.20.0](https://github.com/mamba-org/rattler/compare/rattler_solve-v0.19.0...rattler_solve-v0.20.0) - 2024-03-06

### Added
- [**breaking**] optional strict parsing of matchspec and versionspec ([#552](https://github.com/mamba-org/rattler/pull/552))

### Fixed
- removal of multiple packages that clobber each other ([#556](https://github.com/mamba-org/rattler/pull/556))
- correct condition to downweigh track-feature packages ([#545](https://github.com/mamba-org/rattler/pull/545))
- dont use workspace dependencies for local crates ([#546](https://github.com/mamba-org/rattler/pull/546))

### Other
- every crate should have its own version ([#557](https://github.com/mamba-org/rattler/pull/557))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler_solve-v0.18.0...rattler_solve-v0.19.0) - 2024-02-26
