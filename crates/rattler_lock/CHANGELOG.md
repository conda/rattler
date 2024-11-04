# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.22.29](https://github.com/conda/rattler/compare/rattler_lock-v0.22.28...rattler_lock-v0.22.29) - 2024-11-04

### Added

- use python_site_packages_path field when available for installing noarch: python packages, CEP-17 ([#909](https://github.com/conda/rattler/pull/909))
- bump pep crates in rattler ([#918](https://github.com/conda/rattler/pull/918))
- add debug trait to LockFile struct in rattler_lock ([#922](https://github.com/conda/rattler/pull/922))

## [0.22.28](https://github.com/conda/rattler/compare/rattler_lock-v0.22.27...rattler_lock-v0.22.28) - 2024-10-21

### Other

- updated the following local packages: file_url

## [0.22.27](https://github.com/conda/rattler/compare/rattler_lock-v0.22.26...rattler_lock-v0.22.27) - 2024-10-07

### Other

- updated the following local packages: rattler_conda_types

## [0.22.26](https://github.com/conda/rattler/compare/rattler_lock-v0.22.25...rattler_lock-v0.22.26) - 2024-10-03

### Other

- updated the following local packages: rattler_conda_types

## [0.22.25](https://github.com/conda/rattler/compare/rattler_lock-v0.22.24...rattler_lock-v0.22.25) - 2024-09-23

### Other

- updated the following local packages: rattler_conda_types

## [0.22.24](https://github.com/conda/rattler/compare/rattler_lock-v0.22.23...rattler_lock-v0.22.24) - 2024-09-09

### Other

- updated the following local packages: rattler_conda_types

## [0.22.23](https://github.com/conda/rattler/compare/rattler_lock-v0.22.22...rattler_lock-v0.22.23) - 2024-09-05

### Fixed
- typos ([#849](https://github.com/conda/rattler/pull/849))

## [0.22.22](https://github.com/conda/rattler/compare/rattler_lock-v0.22.21...rattler_lock-v0.22.22) - 2024-09-03

### Other
- make PackageCache multi-process safe ([#837](https://github.com/conda/rattler/pull/837))

## [0.22.21](https://github.com/conda/rattler/compare/rattler_lock-v0.22.20...rattler_lock-v0.22.21) - 2024-09-02

### Added
- Add support for `CONDA_OVERRIDE_CUDA` ([#818](https://github.com/conda/rattler/pull/818))

## [0.22.20](https://github.com/conda/rattler/compare/rattler_lock-v0.22.19...rattler_lock-v0.22.20) - 2024-08-16

### Added
- default construct marker-tree ([#825](https://github.com/conda/rattler/pull/825))

## [0.22.19](https://github.com/conda/rattler/compare/rattler_lock-v0.22.18...rattler_lock-v0.22.19) - 2024-08-15

### Fixed
- move more links to the conda org from conda-incubator ([#816](https://github.com/conda/rattler/pull/816))

### Other
- change links from conda-incubator to conda ([#813](https://github.com/conda/rattler/pull/813))
- update banner ([#808](https://github.com/conda/rattler/pull/808))

## [0.22.18](https://github.com/baszalmstra/rattler/compare/rattler_lock-v0.22.17...rattler_lock-v0.22.18) - 2024-08-06

### Other
- updated the following local packages: rattler_conda_types

## [0.22.17](https://github.com/baszalmstra/rattler/compare/rattler_lock-v0.22.16...rattler_lock-v0.22.17) - 2024-08-02

### Other
- mark some crates 1.0 ([#789](https://github.com/baszalmstra/rattler/pull/789))

## [0.22.16](https://github.com/conda/rattler/compare/rattler_lock-v0.22.15...rattler_lock-v0.22.16) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.22.15](https://github.com/conda/rattler/compare/rattler_lock-v0.22.14...rattler_lock-v0.22.15) - 2024-07-23

### Other
- updated the following local packages: rattler_conda_types

## [0.22.14](https://github.com/conda/rattler/compare/rattler_lock-v0.22.13...rattler_lock-v0.22.14) - 2024-07-15

### Other
- bump dependencies and remove unused ones ([#771](https://github.com/conda/rattler/pull/771))

## [0.22.13](https://github.com/conda/rattler/compare/rattler_lock-v0.22.12...rattler_lock-v0.22.13) - 2024-07-08

### Added
- Only save md5 in lock file if no sha256 is present ([#764](https://github.com/conda/rattler/pull/764))
- return pybytes for sha256 and md5 everywhere and use md5 hash for legacy bz2 md5 ([#752](https://github.com/conda/rattler/pull/752))
- add direct url repodata building ([#725](https://github.com/conda/rattler/pull/725))

### Fixed
- lock file stability issues with PyPI types ([#761](https://github.com/conda/rattler/pull/761))
- errors should not contain trailing punctuation ([#763](https://github.com/conda/rattler/pull/763))

### Other
- revert only save md5 in lock file if no sha256 is present ([#766](https://github.com/conda/rattler/pull/766))

## [0.22.12](https://github.com/conda/rattler/compare/rattler_lock-v0.22.11...rattler_lock-v0.22.12) - 2024-06-06

### Added
- serialize packages from lock file individually ([#728](https://github.com/conda/rattler/pull/728))

## [0.22.11](https://github.com/baszalmstra/rattler/compare/rattler_lock-v0.22.10...rattler_lock-v0.22.11) - 2024-06-04

### Other
- updated the following local packages: file_url, rattler_conda_types

## [0.22.10](https://github.com/conda/rattler/compare/rattler_lock-v0.22.9...rattler_lock-v0.22.10) - 2024-06-03

### Other
- updated the following local packages: rattler_conda_types

## [0.22.9](https://github.com/conda/rattler/compare/rattler_lock-v0.22.8...rattler_lock-v0.22.9) - 2024-05-28

### Added
- add run exports to package data ([#671](https://github.com/conda/rattler/pull/671))

### Other
- bump ([#683](https://github.com/conda/rattler/pull/683))

## [0.22.8](https://github.com/conda/rattler/compare/rattler_lock-v0.22.7...rattler_lock-v0.22.8) - 2024-05-27

### Added
- removed Ord and more ([#673](https://github.com/conda/rattler/pull/673))
- always store purls as a key in lock file ([#669](https://github.com/conda/rattler/pull/669))

## [0.22.7](https://github.com/conda/rattler/compare/rattler_lock-v0.22.6...rattler_lock-v0.22.7) - 2024-05-14

### Other
- bump pep crates ([#661](https://github.com/conda/rattler/pull/661))

## [0.22.6](https://github.com/conda/rattler/compare/rattler_lock-v0.22.5...rattler_lock-v0.22.6) - 2024-05-13

### Added
- high level repodata access ([#560](https://github.com/conda/rattler/pull/560))

### Other
- update README.md

## [0.22.5](https://github.com/conda/rattler/compare/rattler_lock-v0.22.4...rattler_lock-v0.22.5) - 2024-05-06

### Other
- updated the following local packages: rattler_conda_types

## [0.22.4](https://github.com/conda/rattler/compare/rattler_lock-v0.22.3...rattler_lock-v0.22.4) - 2024-04-30

### Added
- adds pypi indexes to the lock-file ([#626](https://github.com/conda/rattler/pull/626))

## [0.22.3](https://github.com/conda/rattler/compare/rattler_lock-v0.22.2...rattler_lock-v0.22.3) - 2024-04-25

### Fixed
- compare `UrlOrPath` ([#618](https://github.com/conda/rattler/pull/618))
- parse absolute paths on Windows correctly in lockfiles ([#616](https://github.com/conda/rattler/pull/616))

## [0.22.2](https://github.com/conda/rattler/compare/rattler_lock-v0.22.1...rattler_lock-v0.22.2) - 2024-04-19

### Other
- update dependencies incl. reqwest ([#606](https://github.com/conda/rattler/pull/606))

## [0.22.1](https://github.com/baszalmstra/rattler/compare/rattler_lock-v0.22.0...rattler_lock-v0.22.1) - 2024-04-05

### Other
- updated the following local packages: rattler_conda_types

## [0.22.0](https://github.com/conda/rattler/compare/rattler_lock-v0.21.0...rattler_lock-v0.22.0) - 2024-03-30

### Added
- editable pypi packages ([#581](https://github.com/conda/rattler/pull/581))

## [0.21.0](https://github.com/conda/rattler/compare/rattler_lock-v0.20.2...rattler_lock-v0.21.0) - 2024-03-21

### Added
- allow passing pypi paths ([#572](https://github.com/conda/rattler/pull/572))

## [0.20.2](https://github.com/conda/rattler/compare/rattler_lock-v0.20.1...rattler_lock-v0.20.2) - 2024-03-14

### Other
- add pixi badge ([#563](https://github.com/conda/rattler/pull/563))

## [0.20.1](https://github.com/conda/rattler/compare/rattler_lock-v0.20.0...rattler_lock-v0.20.1) - 2024-03-08

### Fixed
- chrono deprecation warnings ([#558](https://github.com/conda/rattler/pull/558))

## [0.20.0](https://github.com/conda/rattler/compare/rattler_lock-v0.19.0...rattler_lock-v0.20.0) - 2024-03-06

### Added
- sort extras by name and urls by filename ([#540](https://github.com/conda/rattler/pull/540))

### Fixed
- dont use workspace dependencies for local crates ([#546](https://github.com/conda/rattler/pull/546))

### Other
- every crate should have its own version ([#557](https://github.com/conda/rattler/pull/557))
- bump pep508_rs and pep440_rs ([#549](https://github.com/conda/rattler/pull/549))

## [0.19.0](https://github.com/baszalmstra/rattler/compare/rattler_lock-v0.18.0...rattler_lock-v0.19.0) - 2024-02-26
