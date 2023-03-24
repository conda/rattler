# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2023-03-24

### Added

- Construction methods for NoArchType (#130)
- Function to create package record from index.json + size and hashes (#126)
- Serialization for repodata (#124)
- Functions to apply patches to repodata (#127)
- More tests and evict removed packages from repodata (#128)
- First version of package writing functions (#112)

### Changed

- Removed dependency on `clang-sys` during build (#131)

## [0.1.0] - 2023-03-16

First release

[unreleased]: https://github.com/mamba-org/rattler/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/mamba-org/rattler/releases/tag/v0.1.0
