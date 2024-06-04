# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.25.2](https://github.com/baszalmstra/rattler/compare/rattler_conda_types-v0.25.1...rattler_conda_types-v0.25.2) - 2024-06-04

### Added
- parse url and path as matchspec ([#704](https://github.com/baszalmstra/rattler/pull/704))

### Fixed
- issue 722 ([#723](https://github.com/baszalmstra/rattler/pull/723))

### Other
- remove lfs ([#512](https://github.com/baszalmstra/rattler/pull/512))
- move the cache tooling into its own crate for reuse downstream ([#721](https://github.com/baszalmstra/rattler/pull/721))

## [0.25.1](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.25.0...rattler_conda_types-v0.25.1) - 2024-06-03

### Added
- add a `with_alpha` function that adds `0a0` to the version ([#696](https://github.com/mamba-org/rattler/pull/696))

## [0.25.0](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.24.0...rattler_conda_types-v0.25.0) - 2024-05-28

### Added
- when bumping, extend versions with `0` to match the bump request ([#695](https://github.com/mamba-org/rattler/pull/695))
- extend tests and handle characters better when bumping versions ([#694](https://github.com/mamba-org/rattler/pull/694))
- add a function to extend version with `0s` ([#689](https://github.com/mamba-org/rattler/pull/689))
- add run exports to package data ([#671](https://github.com/mamba-org/rattler/pull/671))

### Fixed
- lenient parsing of 2023.*.* ([#688](https://github.com/mamba-org/rattler/pull/688))
- VersionSpec starts with, with trailing zeros ([#686](https://github.com/mamba-org/rattler/pull/686))

### Other
- move bump implementation to bump.rs and simplify tests ([#692](https://github.com/mamba-org/rattler/pull/692))

## [0.24.0](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.23.1...rattler_conda_types-v0.24.0) - 2024-05-27

### Added
- removed Ord and more ([#673](https://github.com/mamba-org/rattler/pull/673))
- always store purls as a key in lock file ([#669](https://github.com/mamba-org/rattler/pull/669))
- add solve strategies ([#660](https://github.com/mamba-org/rattler/pull/660))

### Fixed
- make topological sorting support fully cyclic dependencies ([#678](https://github.com/mamba-org/rattler/pull/678))

## [0.23.1](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.23.0...rattler_conda_types-v0.23.1) - 2024-05-14

### Added
- exclude repodata records based on timestamp ([#654](https://github.com/mamba-org/rattler/pull/654))

## [0.23.0](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.22.1...rattler_conda_types-v0.23.0) - 2024-05-13

### Added
- high level repodata access ([#560](https://github.com/mamba-org/rattler/pull/560))

### Other
- update README.md

## [0.22.1](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.22.0...rattler_conda_types-v0.22.1) - 2024-05-06

### Added
- expose `*Record.noarch` in Python bindings ([#635](https://github.com/mamba-org/rattler/pull/635))

## [0.22.0](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.21.0...rattler_conda_types-v0.22.0) - 2024-04-25

### Added
- add support for extracting prefix placeholder data to PathsEntry ([#614](https://github.com/mamba-org/rattler/pull/614))

## [0.21.0](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.20.5...rattler_conda_types-v0.21.0) - 2024-04-19

### Added
- make root dir configurable in channel config ([#602](https://github.com/mamba-org/rattler/pull/602))

### Fixed
- better value for `link` field ([#610](https://github.com/mamba-org/rattler/pull/610))

### Other
- update dependencies incl. reqwest ([#606](https://github.com/mamba-org/rattler/pull/606))

## [0.20.5](https://github.com/baszalmstra/rattler/compare/rattler_conda_types-v0.20.4...rattler_conda_types-v0.20.5) - 2024-04-05

### Fixed
- run post-link scripts ([#574](https://github.com/baszalmstra/rattler/pull/574))

## [0.20.4](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.20.3...rattler_conda_types-v0.20.4) - 2024-03-30

### Fixed
- matchspec empty namespace and channel cannonical name ([#582](https://github.com/mamba-org/rattler/pull/582))

## [0.20.3](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.20.2...rattler_conda_types-v0.20.3) - 2024-03-21

### Fixed
- allow not starts with in strict mode ([#577](https://github.com/mamba-org/rattler/pull/577))

## [0.20.2](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.20.1...rattler_conda_types-v0.20.2) - 2024-03-14

### Other
- add pixi badge ([#563](https://github.com/mamba-org/rattler/pull/563))

## [0.20.1](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.20.0...rattler_conda_types-v0.20.1) - 2024-03-08

### Fixed
- chrono deprecation warnings ([#558](https://github.com/mamba-org/rattler/pull/558))

## [0.20.0](https://github.com/mamba-org/rattler/compare/rattler_conda_types-v0.19.0...rattler_conda_types-v0.20.0) - 2024-03-06

### Added
- [**breaking**] optional strict parsing of matchspec and versionspec ([#552](https://github.com/mamba-org/rattler/pull/552))

### Fixed
- patch unsupported glob operators ([#551](https://github.com/mamba-org/rattler/pull/551))
- dont use workspace dependencies for local crates ([#546](https://github.com/mamba-org/rattler/pull/546))

### Other
- every crate should have its own version ([#557](https://github.com/mamba-org/rattler/pull/557))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler_conda_types-v0.18.0...rattler_conda_types-v0.19.0) - 2024-02-26

### Fixed
- Fix arch for osx-arm64 and win-arm64 ([#528](https://github.com/baszalmstra/rattler/pull/528))
- Channel name display ([#531](https://github.com/baszalmstra/rattler/pull/531))
