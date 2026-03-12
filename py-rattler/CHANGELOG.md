# Changelog

All notable changes to py-rattler will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.23.1] - 2026-03-10

### Added

- Add methods to download a file by @pavelzw in [#2201](https://github.com/conda/rattler/pull/2201)

## [0.23.0] - 2026-03-06

### Added

- Add support for range requests to download individual files from packages ([#1935](https://github.com/conda/rattler/pull/1935), [#2178](https://github.com/conda/rattler/pull/2178))
- Add `timeout` parameter to `Client` ([#2151](https://github.com/conda/rattler/pull/2151))
- Add `default_client` with built-in retry, OCI, GCS, and S3 middleware ([#2106](https://github.com/conda/rattler/pull/2106))
- Expose `archspec` in virtual package overrides ([#2019](https://github.com/conda/rattler/pull/2019))
- Implement comparison and hashing for `PackageRecord` types ([#2046](https://github.com/conda/rattler/pull/2046))
- Support glob and regex patterns in repodata queries ([#2036](https://github.com/conda/rattler/pull/2036))
- Add OAuth/OIDC authentication support in the authentication middleware ([#2049](https://github.com/conda/rattler/pull/2049))
- Add extra to AboutJson ([#2198](https://github.com/conda/rattler/pull/2198))

### Changed

- **BREAKING:** Standardize exception names to `*Error` suffix ([#2083](https://github.com/conda/rattler/pull/2083))
- **BREAKING:** Make `name` in `MatchSpec` non-optional ([#2132](https://github.com/conda/rattler/pull/2132))
- **BREAKING:** Remove support for JLAP; `jlap_enabled` is now deprecated and ignored in `FetchRepoDataOptions` and `SourceConfig` ([#2038](https://github.com/conda/rattler/pull/2038))
- **BREAKING:** Replace `; if` conditional dependency syntax with `when` key (e.g., `foo[when="python >=3.6"]` instead of `foo; if python >=3.6`) to align with the [conda CEP](https://github.com/conda/ceps/pull/111); the old syntax now raises an error ([#2007](https://github.com/conda/rattler/pull/2007))
- **BREAKING:** Restructure experimental repodata to use a `v3` top-level key with per-archive-type sub-maps (`conda`, `tar.bz2`, `whl`), replacing the separate `packages.whl` key, to align with the conda CEPs for [repodata v3](https://github.com/conda/ceps/pull/146), [conditional dependencies](https://github.com/conda/ceps/pull/111), and [wheel support](https://github.com/conda/ceps/pull/145) ([#2093](https://github.com/conda/rattler/pull/2093))
- Use `Arc<RepoDataRecord>` throughout the gateway and Python bindings, eliminating redundant copies when passing records between Python and Rust (e.g., parsing repodata and feeding it to the solver); also release the GIL during `SparseRepoData` construction to allow parallel channel loading ([#2061](https://github.com/conda/rattler/pull/2061))
- Replace `.conda` extraction with fully async `astral-async-zip`, improving package download and extraction performance ([#1855](https://github.com/conda/rattler/pull/1855))

### Fixed

- Fix type error for `channels` argument of `Environment` ([#2062](https://github.com/conda/rattler/pull/2062))
- Cache GCS OAuth2 token across requests ([#2114](https://github.com/conda/rattler/pull/2114))
- Reuse reqwest client in OCI middleware ([#2089](https://github.com/conda/rattler/pull/2089))
- Record actual link type in `PrefixRecord` instead of always writing `HardLink` ([#2169](https://github.com/conda/rattler/pull/2169))
- Fix bz2 cache state being overwritten with zst state in repodata cache ([#2180](https://github.com/conda/rattler/pull/2180))
- Enable deletion of memory-mapped repodata on Windows during concurrent fetches ([#2084](https://github.com/conda/rattler/pull/2084))
- Resolve data race in `BarrierCell` by using `compare_exchange` instead of `fetch_max` ([#2101](https://github.com/conda/rattler/pull/2101))
- Handle cleanup failures during installation without panicking ([#2088](https://github.com/conda/rattler/pull/2088))
- Replace panicking unwraps in `OCIUrl::new` with proper error handling ([#2162](https://github.com/conda/rattler/pull/2162))
- Fix track features package record ordering ([#2092](https://github.com/conda/rattler/pull/2092))
- Retry at least three times during install on broken pipe errors ([#2068](https://github.com/conda/rattler/pull/2068))
- Gracefully handle missing `$HOME` in file backend ([#2065](https://github.com/conda/rattler/pull/2065))
- Tolerate already-deleted conda-meta files during concurrent unlink ([#2060](https://github.com/conda/rattler/pull/2060))
- Cache negative credential lookups in auth middleware, significantly improving performance on hosts without stored credentials ([#2055](https://github.com/conda/rattler/pull/2055))
- Set modification time of patched files to ensure pyc files get updated ([#2096](https://github.com/conda/rattler/pull/2096))

### Performance

- Optimized repodata loading: up to 65x faster for in-memory queries and 2.4x faster for warm disk cache ([#2058](https://github.com/conda/rattler/pull/2058))
- Speed up matchspec parsing by ~2x ([#2066](https://github.com/conda/rattler/pull/2066))

[Unreleased]: https://github.com/conda/rattler/compare/py-rattler-v0.23.1...HEAD
[0.23.0]: https://github.com/conda/rattler/compare/py-rattler-v0.22.0...py-rattler-v0.23.0
[0.23.1]: https://github.com/conda/rattler/compare/py-rattler-v0.23.0...py-rattler-v0.23.1
