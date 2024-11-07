# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.29.0](https://github.com/conda/rattler/compare/rattler_conda_types-v0.28.3...rattler_conda_types-v0.29.0) - 2024-11-04

### Added

- use python_site_packages_path field when available for installing noarch: python packages, CEP-17 ([#909](https://github.com/conda/rattler/pull/909))
- Add `PackageRecord::validate` function ([#911](https://github.com/conda/rattler/pull/911))

### Fixed

- matchspec build / version from brackets and string serialization ([#917](https://github.com/conda/rattler/pull/917))

### Other

- root constraint shouldnt crash ([#916](https://github.com/conda/rattler/pull/916))

## [0.28.3](https://github.com/conda/rattler/compare/rattler_conda_types-v0.28.2...rattler_conda_types-v0.28.3) - 2024-10-21

### Other

- updated the following local packages: file_url

## [0.28.2](https://github.com/conda/rattler/compare/rattler_conda_types-v0.28.1...rattler_conda_types-v0.28.2) - 2024-10-07

### Other

- add snapshot tests to verify solver sorting order ([#895](https://github.com/conda/rattler/pull/895))

## [0.28.1](https://github.com/conda/rattler/compare/rattler_conda_types-v0.28.0...rattler_conda_types-v0.28.1) - 2024-10-03

### Fixed

- topological sort when cycles appear in leaf nodes ([#879](https://github.com/conda/rattler/pull/879))

## [0.28.0](https://github.com/conda/rattler/compare/rattler_conda_types-v0.27.6...rattler_conda_types-v0.28.0) - 2024-09-23

### Added

- add path to namedchannelorurl ([#873](https://github.com/conda/rattler/pull/873))
- add serialization for `GenericVirtualPackage` ([#865](https://github.com/conda/rattler/pull/865))

### Fixed

- improve when we print brackets ([#861](https://github.com/conda/rattler/pull/861))

## [0.27.6](https://github.com/conda/rattler/compare/rattler_conda_types-v0.27.5...rattler_conda_types-v0.27.6) - 2024-09-09

### Fixed

- publish `MatchSpecOrSubSection` for env yaml ([#855](https://github.com/conda/rattler/pull/855))

## [0.27.5](https://github.com/conda/rattler/compare/rattler_conda_types-v0.27.4...rattler_conda_types-v0.27.5) - 2024-09-05

### Fixed
- typos ([#849](https://github.com/conda/rattler/pull/849))

## [0.27.4](https://github.com/conda/rattler/compare/rattler_conda_types-v0.27.3...rattler_conda_types-v0.27.4) - 2024-09-03

### Other
- make PackageCache multi-process safe ([#837](https://github.com/conda/rattler/pull/837))

## [0.27.3](https://github.com/conda/rattler/compare/rattler_conda_types-v0.27.2...rattler_conda_types-v0.27.3) - 2024-09-02

### Added
- add edge case tests for `StringMatcher` ([#839](https://github.com/conda/rattler/pull/839))

## [0.27.2](https://github.com/conda/rattler/compare/rattler_conda_types-v0.27.1...rattler_conda_types-v0.27.2) - 2024-08-15

### Added
- add extra field ([#811](https://github.com/conda/rattler/pull/811))
- parse `channel` key and consolidate `NamelessMatchSpec` ([#810](https://github.com/conda/rattler/pull/810))

### Fixed
- move more links to the conda org from conda-incubator ([#816](https://github.com/conda/rattler/pull/816))
- use conda-incubator

### Other
- change links from conda-incubator to conda ([#813](https://github.com/conda/rattler/pull/813))
- update banner ([#808](https://github.com/conda/rattler/pull/808))

## [0.27.1](https://github.com/baszalmstra/rattler/compare/rattler_conda_types-v0.27.0...rattler_conda_types-v0.27.1) - 2024-08-06

### Fixed
- parse `~=` as version not as path ([#804](https://github.com/baszalmstra/rattler/pull/804))

## [0.27.0](https://github.com/baszalmstra/rattler/compare/rattler_conda_types-v0.26.3...rattler_conda_types-v0.27.0) - 2024-08-02

### Fixed
- redact secrets in the `canonical_name` functions ([#801](https://github.com/baszalmstra/rattler/pull/801))
- make `base_url` of `Channel` always contain a trailing slash ([#800](https://github.com/baszalmstra/rattler/pull/800))
- parse channel in matchspec string ([#792](https://github.com/baszalmstra/rattler/pull/792))
- constraints on virtual packages were ignored ([#795](https://github.com/baszalmstra/rattler/pull/795))
- url parsing for namelessmatchspec and cleanup functions ([#790](https://github.com/baszalmstra/rattler/pull/790))

### Other
- mark some crates 1.0 ([#789](https://github.com/baszalmstra/rattler/pull/789))

## [0.26.3](https://github.com/conda/rattler/compare/rattler_conda_types-v0.26.2...rattler_conda_types-v0.26.3) - 2024-07-23

### Fixed
- channel `base_url` requires trailing slash ([#787](https://github.com/conda/rattler/pull/787))

## [0.26.2](https://github.com/conda/rattler/compare/rattler_conda_types-v0.26.1...rattler_conda_types-v0.26.2) - 2024-07-23

### Added
- `environment.yaml` type ([#786](https://github.com/conda/rattler/pull/786))
- Add to_path() method to ExplicitEnvironmentSpec ([#781](https://github.com/conda/rattler/pull/781))
- expose `HasPrefixEntry` for public use ([#784](https://github.com/conda/rattler/pull/784))

## [0.26.1](https://github.com/conda/rattler/compare/rattler_conda_types-v0.26.0...rattler_conda_types-v0.26.1) - 2024-07-15

### Other
- PrefixRecord deserialization using simd ([#777](https://github.com/conda/rattler/pull/777))

## [0.26.0](https://github.com/conda/rattler/compare/rattler_conda_types-v0.25.2...rattler_conda_types-v0.26.0) - 2024-07-08

### Added
- add support for zos-z ([#753](https://github.com/conda/rattler/pull/753))
- return pybytes for sha256 and md5 everywhere and use md5 hash for legacy bz2 md5 ([#752](https://github.com/conda/rattler/pull/752))
- add direct url repodata building ([#725](https://github.com/conda/rattler/pull/725))
- add shards_base_url and write shards atomically ([#747](https://github.com/conda/rattler/pull/747))

### Fixed
- allow version following package in strict mode ([#770](https://github.com/conda/rattler/pull/770))
- Fix doctests and start testing them again ([#767](https://github.com/conda/rattler/pull/767))
- skip over implicit `0` components when copying ([#760](https://github.com/conda/rattler/pull/760))
- allow empty json repodata ([#745](https://github.com/conda/rattler/pull/745))
- lenient and strict parsing of equality signs ([#738](https://github.com/conda/rattler/pull/738))
- This fixes parsing of `ray[default,data] >=2.9.0,<3.0.0` ([#732](https://github.com/conda/rattler/pull/732))

## [0.25.2](https://github.com/baszalmstra/rattler/compare/rattler_conda_types-v0.25.1...rattler_conda_types-v0.25.2) - 2024-06-04

### Added
- parse url and path as matchspec ([#704](https://github.com/baszalmstra/rattler/pull/704))

### Fixed
- issue 722 ([#723](https://github.com/baszalmstra/rattler/pull/723))

### Other
- remove lfs ([#512](https://github.com/baszalmstra/rattler/pull/512))
- move the cache tooling into its own crate for reuse downstream ([#721](https://github.com/baszalmstra/rattler/pull/721))

## [0.25.1](https://github.com/conda/rattler/compare/rattler_conda_types-v0.25.0...rattler_conda_types-v0.25.1) - 2024-06-03

### Added
- add a `with_alpha` function that adds `0a0` to the version ([#696](https://github.com/conda/rattler/pull/696))

## [0.25.0](https://github.com/conda/rattler/compare/rattler_conda_types-v0.24.0...rattler_conda_types-v0.25.0) - 2024-05-28

### Added
- when bumping, extend versions with `0` to match the bump request ([#695](https://github.com/conda/rattler/pull/695))
- extend tests and handle characters better when bumping versions ([#694](https://github.com/conda/rattler/pull/694))
- add a function to extend version with `0s` ([#689](https://github.com/conda/rattler/pull/689))
- add run exports to package data ([#671](https://github.com/conda/rattler/pull/671))

### Fixed
- lenient parsing of 2023.*.* ([#688](https://github.com/conda/rattler/pull/688))
- VersionSpec starts with, with trailing zeros ([#686](https://github.com/conda/rattler/pull/686))

### Other
- move bump implementation to bump.rs and simplify tests ([#692](https://github.com/conda/rattler/pull/692))

## [0.24.0](https://github.com/conda/rattler/compare/rattler_conda_types-v0.23.1...rattler_conda_types-v0.24.0) - 2024-05-27

### Added
- removed Ord and more ([#673](https://github.com/conda/rattler/pull/673))
- always store purls as a key in lock file ([#669](https://github.com/conda/rattler/pull/669))
- add solve strategies ([#660](https://github.com/conda/rattler/pull/660))

### Fixed
- make topological sorting support fully cyclic dependencies ([#678](https://github.com/conda/rattler/pull/678))

## [0.23.1](https://github.com/conda/rattler/compare/rattler_conda_types-v0.23.0...rattler_conda_types-v0.23.1) - 2024-05-14

### Added
- exclude repodata records based on timestamp ([#654](https://github.com/conda/rattler/pull/654))

## [0.23.0](https://github.com/conda/rattler/compare/rattler_conda_types-v0.22.1...rattler_conda_types-v0.23.0) - 2024-05-13

### Added
- high level repodata access ([#560](https://github.com/conda/rattler/pull/560))

### Other
- update README.md

## [0.22.1](https://github.com/conda/rattler/compare/rattler_conda_types-v0.22.0...rattler_conda_types-v0.22.1) - 2024-05-06

### Added
- expose `*Record.noarch` in Python bindings ([#635](https://github.com/conda/rattler/pull/635))

## [0.22.0](https://github.com/conda/rattler/compare/rattler_conda_types-v0.21.0...rattler_conda_types-v0.22.0) - 2024-04-25

### Added
- add support for extracting prefix placeholder data to PathsEntry ([#614](https://github.com/conda/rattler/pull/614))

## [0.21.0](https://github.com/conda/rattler/compare/rattler_conda_types-v0.20.5...rattler_conda_types-v0.21.0) - 2024-04-19

### Added
- make root dir configurable in channel config ([#602](https://github.com/conda/rattler/pull/602))

### Fixed
- better value for `link` field ([#610](https://github.com/conda/rattler/pull/610))

### Other
- update dependencies incl. reqwest ([#606](https://github.com/conda/rattler/pull/606))

## [0.20.5](https://github.com/baszalmstra/rattler/compare/rattler_conda_types-v0.20.4...rattler_conda_types-v0.20.5) - 2024-04-05

### Fixed
- run post-link scripts ([#574](https://github.com/baszalmstra/rattler/pull/574))

## [0.20.4](https://github.com/conda/rattler/compare/rattler_conda_types-v0.20.3...rattler_conda_types-v0.20.4) - 2024-03-30

### Fixed
- matchspec empty namespace and channel canonical name ([#582](https://github.com/conda/rattler/pull/582))

## [0.20.3](https://github.com/conda/rattler/compare/rattler_conda_types-v0.20.2...rattler_conda_types-v0.20.3) - 2024-03-21

### Fixed
- allow not starts with in strict mode ([#577](https://github.com/conda/rattler/pull/577))

## [0.20.2](https://github.com/conda/rattler/compare/rattler_conda_types-v0.20.1...rattler_conda_types-v0.20.2) - 2024-03-14

### Other
- add pixi badge ([#563](https://github.com/conda/rattler/pull/563))

## [0.20.1](https://github.com/conda/rattler/compare/rattler_conda_types-v0.20.0...rattler_conda_types-v0.20.1) - 2024-03-08

### Fixed
- chrono deprecation warnings ([#558](https://github.com/conda/rattler/pull/558))

## [0.20.0](https://github.com/conda/rattler/compare/rattler_conda_types-v0.19.0...rattler_conda_types-v0.20.0) - 2024-03-06

### Added
- [**breaking**] optional strict parsing of matchspec and versionspec ([#552](https://github.com/conda/rattler/pull/552))

### Fixed
- patch unsupported glob operators ([#551](https://github.com/conda/rattler/pull/551))
- dont use workspace dependencies for local crates ([#546](https://github.com/conda/rattler/pull/546))

### Other
- every crate should have its own version ([#557](https://github.com/conda/rattler/pull/557))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler_conda_types-v0.18.0...rattler_conda_types-v0.19.0) - 2024-02-26

### Fixed
- Fix arch for osx-arm64 and win-arm64 ([#528](https://github.com/baszalmstra/rattler/pull/528))
- Channel name display ([#531](https://github.com/baszalmstra/rattler/pull/531))
