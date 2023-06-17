# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Refactored shell detection code using `$SHELL` or parent process name ([#219](https://github.com/mamba-org/rattler/pull/219))

## [0.3.0] - 2023-06-15

### Highlights

This release contains lots of fixes, small (breaking) changes, and new features.
The biggest highlights are:

#### JLAP support

JLAP is a file format to incrementally update a cached `repodata.json` without downloading the entire file.
This can save a huge amount of bandwidth for large repodatas that change often (like those from conda-forge).
If you have a previously cached `repodata.json` on your system only small JSON patches are downloaded to bring your cache up to date.
The format was initially proposed through a [CEP](https://github.com/conda-incubator/ceps/pull/20) and has been available in conda as an experimental feature since `23.3.0`.

When using rattler you get JLAP support out of the box. 
No changes are needed.

#### Support for local `file://`

`file://` based urls are now supported for all functions that use a Url to download certain data.

#### `rattler_networking`

A new crate has been added to facilitate authentication when downloading repodata or packages called `rattler_networking`.

### Details

#### Added

- Support for detecting more platforms ([#135](https://github.com/mamba-org/rattler/pull/135))
- `RepoData` is now clonable ([#138](https://github.com/mamba-org/rattler/pull/138))
- `RunExportsJson` is now clonable ([#169](https://github.com/mamba-org/rattler/pull/169))
- `file://` urls are now supported for package extraction functions ([#157](https://github.com/mamba-org/rattler/pull/157))
- `file://` urls are now supported for repodata fetching ([#158](https://github.com/mamba-org/rattler/pull/158))
- Getting started with rattler using micromamba ([#163](https://github.com/mamba-org/rattler/pull/163))
- Add `Platform::arch` function to return the architecture of a given platform ([#166](https://github.com/mamba-org/rattler/pull/166))
- Extracted Python style JSON formatting into [a separate crate](https://github.com/prefix-dev/serde-json-python-formatter) ([#163](https://github.com/mamba-org/rattler/pull/180))
- Added feature to use `rustls` with `rattler_package_streaming` and `rattler_repodata_gateway` ([#179](https://github.com/mamba-org/rattler/pull/179) & [#181](https://github.com/mamba-org/rattler/pull/181))
- Expose `version_spec` module ([#183](https://github.com/mamba-org/rattler/pull/183))
- `NamelessMatchSpec` a variant of `MatchSpec` that does not include a package name [#185](https://github.com/mamba-org/rattler/pull/185))
- `ShellEnum` - a dynamic shell type for dynamic discovery [#187](https://github.com/mamba-org/rattler/pull/187))
- Exposed the `python_entry_point_template` function ([#190](https://github.com/mamba-org/rattler/pull/190))
- Enable deserializing virtual packages ([#198](https://github.com/mamba-org/rattler/pull/198))
- Refactored CI to add macOS arm64 ([#201](https://github.com/mamba-org/rattler/pull/201))
- Support for JLAP when downloading repodata ([#197](https://github.com/mamba-org/rattler/pull/197) & [#214](https://github.com/mamba-org/rattler/pull/214))
- `Clone`, `Debug`, `PartialEq`, `Eq` implementations for conda lock types ([#213](https://github.com/mamba-org/rattler/pull/213)) 
- `rattler_networking` to enable accessing `repodata.json` and packages that require authentication ([#191](https://github.com/mamba-org/rattler/pull/191))

#### Changed

- `FileMode` is now included with `prefix_placeholder` is set ([#136](https://github.com/mamba-org/rattler/pull/135))  
- `rattler_digest` now re-exports commonly used hash types and typed hashes are now used in more placed (instead of strings) [[#137](https://github.com/mamba-org/rattler/pull/137) & [#153](https://github.com/mamba-org/rattler/pull/153)]
- Use `Platform` in to detect running operating system ([#144](https://github.com/mamba-org/rattler/pull/144))
- `paths.json` is now serialized in a deterministic fashion ([#147](https://github.com/mamba-org/rattler/pull/147))
- Determine the `subdir` for the `platform` and `arch` fields when creating a `PackageRecord` from an `index.json` ([#145](https://github.com/mamba-org/rattler/pull/145) & [#152](https://github.com/mamba-org/rattler/pull/152))
- `Activator::activation` now returns the new `PATH` in addition to the script ([#151](https://github.com/mamba-org/rattler/pull/151))
- Use properly typed `chrono::DateTime<chrono::Utc>` for timestamps instead of `u64` ([#157](https://github.com/mamba-org/rattler/pull/157))
- Made `ParseError` public and reuse `ArchiveType` ([#167](https://github.com/mamba-org/rattler/pull/167))
- Allow setting timestamps when creating package archive ([#171](https://github.com/mamba-org/rattler/pull/171))
- `about.json` and `index.json` are now serialized in a deterministic fashion ([#180](https://github.com/mamba-org/rattler/pull/180))
- SHA256 and MD5 hashes are computed on the fly when extracting packages ([#176](https://github.com/mamba-org/rattler/pull/176)
- Change blake2 hash to use blake2b instead of blake2s ([#192](https://github.com/mamba-org/rattler/pull/192)
- LibSolv error messages are now passed through ([#202](https://github.com/mamba-org/rattler/pull/202) & [#210](https://github.com/mamba-org/rattler/pull/210))
- `VersionTree` parsing now uses `nom` instead of a complex regex ([#206](https://github.com/mamba-org/rattler/pull/206)
- `libc` version detection now uses `lld --version` to properly detect the libc version on the host ([#209](https://github.com/mamba-org/rattler/pull/209)
- Improved version parse error messages ([#211](https://github.com/mamba-org/rattler/pull/211)
- Parsing of some complex MatchSpecs ([#217](https://github.com/mamba-org/rattler/pull/217)

#### Fixed

- MatchSpec bracket list parsing can now handle quoted values ([#157](https://github.com/mamba-org/rattler/pull/156))
- Typos and documentation ([#164](https://github.com/mamba-org/rattler/pull/164) & [#188](https://github.com/mamba-org/rattler/pull/188))
- Allow downloading of repodata.json to fail in some cases (only noarch is a required subdir) ([#174](https://github.com/mamba-org/rattler/pull/174))
- Missing feature when using the sparse index ([#182](https://github.com/mamba-org/rattler/pull/182))
- Several small issues or missing functionality ([#184](https://github.com/mamba-org/rattler/pull/184))
- Loosened strictness of comparing packages in `Transaction`s ([#186](https://github.com/mamba-org/rattler/pull/186)
- Missing `noarch: generic` parsing in `links.json` ([#189](https://github.com/mamba-org/rattler/pull/189)
- Ignore trailing .0 in version comparison ([#196](https://github.com/mamba-org/rattler/pull/196)

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
