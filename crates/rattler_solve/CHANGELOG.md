# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
