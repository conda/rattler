# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.26.4](https://github.com/mamba-org/rattler/compare/rattler-v0.26.3...rattler-v0.26.4) - 2024-06-06

### Other
- updated the following local packages: rattler_shell

## [0.26.3](https://github.com/baszalmstra/rattler/compare/rattler-v0.26.2...rattler-v0.26.3) - 2024-06-04

### Other
- remove lfs ([#512](https://github.com/baszalmstra/rattler/pull/512))
- move the cache tooling into its own crate for reuse downstream ([#721](https://github.com/baszalmstra/rattler/pull/721))

## [0.26.2](https://github.com/mamba-org/rattler/compare/rattler-v0.26.1...rattler-v0.26.2) - 2024-06-03

### Other
- updated the following local packages: rattler_conda_types, rattler_package_streaming

## [0.26.1](https://github.com/mamba-org/rattler/compare/rattler-v0.26.0...rattler-v0.26.1) - 2024-05-28

### Other
- updated the following local packages: rattler_conda_types

## [0.26.0](https://github.com/mamba-org/rattler/compare/rattler-v0.25.0...rattler-v0.26.0) - 2024-05-27

### Fixed
- improve progress bar duration display ([#680](https://github.com/mamba-org/rattler/pull/680))

### Other
- introducing the installer ([#664](https://github.com/mamba-org/rattler/pull/664))
- create directories up front ([#533](https://github.com/mamba-org/rattler/pull/533))

## [0.25.0](https://github.com/mamba-org/rattler/compare/rattler-v0.24.1...rattler-v0.25.0) - 2024-05-14

### Added
- exclude repodata records based on timestamp ([#654](https://github.com/mamba-org/rattler/pull/654))

### Other
- use semaphore for install driver ([#653](https://github.com/mamba-org/rattler/pull/653))

## [0.24.1](https://github.com/mamba-org/rattler/compare/rattler-v0.24.0...rattler-v0.24.1) - 2024-05-13

### Other
- updated the following local packages: rattler_conda_types, rattler_digest, rattler_package_streaming, rattler_networking

## [0.24.0](https://github.com/mamba-org/rattler/compare/rattler-v0.23.2...rattler-v0.24.0) - 2024-05-06

### Fixed
- use the output of `readlink` as hash for softlinks ([#643](https://github.com/mamba-org/rattler/pull/643))
- sha computation of symlinks was failing sometimes ([#641](https://github.com/mamba-org/rattler/pull/641))

## [0.23.2](https://github.com/mamba-org/rattler/compare/rattler-v0.23.1...rattler-v0.23.2) - 2024-04-30

### Other
- updated the following local packages: rattler_networking

## [0.23.1](https://github.com/mamba-org/rattler/compare/rattler-v0.23.0...rattler-v0.23.1) - 2024-04-25

### Other
- updated the following local packages: rattler_networking

## [0.23.0](https://github.com/mamba-org/rattler/compare/rattler-v0.22.0...rattler-v0.23.0) - 2024-04-25

### Added
- Expose paths_data as PathEntry in py-rattler ([#620](https://github.com/mamba-org/rattler/pull/620))
- add support for extracting prefix placeholder data to PathsEntry ([#614](https://github.com/mamba-org/rattler/pull/614))

### Fixed
- compare `UrlOrPath` ([#618](https://github.com/mamba-org/rattler/pull/618))

## [0.22.0](https://github.com/mamba-org/rattler/compare/rattler-v0.21.0...rattler-v0.22.0) - 2024-04-19

### Added
- make root dir configurable in channel config ([#602](https://github.com/mamba-org/rattler/pull/602))

### Fixed
- unicode activation issues on windows ([#604](https://github.com/mamba-org/rattler/pull/604))
- no shebang on windows to make spaces in prefix work ([#611](https://github.com/mamba-org/rattler/pull/611))
- use correct platform to decide the windows launcher ([#608](https://github.com/mamba-org/rattler/pull/608))

### Other
- update dependencies incl. reqwest ([#606](https://github.com/mamba-org/rattler/pull/606))

## [0.21.0](https://github.com/baszalmstra/rattler/compare/rattler-v0.20.1...rattler-v0.21.0) - 2024-04-05

### Fixed
- replace long shebangs with `/usr/bin/env` ([#594](https://github.com/baszalmstra/rattler/pull/594))
- run post-link scripts ([#574](https://github.com/baszalmstra/rattler/pull/574))

## [0.20.1](https://github.com/mamba-org/rattler/compare/rattler-v0.20.0...rattler-v0.20.1) - 2024-04-02

### Fixed
- copy windows dll without replacements ([#590](https://github.com/mamba-org/rattler/pull/590))

## [0.20.0](https://github.com/mamba-org/rattler/compare/rattler-v0.19.6...rattler-v0.20.0) - 2024-04-02

### Fixed
- do not do cstring replacement on windows ([#589](https://github.com/mamba-org/rattler/pull/589))

## [0.19.6](https://github.com/mamba-org/rattler/compare/rattler-v0.19.5...rattler-v0.19.6) - 2024-03-30

### Other
- remove unused dependencies ([#585](https://github.com/mamba-org/rattler/pull/585))

## [0.19.5](https://github.com/mamba-org/rattler/compare/rattler-v0.19.4...rattler-v0.19.5) - 2024-03-21

### Fixed
- typo ([#576](https://github.com/mamba-org/rattler/pull/576))

## [0.19.4](https://github.com/mamba-org/rattler/compare/rattler-v0.19.3...rattler-v0.19.4) - 2024-03-19

### Fixed
- multi-prefix replacement in binary files ([#570](https://github.com/mamba-org/rattler/pull/570))

## [0.19.3](https://github.com/mamba-org/rattler/compare/rattler-v0.19.2...rattler-v0.19.3) - 2024-03-14

### Added
- add mirror handling and OCI mirror type ([#553](https://github.com/mamba-org/rattler/pull/553))

### Other
- add pixi badge ([#563](https://github.com/mamba-org/rattler/pull/563))

## [0.19.2](https://github.com/mamba-org/rattler/compare/rattler-v0.19.1...rattler-v0.19.2) - 2024-03-08

### Other
- update Cargo.toml dependencies

## [0.19.1](https://github.com/mamba-org/rattler/compare/rattler-v0.19.0...rattler-v0.19.1) - 2024-03-06

### Added
- generalised CLI authentication ([#537](https://github.com/mamba-org/rattler/pull/537))

### Fixed
- removal of multiple packages that clobber each other ([#556](https://github.com/mamba-org/rattler/pull/556))
- dont use workspace dependencies for local crates ([#546](https://github.com/mamba-org/rattler/pull/546))

### Other
- every crate should have its own version ([#557](https://github.com/mamba-org/rattler/pull/557))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler-v0.18.0...rattler-v0.19.0) - 2024-02-26

