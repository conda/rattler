# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.5](https://github.com/conda/rattler/compare/rattler-bin-v0.1.4...rattler-bin-v0.1.5) - 2026-04-13

### Fixed

- CLI crash on malformed virtual package version ([#2327](https://github.com/conda/rattler/pull/2327))

## [0.1.4](https://github.com/conda/rattler/compare/rattler-bin-v0.1.3...rattler-bin-v0.1.4) - 2026-04-08

### Other

- updated the following local packages: rattler_conda_types, rattler_shell, rattler_upload, rattler_networking, rattler_package_streaming, rattler_cache, rattler_menuinst, rattler, rattler_solve, rattler_repodata_gateway, rattler_virtual_packages

## [0.1.3](https://github.com/conda/rattler/compare/rattler-bin-v0.1.2...rattler-bin-v0.1.3) - 2026-04-07

### Added

- Add CLI run command ([#2263](https://github.com/conda/rattler/pull/2263))
- Implement `rattler list` subcommand (and smaller `PrefixData` improvements) ([#2266](https://github.com/conda/rattler/pull/2266))
- *(bin)* Add `rattler completion` command ([#2293](https://github.com/conda/rattler/pull/2293))

## [0.1.2](https://github.com/conda/rattler/compare/rattler-bin-v0.1.1...rattler-bin-v0.1.2) - 2026-03-27

### Added

- *(solve)* Move min_age into exclude_newer, add per-channel configuration ([#2279](https://github.com/conda/rattler/pull/2279))
- Add `rattler shell-hook` ([#2290](https://github.com/conda/rattler/pull/2290))
- Add `rattler download` command, improve `rattler --help` ([#2272](https://github.com/conda/rattler/pull/2272))

## [0.1.1](https://github.com/conda/rattler/compare/rattler-bin-v0.1.0...rattler-bin-v0.1.1) - 2026-03-25

### Other

- *(rattler-bin)* release v0.1.0 ([#2270](https://github.com/conda/rattler/pull/2270))

## [0.1.0](https://github.com/conda/rattler/releases/tag/rattler-bin-v0.1.0) - 2026-03-21

### Added

- `rattler create` doc improvements and `conda create` alignment ([#2264](https://github.com/conda/rattler/pull/2264))
- Add support for downloading info files via range requests ([#1935](https://github.com/conda/rattler/pull/1935))
- support glob and regex patterns in repodata queries ([#2036](https://github.com/conda/rattler/pull/2036))
- Add CACHEDIR.TAG to environments and global cache ([#2011](https://github.com/conda/rattler/pull/2011))
- add `rattler extract` utility to the rattler-bin crate ([#1832](https://github.com/conda/rattler/pull/1832))
- implement `--exclude-newer` flag for `rattler create` command ([#1815](https://github.com/conda/rattler/pull/1815))
- derive default credentials from aws sdk ([#1629](https://github.com/conda/rattler/pull/1629))
- ability to ignore packages in the installer ([#1612](https://github.com/conda/rattler/pull/1612))
- populate `requested_spec` ([#1596](https://github.com/conda/rattler/pull/1596))
- implement extras with conditional dependencies ([#1542](https://github.com/conda/rattler/pull/1542))
- make rattler_networking system integration optional ([#1381](https://github.com/conda/rattler/pull/1381))
- add reinstallation method to installer and transaction ([#1128](https://github.com/conda/rattler/pull/1128))
- add `rattler_menuinst` crate ([#840](https://github.com/conda/rattler/pull/840))
- implement `--no-deps` and `--only-deps` ([#1068](https://github.com/conda/rattler/pull/1068))
- add S3 support ([#1008](https://github.com/conda/rattler/pull/1008))
- Add support for optional dependencies ([#1019](https://github.com/conda/rattler/pull/1019))
- speed up `PrefixRecord` loading ([#984](https://github.com/conda/rattler/pull/984))
- improve performance when linking files using `rayon` ([#985](https://github.com/conda/rattler/pull/985))
- merge pixi-build branch ([#950](https://github.com/conda/rattler/pull/950))
- start adding interface to override ([#834](https://github.com/conda/rattler/pull/834))
- add direct url repodata building ([#725](https://github.com/conda/rattler/pull/725))
- add solve strategies ([#660](https://github.com/conda/rattler/pull/660))
- exclude repodata records based on timestamp ([#654](https://github.com/conda/rattler/pull/654))
- high level repodata access ([#560](https://github.com/conda/rattler/pull/560))
- add channel priority to solve task and expose to python solve ([#598](https://github.com/conda/rattler/pull/598))
- make root dir configurable in channel config ([#602](https://github.com/conda/rattler/pull/602))
- proper archspec detection using archspec-rs ([#584](https://github.com/conda/rattler/pull/584))
- [**breaking**] optional strict parsing of matchspec and versionspec ([#552](https://github.com/conda/rattler/pull/552))
- use resolvo 0.4.0 ([#523](https://github.com/conda/rattler/pull/523))
- add timeout parameter and SolverOptions to return early ([#499](https://github.com/conda/rattler/pull/499))
- implement separate auth stores and allow using only disk auth ([#435](https://github.com/conda/rattler/pull/435))
- add channel priority and channel-specific selectors to solver info ([#394](https://github.com/conda/rattler/pull/394))
- add strict channel priority option ([#385](https://github.com/conda/rattler/pull/385))
- also fix bench
- use PackageName everywhere
- normalize package names where applicable
- add rattler_networking and AuthenticatedClient to perform authenticated requests ([#191](https://github.com/conda/rattler/pull/191))
- prepare for release ([#119](https://github.com/conda/rattler/pull/119))
- better readme ([#118](https://github.com/conda/rattler/pull/118))
- replace zulip with discord ([#116](https://github.com/conda/rattler/pull/116))
- extra methods to query spare repodata ([#110](https://github.com/conda/rattler/pull/110))
- allow caching repodata.json as .solv file ([#85](https://github.com/conda/rattler/pull/85))
- implement sparse repodata loading ([#89](https://github.com/conda/rattler/pull/89))
- create command ([#72](https://github.com/conda/rattler/pull/72))
- stateless solver ([#75](https://github.com/conda/rattler/pull/75))
- support installed (virtual) packages in libsolv ([#51](https://github.com/conda/rattler/pull/51))
- download and cache repodata.json ([#55](https://github.com/conda/rattler/pull/55))
- move all conda types to separate crate
- data models for extracting channel information ([#14](https://github.com/conda/rattler/pull/14))

### Fixed

- set AWS_LC_SYS_CMAKE_BUILDER in pixi-build package configs ([#2241](https://github.com/conda/rattler/pull/2241))
- *(networking)* cache GCS OAuth2 token across requests ([#2114](https://github.com/conda/rattler/pull/2114))
- reuse reqwest client in OCI middleware ([#2089](https://github.com/conda/rattler/pull/2089))
- more reproducible builds with pixi install and source date epoch ([#1956](https://github.com/conda/rattler/pull/1956))
- rename solver argument from lib-solv to libsolv ([#1833](https://github.com/conda/rattler/pull/1833))
- fix login authentication ([#1600](https://github.com/conda/rattler/pull/1600))
- *(ci)* run pre-commit-run for all files ([#1481](https://github.com/conda/rattler/pull/1481))
- consistent usage of rustls-tls / native-tls feature ([#1324](https://github.com/conda/rattler/pull/1324))
- added missing hyphen to relative url linking to what-is-conda section in README.md ([#1192](https://github.com/conda/rattler/pull/1192))
- use new PackageRecord when issuing reinstallation in `Transaction::from_current_and_desired` ([#1070](https://github.com/conda/rattler/pull/1070))
- fix-up shebangs with spaces ([#887](https://github.com/conda/rattler/pull/887))
- typos ([#849](https://github.com/conda/rattler/pull/849))
- allow `gcs://` and `oci://` in gateway ([#845](https://github.com/conda/rattler/pull/845))
- move more links to the conda org from conda-incubator ([#816](https://github.com/conda/rattler/pull/816))
- use conda-incubator
- use the output of `readlink` as hash for softlinks ([#643](https://github.com/conda/rattler/pull/643))
- better value for `link` field ([#610](https://github.com/conda/rattler/pull/610))
- no shebang on windows to make spaces in prefix work ([#611](https://github.com/conda/rattler/pull/611))
- run post-link scripts ([#574](https://github.com/conda/rattler/pull/574))
- dont use workspace dependencies for local crates ([#546](https://github.com/conda/rattler/pull/546))
- initial releaze-pls config
- remove drop-bomb, move empty folder removal to `post_process` ([#519](https://github.com/conda/rattler/pull/519))
- consistent clobbering & removal of `__pycache__` ([#437](https://github.com/conda/rattler/pull/437))
- add python docs badge
- make FetchRepoDataOptions cloneable ([#321](https://github.com/conda/rattler/pull/321))
- add retry behavior for package cache downloads ([#280](https://github.com/conda/rattler/pull/280))
- clippy warnings
- allow downloading of repodata.json to fail for arch specific channels ([#174](https://github.com/conda/rattler/pull/174))
- typo libsolve -> libsolv ([#164](https://github.com/conda/rattler/pull/164))
- change urls from baszalmstra to mamba-org
- build badge
- tests and clippy
- remove dependencies required for the library ([#15](https://github.com/conda/rattler/pull/15))

### Other

- Publish rattler-bin ([#2269](https://github.com/conda/rattler/pull/2269))
- Improve range request API, fix 416 issue ([#2199](https://github.com/conda/rattler/pull/2199))
- Typo and clippy fixes ([#2047](https://github.com/conda/rattler/pull/2047))
- Improve AuthenticationStorage ([#1026](https://github.com/conda/rattler/pull/1026))
- enable using sharded repodata for custom channels ([#910](https://github.com/conda/rattler/pull/910))
- update all versions of packages ([#886](https://github.com/conda/rattler/pull/886))
- make virtual package overrides none by default consistently ([#842](https://github.com/conda/rattler/pull/842))
