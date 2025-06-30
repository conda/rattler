# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.14](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.13...rattler_menuinst-v0.2.14) - 2025-06-26

### Other

- updated the following local packages: rattler_conda_types, rattler_shell

## [0.2.13](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.12...rattler_menuinst-v0.2.13) - 2025-06-25

### Other

- updated the following local packages: rattler_conda_types, rattler_shell

## [0.2.12](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.11...rattler_menuinst-v0.2.12) - 2025-06-24

### Other

- *(ci)* Update Rust crate windows to 0.61.0 ([#1462](https://github.com/conda/rattler/pull/1462))

## [0.2.11](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.10...rattler_menuinst-v0.2.11) - 2025-06-23

### Fixed

- use $PATH prepend behavior in `menuinst` activation ([#1376](https://github.com/conda/rattler/pull/1376))

### Other

- update npm name ([#1368](https://github.com/conda/rattler/pull/1368))
- update readme ([#1364](https://github.com/conda/rattler/pull/1364))

## [0.2.10](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.9...rattler_menuinst-v0.2.10) - 2025-05-23

### Other

- updated the following local packages: rattler_conda_types, rattler_shell

## [0.2.9](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.8...rattler_menuinst-v0.2.9) - 2025-05-16

### Other

- update dependencies ([#1126](https://github.com/conda/rattler/pull/1126))
- Bump zip to 3.0.0 ([#1310](https://github.com/conda/rattler/pull/1310))

## [0.2.8](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.7...rattler_menuinst-v0.2.8) - 2025-05-03

### Fixed

- menuinst windows shortcut path ([#1273](https://github.com/conda/rattler/pull/1273))

### Other

- lock workspace member dependencies ([#1279](https://github.com/conda/rattler/pull/1279))

## [0.2.7](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.6...rattler_menuinst-v0.2.7) - 2025-04-10

### Other

- update Cargo.toml dependencies

## [0.2.6](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.5...rattler_menuinst-v0.2.6) - 2025-04-04

### Fixed

- install windows start menu shortcut ([#1198](https://github.com/conda/rattler/pull/1198))

### Other

- change default value of  menuinst windows `quicklaunch` to `false` ([#1196](https://github.com/conda/rattler/pull/1196))

## [0.2.5](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.4...rattler_menuinst-v0.2.5) - 2025-03-18

### Added

- allow to pass a semaphore for concurrency control ([#1169](https://github.com/conda/rattler/pull/1169))

## [0.2.4](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.3...rattler_menuinst-v0.2.4) - 2025-03-14

### Added

- remove menu item and trackers in one function ([#1160](https://github.com/conda/rattler/pull/1160))

## [0.2.3](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.2...rattler_menuinst-v0.2.3) - 2025-03-10

### Other

- update Cargo.toml dependencies

## [0.2.2](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.1...rattler_menuinst-v0.2.2) - 2025-03-04

### Fixed

- shortcut filename ([#1136](https://github.com/conda/rattler/pull/1136))
- create test directories ([#1135](https://github.com/conda/rattler/pull/1135))

## [0.2.1](https://github.com/conda/rattler/compare/rattler_menuinst-v0.2.0...rattler_menuinst-v0.2.1) - 2025-02-28

### Added

- add fake folders method on win for easier testing (#1125)

## [0.2.0](https://github.com/conda/rattler/compare/rattler_menuinst-v0.1.0...rattler_menuinst-v0.2.0) - 2025-02-27

### Added

- Use `opendal` in `rattler-index` and add executable (#1076)

### Fixed

- make `menuinst` schema pub, hide utils, fix indexing for rattler-build (#1111)
- clippy lint (#1105)

## [0.1.0](https://github.com/conda/rattler/releases/tag/rattler_menuinst-v0.1.0) - 2025-02-25

### Added

- add `rattler_menuinst` crate (#840)
- better readme (#118)
- replace zulip with discord (#116)
- move all conda types to separate crate

### Fixed

- release-plz (#1100)
- typos (#849)
- move more links to the conda org from conda-incubator (#816)
- use conda-incubator
- add python docs badge
- typo libsolve -> libsolv (#164)
- change urls from baszalmstra to mamba-org
- build badge

### Other

- fix anchor link (#1035)
- change links from conda-incubator to conda (#813)
- update banner (#808)
- update README.md
- add pixi badge (#563)
- update installation gif
- update banner image
- address issue [#282](https://github.com/conda/rattler/pull/282) ([#283](https://github.com/conda/rattler/pull/283))
- Add an image to Readme ([#203](https://github.com/conda/rattler/pull/203))
- Improve getting started with a micromamba environment. ([#163](https://github.com/conda/rattler/pull/163))
- Misc/update readme ([#66](https://github.com/conda/rattler/pull/66))
- update readme
- layout the vision a little bit better
- *(docs)* add build badge
- matchspec parsing
