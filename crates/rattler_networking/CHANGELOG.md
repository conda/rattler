# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.20.8](https://github.com/mamba-org/rattler/compare/rattler_networking-v0.20.7...rattler_networking-v0.20.8) - 2024-05-27

### Other
- introducing the installer ([#664](https://github.com/mamba-org/rattler/pull/664))

## [0.20.7](https://github.com/mamba-org/rattler/compare/rattler_networking-v0.20.6...rattler_networking-v0.20.7) - 2024-05-14

### Added
- exclude repodata records based on timestamp ([#654](https://github.com/mamba-org/rattler/pull/654))

## [0.20.6](https://github.com/mamba-org/rattler/compare/rattler_networking-v0.20.5...rattler_networking-v0.20.6) - 2024-05-13

### Added
- high level repodata access ([#560](https://github.com/mamba-org/rattler/pull/560))
- add AuthenticationStorage::from_file() ([#645](https://github.com/mamba-org/rattler/pull/645))

### Other
- update README.md

## [0.20.5](https://github.com/mamba-org/rattler/compare/rattler_networking-v0.20.4...rattler_networking-v0.20.5) - 2024-05-06

### Added
- respect `RATTLER_AUTH_FILE` when using AuthenticationStorage::default() ([#636](https://github.com/mamba-org/rattler/pull/636))

## [0.20.4](https://github.com/mamba-org/rattler/compare/rattler_networking-v0.20.3...rattler_networking-v0.20.4) - 2024-04-30

### Other
- bump py-rattler 0.5.0 ([#629](https://github.com/mamba-org/rattler/pull/629))

## [0.20.3](https://github.com/mamba-org/rattler/compare/rattler_networking-v0.20.2...rattler_networking-v0.20.3) - 2024-04-25

### Added
- Add GCS support for rattler auth ([#605](https://github.com/mamba-org/rattler/pull/605))

## [0.20.2](https://github.com/mamba-org/rattler/compare/rattler_networking-v0.20.1...rattler_networking-v0.20.2) - 2024-04-19

### Added
- enable zst support for OCI registry ([#601](https://github.com/mamba-org/rattler/pull/601))

### Other
- update dependencies incl. reqwest ([#606](https://github.com/mamba-org/rattler/pull/606))

## [0.20.1](https://github.com/baszalmstra/rattler/compare/rattler_networking-v0.20.0...rattler_networking-v0.20.1) - 2024-04-05

### Fixed
- run post-link scripts ([#574](https://github.com/baszalmstra/rattler/pull/574))
- properly fall back to netrc file ([#592](https://github.com/baszalmstra/rattler/pull/592))

## [0.20.0](https://github.com/mamba-org/rattler/compare/rattler_networking-v0.19.2...rattler_networking-v0.20.0) - 2024-03-21

### Fixed
- implement cache for authentication filestorage backend ([#573](https://github.com/mamba-org/rattler/pull/573))

## [0.19.2](https://github.com/mamba-org/rattler/compare/rattler_networking-v0.19.1...rattler_networking-v0.19.2) - 2024-03-14

### Added
- add mirror handling and OCI mirror type ([#553](https://github.com/mamba-org/rattler/pull/553))

### Other
- add pixi badge ([#563](https://github.com/mamba-org/rattler/pull/563))

## [0.19.1](https://github.com/mamba-org/rattler/compare/rattler_networking-v0.19.0...rattler_networking-v0.19.1) - 2024-03-06

### Fixed
- add snapshot test and use btreemap in file backend ([#543](https://github.com/mamba-org/rattler/pull/543))

### Other
- every crate should have its own version ([#557](https://github.com/mamba-org/rattler/pull/557))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler_networking-v0.18.0...rattler_networking-v0.19.0) - 2024-02-26

### Fixed
- redaction ([#539](https://github.com/baszalmstra/rattler/pull/539))
