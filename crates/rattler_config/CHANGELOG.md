# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- A compatibility test catalog (`tests/compat.rs` + `test-data/compat/`) pinning down the configuration contract: parsing permutations (kebab-case, snake_case aliases, deprecated keys, typos), lossless/idempotent round-trips, a set/unset editing matrix over every key family, and merge semantics per key family. Extend the fixtures and `EDIT_MATRIX` when adding keys.
- `ConfigBase::from_toml_str` parses a configuration and reports the keys that neither the common configuration nor the extension recognized, so tools can warn about typos — including keys of tool-specific extensions and deprecated keys inside known tables (e.g. `repodata-config.disable-jlap`).
- `ConfigBase::load_from_default_locations` and the new `locations` module: shared config-file discovery (`/etc/<tool>/config.toml`, `$XDG_CONFIG_HOME/<tool>/config.toml`, `$<TOOL>_HOME` or `~/.<tool>/config.toml`) for a list of cooperating tools, e.g. `&["pixi", "rattler-build"]`.
- `ConfigBase::set` is now implemented generically by round-tripping through TOML: every key — including extension keys — can be set/unset without any per-field code. Values are interpreted as JSON when possible and fall back to plain strings. Key segments containing dots can be quoted (`mirrors."https://conda.anaconda.org"`).
- `ProxyConfig::from_env` reads the proxy configuration from the standard `HTTP(S)_PROXY`/`NO_PROXY` environment variables.
- Top-level `allow-symbolic-links`, `allow-hard-links` and `allow-ref-links` keys (previously only modeled by the unwired `link_config` module, and matching pixi's flat schema).

### Changed

- **Breaking:** the common fields of `ConfigBase<T>` moved into the new `CommonConfig` struct (`config.common`). `ConfigBase` dereferences to `CommonConfig`, so field *access* (`config.default_channels`) keeps working; struct literals need updating.
- **Breaking:** the `Config` trait was slimmed down: `get_extension_name` and `set` were removed, `validate`, `is_default` and `keys` have default implementations. A minimal extension now only implements `merge_config`. The `Eq` bound was relaxed to `PartialEq`.
- **Breaking:** the extension type for "no extension" is the new `NoExtension` struct (default type parameter of `ConfigBase`); the `Config` impl for `()` was removed because a unit type cannot be deserialized from a TOML document.
- **Breaking:** `ProxyConfig::default()` no longer reads proxy environment variables (this leaked machine state into serialized configs, e.g. when saving after `config set`); use `ProxyConfig::from_env()` and merge it explicitly.
- **Breaking:** `ConfigBase::default().tls_no_verify` is now `None` instead of `Some(false)`, so "unset" is representable and later layers can distinguish it from an explicit `false`.
- `ConfigBase::validate` now actually validates: it recurses into the nested sections (concurrency, proxy, index, …) and the extension. Previously it always returned `Ok(())`, so `load_from_files` never validated anything.
- `ConfigBase::load_from_files` now records the source files in `loaded_from` and warns (via `tracing`) about unrecognized keys.
- `Config::keys` now returns real dotted TOML key paths (e.g. `repodata-config.disable-bzip2` instead of the previous `repodata.default`), fixing the "supported keys" listing in error messages.

### Fixed

- `RepodataConfig::merge_config` merged in the wrong direction: repodata flags from an earlier configuration file could not be overridden by later files (e.g. a project config could not re-enable zstd disabled in the global config). It now follows the documented "`other` takes priority" contract like every other section.
- `ProxyConfig::is_default` checked `https` twice and never `http`.
- Removed the misleading `load_config` free function (it neither merged nor validated).
- Removed the unwired `link_config` module in favor of the flat top-level keys.

## [0.5.2](https://github.com/conda/rattler/compare/rattler_config-v0.5.1...rattler_config-v0.5.2) - 2026-06-17

### Other

- updated the following local packages: rattler_conda_types

## [0.5.1](https://github.com/conda/rattler/compare/rattler_config-v0.5.0...rattler_config-v0.5.1) - 2026-06-09

### Other

- updated the following local packages: rattler_conda_types

## [0.5.0](https://github.com/conda/rattler/compare/rattler_config-v0.4.1...rattler_config-v0.5.0) - 2026-06-02

### Added

- align rattler_config with pixi config fields ([#2439](https://github.com/conda/rattler/pull/2439))

### Fixed

- make sdist PEP 625 conformant and trim test data ([#2470](https://github.com/conda/rattler/pull/2470))

## [0.4.1](https://github.com/conda/rattler/compare/rattler_config-v0.4.0...rattler_config-v0.4.1) - 2026-05-19

### Other

- updated the following local packages: rattler_conda_types

## [0.4.0](https://github.com/conda/rattler/compare/rattler_config-v0.3.13...rattler_config-v0.4.0) - 2026-05-19

### Added

- channel index options (TOML) for `rattler-index` ([#2390](https://github.com/conda/rattler/pull/2390))

## [0.3.13](https://github.com/conda/rattler/compare/rattler_config-v0.3.12...rattler_config-v0.3.13) - 2026-05-13

### Other

- bump Rust edition to 2024 ([#2429](https://github.com/conda/rattler/pull/2429))

## [0.3.12](https://github.com/conda/rattler/compare/rattler_config-v0.3.11...rattler_config-v0.3.12) - 2026-05-07

### Other

- updated the following local packages: rattler_conda_types

## [0.3.11](https://github.com/conda/rattler/compare/rattler_config-v0.3.10...rattler_config-v0.3.11) - 2026-05-01

### Other

- updated the following local packages: rattler_conda_types

## [0.3.10](https://github.com/conda/rattler/compare/rattler_config-v0.3.9...rattler_config-v0.3.10) - 2026-04-30

### Other

- Added a getting started explainer to README ([#2334](https://github.com/conda/rattler/pull/2334))

## [0.3.9](https://github.com/conda/rattler/compare/rattler_config-v0.3.8...rattler_config-v0.3.9) - 2026-04-13

### Other

- updated the following local packages: rattler_conda_types

## [0.3.8](https://github.com/conda/rattler/compare/rattler_config-v0.3.7...rattler_config-v0.3.8) - 2026-04-08

### Other

- updated the following local packages: rattler_conda_types

## [0.3.7](https://github.com/conda/rattler/compare/rattler_config-v0.3.6...rattler_config-v0.3.7) - 2026-04-07

### Other

- updated the following local packages: rattler_conda_types

## [0.3.6](https://github.com/conda/rattler/compare/rattler_config-v0.3.5...rattler_config-v0.3.6) - 2026-03-25

### Other

- updated the following local packages: rattler_conda_types

## [0.3.5](https://github.com/conda/rattler/compare/rattler_config-v0.3.4...rattler_config-v0.3.5) - 2026-03-20

### Other

- updated the following local packages: rattler_conda_types

## [0.3.4](https://github.com/conda/rattler/compare/rattler_config-v0.3.3...rattler_config-v0.3.4) - 2026-03-18

### Other

- update Cargo.toml dependencies

## [0.3.3](https://github.com/conda/rattler/compare/rattler_config-v0.3.2...rattler_config-v0.3.3) - 2026-03-16

### Other

- updated the following local packages: rattler_conda_types

## [0.3.2](https://github.com/conda/rattler/compare/rattler_config-v0.3.1...rattler_config-v0.3.2) - 2026-02-25

### Other

- updated the following local packages: rattler_conda_types

## [0.3.1](https://github.com/conda/rattler/compare/rattler_config-v0.3.0...rattler_config-v0.3.1) - 2026-02-20

### Other

- updated the following local packages: rattler_conda_types

## [0.3.0](https://github.com/conda/rattler/compare/rattler_config-v0.2.26...rattler_config-v0.3.0) - 2026-02-19

### Other

- [**breaking**] remove support for JLAP ([#2038](https://github.com/conda/rattler/pull/2038))

## [0.2.26](https://github.com/conda/rattler/compare/rattler_config-v0.2.25...rattler_config-v0.2.26) - 2026-02-04

### Other

- updated the following local packages: rattler_conda_types

## [0.2.25](https://github.com/conda/rattler/compare/rattler_config-v0.2.24...rattler_config-v0.2.25) - 2026-01-22

### Other

- updated the following local packages: rattler_conda_types

## [0.2.24](https://github.com/conda/rattler/compare/rattler_config-v0.2.23...rattler_config-v0.2.24) - 2026-01-22

### Added

- add support for `packages.whl` and wheel archives types ([#1988](https://github.com/conda/rattler/pull/1988))

## [0.2.23](https://github.com/conda/rattler/compare/rattler_config-v0.2.22...rattler_config-v0.2.23) - 2025-12-18

### Other

- update README.md with new banner image ([#1926](https://github.com/conda/rattler/pull/1926))

## [0.2.22](https://github.com/conda/rattler/compare/rattler_config-v0.2.21...rattler_config-v0.2.22) - 2025-12-08

### Other

- update Cargo.toml dependencies

## [0.2.21](https://github.com/conda/rattler/compare/rattler_config-v0.2.20...rattler_config-v0.2.21) - 2025-11-27

### Other

- updated the following local packages: rattler_conda_types

## [0.2.20](https://github.com/conda/rattler/compare/rattler_config-v0.2.19...rattler_config-v0.2.20) - 2025-11-25

### Other

- updated the following local packages: rattler_conda_types

## [0.2.19](https://github.com/conda/rattler/compare/rattler_config-v0.2.18...rattler_config-v0.2.19) - 2025-11-22

### Other

- updated the following local packages: rattler_conda_types

## [0.2.18](https://github.com/conda/rattler/compare/rattler_config-v0.2.17...rattler_config-v0.2.18) - 2025-11-20

### Other

- updated the following local packages: rattler_conda_types

## [0.2.17](https://github.com/conda/rattler/compare/rattler_config-v0.2.16...rattler_config-v0.2.17) - 2025-11-19

### Other

- updated the following local packages: rattler_conda_types

## [0.2.16](https://github.com/conda/rattler/compare/rattler_config-v0.2.15...rattler_config-v0.2.16) - 2025-11-13

### Added

- expose crate features on docs.rs ([#1835](https://github.com/conda/rattler/pull/1835))

## [0.2.15](https://github.com/conda/rattler/compare/rattler_config-v0.2.14...rattler_config-v0.2.15) - 2025-10-28

### Other

- updated the following local packages: rattler_conda_types

## [0.2.14](https://github.com/conda/rattler/compare/rattler_config-v0.2.13...rattler_config-v0.2.14) - 2025-10-18

### Other

- updated the following local packages: rattler_conda_types

## [0.2.13](https://github.com/conda/rattler/compare/rattler_config-v0.2.12...rattler_config-v0.2.13) - 2025-10-17

### Other

- updated the following local packages: rattler_conda_types

## [0.2.12](https://github.com/conda/rattler/compare/rattler_config-v0.2.11...rattler_config-v0.2.12) - 2025-10-14

### Other

- updated the following local packages: rattler_conda_types

## [0.2.11](https://github.com/conda/rattler/compare/rattler_config-v0.2.10...rattler_config-v0.2.11) - 2025-10-03

### Other

- updated the following local packages: rattler_conda_types

## [0.2.10](https://github.com/conda/rattler/compare/rattler_config-v0.2.9...rattler_config-v0.2.10) - 2025-09-30

### Other

- updated the following local packages: rattler_conda_types

## [0.2.9](https://github.com/conda/rattler/compare/rattler_config-v0.2.8...rattler_config-v0.2.9) - 2025-09-05

### Other

- updated the following local packages: rattler_conda_types

## [0.2.8](https://github.com/conda/rattler/compare/rattler_config-v0.2.7...rattler_config-v0.2.8) - 2025-09-02

### Other

- updated the following local packages: rattler_conda_types

## [0.2.7](https://github.com/conda/rattler/compare/rattler_config-v0.2.6...rattler_config-v0.2.7) - 2025-08-15

### Other

- updated the following local packages: rattler_conda_types

## [0.2.6](https://github.com/conda/rattler/compare/rattler_config-v0.2.5...rattler_config-v0.2.6) - 2025-08-12

### Other

- updated the following local packages: rattler_conda_types

## [0.2.5](https://github.com/conda/rattler/compare/rattler_config-v0.2.4...rattler_config-v0.2.5) - 2025-07-23

### Other

- update Cargo.toml dependencies

## [0.2.4](https://github.com/conda/rattler/compare/rattler_config-v0.2.3...rattler_config-v0.2.4) - 2025-07-21

### Other

- update Cargo.toml dependencies

## [0.2.3](https://github.com/conda/rattler/compare/rattler_config-v0.2.2...rattler_config-v0.2.3) - 2025-07-14

### Other

- updated the following local packages: rattler_conda_types

## [0.2.2](https://github.com/conda/rattler/compare/rattler_config-v0.2.1...rattler_config-v0.2.2) - 2025-07-09

### Other

- updated the following local packages: rattler_conda_types

## [0.2.1](https://github.com/conda/rattler/compare/rattler_config-v0.2.0...rattler_config-v0.2.1) - 2025-07-01

### Fixed

- *(ci)* run pre-commit-run for all files ([#1481](https://github.com/conda/rattler/pull/1481))
- use kebab-case ([#1482](https://github.com/conda/rattler/pull/1482))

## [0.2.0](https://github.com/conda/rattler/compare/rattler_config-v0.1.1...rattler_config-v0.2.0) - 2025-06-26

### Other

- Fix typo ([#1479](https://github.com/conda/rattler/pull/1479))

## [0.1.1](https://github.com/conda/rattler/compare/rattler_config-v0.1.0...rattler_config-v0.1.1) - 2025-06-25

### Added

- *(rattler_index)* Use rattler_config ([#1466](https://github.com/conda/rattler/pull/1466))

## [0.1.0](https://github.com/conda/rattler/releases/tag/rattler_config-v0.1.0) - 2025-06-23

### Added

- add `rattler_config` crate (derived from `pixi_config`) ([#1389](https://github.com/conda/rattler/pull/1389))
- better readme ([#118](https://github.com/conda/rattler/pull/118))
- replace zulip with discord ([#116](https://github.com/conda/rattler/pull/116))
- move all conda types to separate crate

### Fixed

- added missing hyphen to relative url linking to what-is-conda section in README.md ([#1192](https://github.com/conda/rattler/pull/1192))
- typos ([#849](https://github.com/conda/rattler/pull/849))
- move more links to the conda org from conda-incubator ([#816](https://github.com/conda/rattler/pull/816))
- use conda-incubator
- add python docs badge
- typo libsolve -> libsolv ([#164](https://github.com/conda/rattler/pull/164))
- change urls from baszalmstra to mamba-org
- build badge

### Other

- update npm name ([#1368](https://github.com/conda/rattler/pull/1368))
- update readme ([#1364](https://github.com/conda/rattler/pull/1364))
- Fix badge style ([#1110](https://github.com/conda/rattler/pull/1110))
- fix anchor link ([#1035](https://github.com/conda/rattler/pull/1035))
- change links from conda-incubator to conda ([#813](https://github.com/conda/rattler/pull/813))
- update banner ([#808](https://github.com/conda/rattler/pull/808))
- update README.md
- add pixi badge ([#563](https://github.com/conda/rattler/pull/563))
- update installation gif
- update banner image
- address issue #282 ([#283](https://github.com/conda/rattler/pull/283))
- Add an image to Readme ([#203](https://github.com/conda/rattler/pull/203))
- Improve getting started with a micromamba environment. ([#163](https://github.com/conda/rattler/pull/163))
- Misc/update readme ([#66](https://github.com/conda/rattler/pull/66))
- update readme
- layout the vision a little bit better
- *(docs)* add build badge
- matchspec parsing
