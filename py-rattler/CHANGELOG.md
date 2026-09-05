# Changelog

All notable changes to py-rattler will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.26.0] - 2026-08-27

### Highlights

**Solving got about twice as fast.** A direct release-to-release benchmark of `py-rattler-v0.25.0` (resolvo 0.10.3) against this release (0.12.1) produced these median solve times:

| Environment | 0.25.0 / 0.10.3 | 0.26.0 / 0.12.1 | Reduction |
|---|---:|---:|---:|
| Python 3.9 | 2.948 ms | 1.588 ms | 46.1% |
| xtensor/xsimd | 1.903 ms | 1.163 ms | 38.9% |
| TensorFlow | 152.068 ms | 65.047 ms | 57.2% |
| Quetz | 203.054 ms | 97.023 ms | 52.2% |
| TensorBoard/grpc | 74.354 ms | 30.696 ms | 58.7% |

**Read a single file out of a package without downloading it.** `PackageArchive` opens a `.conda` archive over HTTP range requests and pulls out only the entries you ask for. Inspecting metadata no longer costs a full download, and `list_files()` lists either archive section without reading its payloads. Works on local paths too.

```python
from rattler.package_streaming import PackageArchive

pkg = await PackageArchive.from_url(client, url)
index = await pkg.index_json()
info_files = await pkg.list_files("info")
recipe = await pkg.read_file("info/recipe/meta.yaml")
```

**Install local package archives directly.** `RepoDataRecord.from_package_archive()` builds the record that `install()` needs from a local `.conda` or `.tar.bz2` file. You no longer need to put the package in a channel or write `repodata.json` first.

**Solve from repodata you already have.** `solve` now accepts `SparseRepoData` alongside channels and `RepoDataSource`, so you can mix cached or locally produced repodata into a solve without routing it through the gateway. Both solve entry points also gained `add_pip_as_python_dependency`, mirroring conda's default of pulling `pip` in with Python.

```python
sparse = SparseRepoData(Channel("conda-forge"), "noarch", "noarch/repodata.json")
records = await solve([sparse], ["python 3.12.*"], add_pip_as_python_dependency=True)
```

**CEP-6 and CEP-42 are implemented.** The gateway surfaces CEP-6 notices now. Pass `channel_notices=True` to `query`/`names` and read them off the result, or fetch them directly with `Gateway.channel_notices(...)`. CEP-42 `channel_relations` are followed as well, see the breaking note below.

**`ChannelPriority.Flexible`.** Alongside `Strict` and `Disabled`, `Flexible` prefers packages from higher-priority channels but still falls back to lower ones when the top channel can't satisfy a requirement, matching conda's flexible priority.

**Breaking: `repodata_revisions` is a list now.** This is the only signature change in the release. The `vN`-keyed mapping introduced in 0.25.0 let callers hand-write revision statistics that the indexer already derives. Revisions are now selected by name, optionally with a publisher message:

```python
# before
await index_fs(channel_dir, repodata_revisions={"v3": {"n_packages": 1}})
# now
await index_fs(channel_dir, repodata_revisions=["v3"])
await index_fs(channel_dir, repodata_revisions=[{"revision": "v3", "message": "v3 packages"}])
```

Passing the old mapping raises a `TypeError` that spells out the new shape.

**Breaking: CEP-42 relations are followed by default.** `Gateway.query()`, `Gateway.names()` and `solve()` now follow a channel's declared `channel_relations`, pulling the related channels into the query and into the priority order; previously they were ignored. Only channels that publish CEP-42 metadata are affected. Cycles and failed fetches surface as `rattler.exceptions.GatewayWarning`, and `channel_relations="disabled"` restores the old behavior ([#2462](https://github.com/conda/rattler/pull/2462)).

### Added

- Build a `RepoDataRecord` from a local `.conda` or `.tar.bz2` archive with `await RepoDataRecord.from_package_archive(path)`, ready to pass to `install()`, in [#2698](https://github.com/conda/rattler/pull/2698)
- Add stable `MatchSpec.to_canonical_string()` output and `CanonicalMatchSpecError` for values the grammar cannot represent in [#2670](https://github.com/conda/rattler/pull/2670)
- Expose repodata revision metadata on `RepoData`, `ChannelInfo` and `SparseRepoData`, plus `PackageRecord.flags`, in [#2674](https://github.com/conda/rattler/pull/2674)
- Add the `networkx` install extra for `PackageRecord.to_graph()` in [#2725](https://github.com/conda/rattler/pull/2725)
- Publish wheels for Windows ARM64 and Linux RISC-V GNU and musl in [#2700](https://github.com/conda/rattler/pull/2700)
- `PackageArchive` for sparse reads from remote and local packages, with `read_file`, `read_files`, `list_files`, `index_json` and `paths_json` in [#2632](https://github.com/conda/rattler/pull/2632)
- Accept `SparseRepoData` directly as a `solve` source in [#2627](https://github.com/conda/rattler/pull/2627)
- `add_pip_as_python_dependency` on `solve` and `solve_with_sparse_repodata` in [#2677](https://github.com/conda/rattler/pull/2677)
- CEP-6 channel notices: `Gateway.channel_notices()` plus a `channel_notices` flag on `query` and `names` in [#2639](https://github.com/conda/rattler/pull/2639)
- `ChannelPriority.Flexible` in [#2617](https://github.com/conda/rattler/pull/2617)
- `alternative_target_prefix` on `Installer`, to patch a different prefix into hardcoded paths in [#2484](https://github.com/conda/rattler/pull/2484)
- iOS and Android subdirs with matching `__ios` and `__android` virtual packages and overrides in [#2613](https://github.com/conda/rattler/pull/2613)
- `emscripten-wasm64` platform in [#2680](https://github.com/conda/rattler/pull/2680)
- `cache_dir` on `VirtualPackage.detect()` in [#2568](https://github.com/conda/rattler/pull/2568)
- Signed entry-point launchers for Windows x86, x64 and arm64 in [#2493](https://github.com/conda/rattler/pull/2493)

### Changed

- **BREAKING:** `index_fs`/`index_s3` take `repodata_revisions` as a sequence of selections (`["v3"]` or `[{"revision": "v3", "message": ...}]`); the `vN`-keyed mapping and `RepodataRevisionMetadata` are gone in [#2669](https://github.com/conda/rattler/pull/2669)
- **BREAKING:** `Gateway.query()`, `Gateway.names()` and `solve()` follow CEP-42 `channel_relations` by default, so channels declaring relations contribute extra channels to the query and the priority order; the new `channel_relations` and `channel_relations_max_depth` arguments control it, and `"disabled"` restores the old behavior in [#2462](https://github.com/conda/rattler/pull/2462)
- Upgrade PyO3 to 0.29 in [#2545](https://github.com/conda/rattler/pull/2545) and [#2546](https://github.com/conda/rattler/pull/2546)
- Bound concurrent downloads with a semaphore in [#2475](https://github.com/conda/rattler/pull/2475)
- Record git `lfs` on source locations in the lock file in [#2633](https://github.com/conda/rattler/pull/2633)

### Fixed

- Make `MatchSpec` parsing and `str(MatchSpec)` round-trip safely in awkward cases; build-only specs now render as `foo[build="py39h123_0"]` instead of inventing a `*` version in [#2670](https://github.com/conda/rattler/pull/2670)
- Canonicalize `depends`, `constrains` and `extra_depends` when `index_fs`/`index_s3` write v3 repodata in [#2718](https://github.com/conda/rattler/pull/2718)
- Keep packages with legacy-compatible `extra_depends` in legacy repodata so older clients can see them in [#2721](https://github.com/conda/rattler/pull/2721)
- Deduplicate archives present in both legacy and v3 repodata and prefer the v3 record in [#2720](https://github.com/conda/rattler/pull/2720)
- Rate a solvable's repeated requirements on the same package by the most restrictive one, so `solve()` picks the same build for tied candidates across platforms in [#2649](https://github.com/conda/rattler/pull/2649)
- Fix a panic in the solver's conflict detection that could abort a `solve()` ([resolvo#231](https://github.com/prefix-dev/resolvo/pull/231))
- Accept quoted extras lists in `MatchSpec` (`foobar[extras=["science"]]`) and enforce CEP 44's group-name grammar in [#2552](https://github.com/conda/rattler/pull/2552)
- Keep a trailing underscore in a `MatchSpec` version instead of splitting it into the build string, per CEP 33: `tmux=3.7b_` is version `3.7b_` with no build in [#2606](https://github.com/conda/rattler/pull/2606)
- Follow the OCI registry's `WWW-Authenticate` challenge in [#2628](https://github.com/conda/rattler/pull/2628)
- Report a missing OCI manifest as a 404 in [#2651](https://github.com/conda/rattler/pull/2651) and retry a digest-addressed blob 404 through the manifest in [#2653](https://github.com/conda/rattler/pull/2653)
- Honor `max-age` without a `public` cache directive in [#2664](https://github.com/conda/rattler/pull/2664)
- Limit concurrent shard cache reads in the gateway in [#2163](https://github.com/conda/rattler/pull/2163)
- Detect read-only filesystems in the package cache instead of failing in [#2594](https://github.com/conda/rattler/pull/2594)
- Make the Windows package-cache rename retry actually fire in [#2555](https://github.com/conda/rattler/pull/2555)
- Include the sha256 in the cache key for `file://` packages in [#2507](https://github.com/conda/rattler/pull/2507)
- Use a valid regex when searching Windows keyring credentials in [#2564](https://github.com/conda/rattler/pull/2564)
- Write `repodata.json` atomically when indexing in [#2511](https://github.com/conda/rattler/pull/2511)
- Make shard creation deterministic in [#2553](https://github.com/conda/rattler/pull/2553)
- Preserve a leading `..` when normalizing `UrlOrPath`, so distinct relative paths no longer collapse in [#2548](https://github.com/conda/rattler/pull/2548)
- Escape environment variable values in activation scripts in [#2621](https://github.com/conda/rattler/pull/2621) and preserve newlines in them in [#2591](https://github.com/conda/rattler/pull/2591)
- Build the pty support on Android, OpenBSD and illumos in [#2533](https://github.com/conda/rattler/pull/2533), [#2524](https://github.com/conda/rattler/pull/2524) and [#2635](https://github.com/conda/rattler/pull/2635)
- Build the sdist with pax tar headers by requiring maturin 1.14 in [#2696](https://github.com/conda/rattler/pull/2696)

### Performance

- Cut solve times by 38.9% to 58.7% (51.2% geometric mean) from py-rattler 0.25.0 / resolvo 0.10.3 to 0.26.0 / 0.12.1, combining Resolvo's clause and hot-path work with Rattler's candidate-ordering changes in [#2520](https://github.com/conda/rattler/pull/2520), [#2609](https://github.com/conda/rattler/pull/2609), [#2711](https://github.com/conda/rattler/pull/2711) and [#2730](https://github.com/conda/rattler/pull/2730)
- Speed up version, version spec and match spec parsing in [#2515](https://github.com/conda/rattler/pull/2515)
- Speed up CUDA virtual package detection in [#2568](https://github.com/conda/rattler/pull/2568)
- Probe reflink support once per filesystem instead of per file in [#2508](https://github.com/conda/rattler/pull/2508)
- Avoid parsing match specs when checking dependency overrides in [#2506](https://github.com/conda/rattler/pull/2506)

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
