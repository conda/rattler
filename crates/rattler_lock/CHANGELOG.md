# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.22.12](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.22.11...rattler_lock-v0.22.12) - 2024-06-06

### Added
- serialize packages from lock file individually ([#728](https://github.com/mamba-org/rattler/pull/728))

## [0.22.11](https://github.com/baszalmstra/rattler/compare/rattler_lock-v0.22.10...rattler_lock-v0.22.11) - 2024-06-04

### Other
- updated the following local packages: file_url, rattler_conda_types

## [0.22.10](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.22.9...rattler_lock-v0.22.10) - 2024-06-03

### Other
- updated the following local packages: rattler_conda_types

## [0.22.9](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.22.8...rattler_lock-v0.22.9) - 2024-05-28

### Added
- add run exports to package data ([#671](https://github.com/mamba-org/rattler/pull/671))

### Other
- bump ([#683](https://github.com/mamba-org/rattler/pull/683))

## [0.22.8](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.22.7...rattler_lock-v0.22.8) - 2024-05-27

### Added
- removed Ord and more ([#673](https://github.com/mamba-org/rattler/pull/673))
- always store purls as a key in lock file ([#669](https://github.com/mamba-org/rattler/pull/669))

## [0.22.7](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.22.6...rattler_lock-v0.22.7) - 2024-05-14

### Other
- bump pep crates ([#661](https://github.com/mamba-org/rattler/pull/661))

## [0.22.6](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.22.5...rattler_lock-v0.22.6) - 2024-05-13

### Added
- high level repodata access ([#560](https://github.com/mamba-org/rattler/pull/560))

### Other
- update README.md

## [0.22.5](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.22.4...rattler_lock-v0.22.5) - 2024-05-06

### Other
- updated the following local packages: rattler_conda_types

## [0.22.4](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.22.3...rattler_lock-v0.22.4) - 2024-04-30

### Added
- adds pypi indexes to the lock-file ([#626](https://github.com/mamba-org/rattler/pull/626))

## [0.22.3](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.22.2...rattler_lock-v0.22.3) - 2024-04-25

### Fixed
- compare `UrlOrPath` ([#618](https://github.com/mamba-org/rattler/pull/618))
- parse absolute paths on Windows correctly in lockfiles ([#616](https://github.com/mamba-org/rattler/pull/616))

## [0.22.2](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.22.1...rattler_lock-v0.22.2) - 2024-04-19

### Other
- update dependencies incl. reqwest ([#606](https://github.com/mamba-org/rattler/pull/606))

## [0.22.1](https://github.com/baszalmstra/rattler/compare/rattler_lock-v0.22.0...rattler_lock-v0.22.1) - 2024-04-05

### Other
- updated the following local packages: rattler_conda_types

## [0.22.0](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.21.0...rattler_lock-v0.22.0) - 2024-03-30

### Added
- editable pypi packages ([#581](https://github.com/mamba-org/rattler/pull/581))

## [0.21.0](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.20.2...rattler_lock-v0.21.0) - 2024-03-21

### Added
- allow passing pypi paths ([#572](https://github.com/mamba-org/rattler/pull/572))

## [0.20.2](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.20.1...rattler_lock-v0.20.2) - 2024-03-14

### Other
- add pixi badge ([#563](https://github.com/mamba-org/rattler/pull/563))

## [0.20.1](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.20.0...rattler_lock-v0.20.1) - 2024-03-08

### Fixed
- chrono deprecation warnings ([#558](https://github.com/mamba-org/rattler/pull/558))

## [0.20.0](https://github.com/mamba-org/rattler/compare/rattler_lock-v0.19.0...rattler_lock-v0.20.0) - 2024-03-06

### Added
- sort extras by name and urls by filename ([#540](https://github.com/mamba-org/rattler/pull/540))

### Fixed
- dont use workspace dependencies for local crates ([#546](https://github.com/mamba-org/rattler/pull/546))

### Other
- every crate should have its own version ([#557](https://github.com/mamba-org/rattler/pull/557))
- bump pep508_rs and pep440_rs ([#549](https://github.com/mamba-org/rattler/pull/549))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler_lock-v0.18.0...rattler_lock-v0.19.0) - 2024-02-26
