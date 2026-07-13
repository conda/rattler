# Changelog

All notable changes to py-rattler will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.25.0] - 2026-06-09

### Changed

- **BREAKING:** Rework the `repodata_revisions` indexing API to a `vN`-keyed dictionary and (de)serialize `info.repodata_revisions` as a dictionary. `index_fs`/`index_s3` now take `RepodataRevisions` (e.g. `{"v3": {"n_packages": 1}}`) with `oldest`/`newest` as `datetime`; `RepodataRevisionInfo` is replaced by `RepodataRevisions` and `RepodataRevisionMetadata` in [#2485](https://github.com/conda/rattler/pull/2485)

### Fixed

- Handle missing components when parsing packages: `AboutJson`, `IndexJson`, `PathsJson`, and `RunExportsJson`'s `from_remote_url` now return `None` instead of raising when the component is absent in [#2488](https://github.com/conda/rattler/pull/2488)

## [0.24.0] - 2026-05-20

### Added

- Add additional parameters to `Client` (auth storage, proxy config, cache dir, etc.) in [#2273](https://github.com/conda/rattler/pull/2273)
- Expose `extra_depends` on `PackageRecord` in [#2268](https://github.com/conda/rattler/pull/2268)
- Add support for [CEP-42](https://github.com/conda/ceps/blob/main/cep-0042.md) channel relations in repodata in [#2370](https://github.com/conda/rattler/pull/2370)
- Add repodata revisions as proposed in [conda/ceps#146](https://github.com/conda/ceps/pull/146) in [#2379](https://github.com/conda/rattler/pull/2379)
- Implement simplified variant selection with `flags` in [#2381](https://github.com/conda/rattler/pull/2381)
- Implement shell flavors and workspace-wide initialization in [#2259](https://github.com/conda/rattler/pull/2259)
- Handle HTTP 501 responses in sharded repodata fetching in [#2401](https://github.com/conda/rattler/pull/2401)
- Add `__cuda_arch` virtual package in [#1863](https://github.com/conda/rattler/pull/1863)
- Graduate extras, conditionals, and `flags` from experimental in [#2450](https://github.com/conda/rattler/pull/2450)
- Published wheels now include a CycloneDX SBOM of the Rust dependency tree under `.dist-info/sboms/` ([PEP 770](https://peps.python.org/pep-0770/))

### Changed

- **BREAKING:** Lockfile v7 — restructured format with platform-keyed environments, partial source records, source timestamps, and `run_exports` on source packages ([#2026](https://github.com/conda/rattler/pull/2026), [#2348](https://github.com/conda/rattler/pull/2348))
- **BREAKING:** Move `min_age` into `exclude_newer` and allow per-channel configuration in [#2279](https://github.com/conda/rattler/pull/2279)
- Replace `chrono` with `jiff` for date/time handling in [#1905](https://github.com/conda/rattler/pull/1905)

### Fixed

- Prevent package-cache path traversal via malicious build strings in untrusted channel metadata ([GHSA-h672-p7h7-97v9](https://github.com/conda/rattler/security/advisories/GHSA-h672-p7h7-97v9))
- Reject path traversal in Python entry points ([CVE-2026-47425](https://github.com/conda/rattler/security/advisories/GHSA-q53q-5r4j-5729)) in [#2445](https://github.com/conda/rattler/pull/2445)
- Make sdist PEP 625 conformant and trim bundled test data (roughly halves sdist size) in [#2470](https://github.com/conda/rattler/pull/2470)
- Retry temp-directory rename on transient Windows errors in [#2453](https://github.com/conda/rattler/pull/2453)
- Render conditional `when` dependencies as defined in CEP 43 in [#2436](https://github.com/conda/rattler/pull/2436)
- Avoid runtime import of `typing_extensions` in the index module in [#2428](https://github.com/conda/rattler/pull/2428)
- Make build string matching case-insensitive per CEP-29 in [#2386](https://github.com/conda/rattler/pull/2386)
- Fix ordering of `dev` and `post` components in version comparison in [#2299](https://github.com/conda/rattler/pull/2299)
- Fix `StrictVersion` `Ord` contract violation in [#2225](https://github.com/conda/rattler/pull/2225)
- Sort paths returned by `link_package_sync` for deterministic install output in [#2418](https://github.com/conda/rattler/pull/2418)
- Copy symlinked files when symbolic linking is disabled in [#2409](https://github.com/conda/rattler/pull/2409)
- Handle missing symlinks on Windows install path in [#2399](https://github.com/conda/rattler/pull/2399)
- Don't assume path is a `file://` URL in run-exports extraction in [#2411](https://github.com/conda/rattler/pull/2411)

### Performance

- Bump `resolvo` to 0.10.3, delivering an almost 2x solver speedup ([prefix-dev/resolvo#221](https://github.com/prefix-dev/resolvo/pull/221))
- Preserve `Arc` when crossing the Python custom-source boundary, improving solver performance with many custom sources in [#2400](https://github.com/conda/rattler/pull/2400)

## [0.23.2] - 2026-03-19

### Added

- Expose `WhlPackageRecord` to Python by @Anshgrover23 in [#2221](https://github.com/conda/rattler/pull/2221)
- Add custom progress reporter callbacks to installer by @ritankarsaha in [#2187](https://github.com/conda/rattler/pull/2187)
- Add FreeBSD 32-bit and ARM64 platform support by @wolfv in [#2227](https://github.com/conda/rattler/pull/2227)

### Changed

- Bump dependency versions in [#2237](https://github.com/conda/rattler/pull/2237)
- Improve Windows GUI app launching and file extension registration in [#2135](https://github.com/conda/rattler/pull/2135)

### Fixed

- Handle invalid characters in LibC family for virtual packages in [#2209](https://github.com/conda/rattler/pull/2209)
- Fall back to AWS SDK credential chain for S3 when no rattler credentials are set in [#2222](https://github.com/conda/rattler/pull/2222)
- Fix upload token matching for anaconda.org in [#2231](https://github.com/conda/rattler/pull/2231)
- Preserve mirror URL path when rewriting requests in [#2183](https://github.com/conda/rattler/pull/2183)
- Replace panicking unwrap/expect in mirror, S3, and GCS middleware in [#2216](https://github.com/conda/rattler/pull/2216)
- Keep removed package metadata in repodata in [#2210](https://github.com/conda/rattler/pull/2210)

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
