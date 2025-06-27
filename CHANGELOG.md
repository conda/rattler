# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

# [0.18.0] - 2024-02-13

### 📃 Details

#### Added

* `ProgressBar` trait and progress bar for package writing by @wolfv ([#525](https://github.com/conda/rattler/pull/525))

#### Changed

* improved logging in package validation to include package path by @orhun ([#521](https://github.com/conda/rattler/pull/521))
* use resolvo 0.4.0 with better error messages by @baszalmstra ([#523](https://github.com/conda/rattler/pull/523))

#### Fixed

* allow multiple clobbers per package by @wolfv ([#526](https://github.com/conda/rattler/pull/526))
* remove drop-bomb, move empty folder removal to `post_process` by @wolfv ([#519](https://github.com/conda/rattler/pull/519))
* keep in mind python: noarch packages in clobber calculations by @wolfv ([#511](https://github.com/conda/rattler/pull/511))

# [0.17.0] - 2024-02-01

### ✨ Highlights

This release contains some big changes to rattler:

#### Consistent clobbering

Rattler installs packages in parallel but this was at the cost of not being able to resolve files properly that existed in multiple packages.
With this release we fixed this issue by creating a consistent clobbering experience.
When a file is clobbered (installed by multiple packages) the last package in the topological ordering wins.
This information is also recorded in the prefix itself which means that even if packages are added or removed from the environment the order remains consistent.

#### reqwest-middleware-client

The `AuthenticatedClient` has been rewritten by @vlad-ivanov-name.
Instead of having a custom client for network requests we now use the [`reqwest-middleware`](https://crates.io/crates/reqwest-middleware) crate.
The rattler implementation adds a middleware that handles authentication.
This changes makes it easier to integrate with other crates that use `reqwest` for network requests, and it allows users to add their own middleware.

#### Lock-file v4

The lock-file format has been updated to version 4.
Originally our implementation was semi-compatible with [conda-lock](https://github.com/conda/conda-lock).
We wanted to stay as close as possible to this format because it was already an established standard.
However, with version 2 and 3 of the format we started to diverge more and more.
We felt like the goals between both formats also started to diverge more and more so with version 4 we decided to completely abandon the conda-lock format and create our own.
For more information about the lock-file format and the differences between conda-lock you can [read the documentation](https://docs.rs/rattler_lock/0.17.0/rattler_lock).
Note that all old formats (including the original conda-lock format) can still be parsed by rattler.

### 📃 Details

#### Added

* Add `get_windows_launcher` function by @wolfv ([#477](https://github.com/conda/rattler/pull/477))
* Expose `get_windows_launcher` function by @wolfv ([#477](https://github.com/conda/rattler/pull/477))
* Consistent clobbering & removal of `__pycache__` by @wolfv ([#437](https://github.com/conda/rattler/pull/437))
* Add `name()` to `Channel` by @ruben-arts ([#495](https://github.com/conda/rattler/pull/495))
* Add timeout parameter to the solver by @wolfv ([#499](https://github.com/conda/rattler/pull/499))
* Add a very simple basic test to validate that we can at least parse netrc properly by @mariusvniekerk ([#503](https://github.com/conda/rattler/pull/503))

#### Changed

* Allow the full range of compression levels for zstd by @wolfv ([#479](https://github.com/conda/rattler/pull/479))
* Make compression conversion functions `pub` by @wolfv ([#480](https://github.com/conda/rattler/pull/480))
* Lock-file v4 by @baszalmstra ([#484](https://github.com/conda/rattler/pull/484))
* Convert authenticated client to reqwest middleware by @vlad-ivanov-name ([#488](https://github.com/conda/rattler/pull/488))
* Upgrade to latest resolvo main by @tdejager ([#497](https://github.com/conda/rattler/pull/497))
* Bump: resolvo 0.3.0 by @baszalmstra ([#500](https://github.com/conda/rattler/pull/500))

#### Fixed

* Copy over file permissions after reflink by @orhun ([#485](https://github.com/conda/rattler/pull/485))
* Fix clippy and deprecation warnings by @wolfv ([#490](https://github.com/conda/rattler/pull/490))
* Do not unwrap as much in clobberregistry by @wolfv ([#489](https://github.com/conda/rattler/pull/489))
* Fix warning for deref on a double reference by @wolfv ([#493](https://github.com/conda/rattler/pull/493))
* Fix self-clobbering when updating a package by @wolfv ([#494](https://github.com/conda/rattler/pull/494))
* Fix netrc parsing into BasicAuth by @wolfv ([#506](https://github.com/conda/rattler/pull/506))

### New Contributors
* @vlad-ivanov-name made their first contribution in https://github.com/conda/rattler/pull/482

# [0.16.2] - 2024-01-11

### 📃 Details

#### Fixed

* Reduce tracing level for reflink by @baszalmstra ([#475](https://github.com/conda/rattler/pull/475))

# [0.16.1] - 2024-01-09

### 📃 Details

#### Added

* Add `read_package_file` function by @wolfv ([#472](https://github.com/conda/rattler/pull/472))
* implement `Clone` for `AboutJson` by @0xbe7a ([#467](https://github.com/conda/rattler/pull/467))
* Allow using `str` in `HashMap`s with a `PackageName` key by @baszalmstra ([#468](https://github.com/conda/rattler/pull/468))

#### Changed

* Reflink files to destination if supported (instead of hardlinking) by @baszalmstra ([#463](https://github.com/conda/rattler/pull/463))

#### Fixed

* Automatic clippy fixes by @wolfv ([#470](https://github.com/conda/rattler/pull/470))
* Fix getting credentials from keyring error by @0xbe7a ([#474](https://github.com/conda/rattler/pull/474))


# [0.15.0] - 2024-01-05

### 📃 Details

#### Added

* Add ParseMatchSpecError and ParseMatchSpecError tests by @Johnwillliam ([#434](https://github.com/conda/rattler/pull/434))
* Add option to force usage of fallback_auth_store by @0xbe7a ([#435](https://github.com/conda/rattler/pull/435))
* New crate (rattler-index) with index functionality including python bindings by @BenjaminLowry ([#436](https://github.com/conda/rattler/pull/436))
* Add support for netrc files by @mariusvniekerk ([#395](https://github.com/conda/rattler/pull/395))

#### Changed

* Renamed `behaviour` to `behavior` ([#428](https://github.com/conda/rattler/pull/428))
* Enabled more clippy lints by @baszalmstra ([#462](https://github.com/conda/rattler/pull/462))
* Refactor `Version.bump()` to accept bumping `major/minor/patch/last` by @hadim ([#452](https://github.com/conda/rattler/pull/452))

#### Fixed

* Default value for `conda_packages` in repodata.json by @BenjaminLowry ([#441](https://github.com/conda/rattler/pull/441))
* Wildcard expansion for stored credentials of domains by @0xbe7a ([#442](https://github.com/conda/rattler/pull/442))
* Use serde default for proper serialization by @ruben-arts ([#443](https://github.com/conda/rattler/pull/443))
* Better detection of hardlinks and fallback to copy by @baszalmstra ([#461](https://github.com/conda/rattler/pull/461))
* Re-download the repodata cache if is out of sync/corrupt by @orhun ([#466](https://github.com/conda/rattler/pull/466))

# [0.14.0] - 2023-12-05

### 📃 Details

#### Added

* Options to disable `bz2` and `zstd` in `fetch_repo_data` ([#420](https://github.com/conda/rattler/pull/420))
* Support for powerpc64 and s390x ([#425](https://github.com/conda/rattler/pull/425))

#### Changed

* Renamed `behaviour` to `behavior` ([#428](https://github.com/conda/rattler/pull/428))

#### Fixed

* Recursive look for parent process name ([#424](https://github.com/conda/rattler/pull/424))
* Improve repodata fetch errors ([#426](https://github.com/conda/rattler/pull/426))
* Use filelock for authentication fallback storage  ([#427](https://github.com/conda/rattler/pull/427))
* Improved lockfile version mismatch error ([#423](https://github.com/conda/rattler/pull/423))

## [0.13.0] - 2023-11-27

### 📃 Details

#### Added

* Experimental support for purls in PackageRecord and derived datastructures ([#414](https://github.com/conda/rattler/pull/414))

#### Changed

* Rename `pip` to `pypi` in lockfile ([#415](https://github.com/conda/rattler/pull/415))

#### Fixed

* Allow compilation for android ([#418](https://github.com/conda/rattler/pull/418))
* Normalize relative-paths with writing to file ([#416](https://github.com/conda/rattler/pull/416))

## [0.12.3] - 2023-11-23

### 📃 Details

#### Fixed

* Expose missing `StringMatcherParseError` ([#410](https://github.com/conda/rattler/pull/410))
* Fix JLAP issue by setting the nominal hash when first downloading repodata ([#411](https://github.com/conda/rattler/pull/411))
* Support channel names with slashes ([#413](https://github.com/conda/rattler/pull/413))

## [0.12.2] - 2023-11-17

### 📃 Details

#### Fixed

* fix: make redaction work by using `From` explicitly ([#408](https://github.com/conda/rattler/pull/408))

## [0.12.1] - 2023-11-17

### 📃 Details

#### Fixed

* fix: redact tokens from urls in errors ([#407](https://github.com/conda/rattler/pull/407))

## [0.12.0] - 2023-11-14

### ✨ Highlights

Adds support for strict priority channel ordering, channel-specific selectors,

### 📃 Details

#### Added

* Add strict channel priority option ([#385](https://github.com/conda/rattler/pull/385))
* Add lock-file forward compatibility error ([#389](https://github.com/conda/rattler/pull/389))
* Add channel priority and channel-specific selectors to solver info ([#394](https://github.com/conda/rattler/pull/394))

#### Changed

* Channel in the `MatchSpec` struct changed to `Channel` type  ([#401](https://github.com/conda/rattler/pull/401))

#### Fixed

* Expose previous python version information in transaction ([#384](https://github.com/conda/rattler/pull/384))
* Avoid use of \ in doctest strings, for ide integration ([#387](https://github.com/conda/rattler/pull/387))
* Issue with JLAP using the wrong hash ([#390](https://github.com/conda/rattler/pull/390))
* Use the correct channel in the reason for exclude ([#397](https://github.com/conda/rattler/pull/397))
* Environment activation for windows ([#398](https://github.com/conda/rattler/pull/398))

## [0.11.0] - 2023-10-17

### ✨ Highlights

Lock file support has been moved into its own crate (rattler_lock) and support for pip packages has been added.

### 📃 Details

#### Changed

* change authentication fallback warnings to debug by @ruben-arts in https://github.com/conda/rattler/pull/365
* repodata cache now uses `.info.json` instead of `.state.json` by @dholth in https://github.com/conda/rattler/pull/377
* lock file now lives in its own crate with pip support by @baszalmstra in https://github.com/conda/rattler/pull/378

#### Fixed
* Nushell fixes by @wolfv in https://github.com/conda/rattler/pull/360
* Construct placeholder string at runtime to work around invalid conda prefix replacement by @baszalmstra in https://github.com/conda/rattler/pull/371
* xonsh extension by @ruben-arts in https://github.com/conda/rattler/pull/375

## New Contributors
* @dholth made their first contribution in https://github.com/conda/rattler/pull/377

**Full Changelog**: https://github.com/conda/rattler/compare/v0.10.0...v0.11.0

## [0.10.0] - 2023-10-02

### ✨ Highlights

The solver has been renamed and moved to its own repository: [resolvo](https://github.com/mamba-org/resolvo).
With the latest changes to the python bindings you can now download repodata and solve environments!
Still no official release of the bindings though, but getting closer every day.

### 📃 Details

#### Added

* add initial nushell support by @wolfv in [#271](https://github.com/conda/rattler/pull/271)

#### Changed

* the solver has been extracted in its own package: resolvo by @baszalmstra in [#349](https://github.com/conda/rattler/pull/349) & [#350](https://github.com/conda/rattler/pull/350)

#### Fixed

* Change solver implementation doc comment by @nichmor in [#352](https://github.com/conda/rattler/pull/352)

### 🐍 Python

* add more py-rattler types by @Wackyator in [#348](https://github.com/conda/rattler/pull/348)
* add fetch repo data to py-rattler by @Wackyator in [#334](https://github.com/conda/rattler/pull/334)
* use SparseRepoData in fetch_repo_data by @Wackyator in [#359](https://github.com/conda/rattler/pull/359)
* add solver by @Wackyator in [#361](https://github.com/conda/rattler/pull/361)

### 🤗 New Contributors
* @nichmor made their first contribution in [#352](https://github.com/conda/rattler/pull/352)

**Full Changelog**: https://github.com/conda/rattler/compare/v0.9.0...v0.10.0


## [0.9.0] - 2023-09-22

### ✨ Highlights

This is a pretty substantial release which includes many refactors to the solver (which we will pull out of this repository at some point), initial work on Python bindings, and many many fixes.

### 📃 Details

#### Added
* [pixi](https://github.com/prefix-dev/pixi) project to make contributing easier by @YeungOnion in [#283](https://github.com/conda/rattler/pull/283), [#342](https://github.com/conda/rattler/pull/342)
* make rattler-package-streaming compile with wasm by @wolfv in [#287](https://github.com/conda/rattler/pull/287)
* implement base_url cep by @baszalmstra in [#322](https://github.com/conda/rattler/pull/322)
* use emscripten-wasm32 and wasi-wasm32 by @wolfv in [#333](https://github.com/conda/rattler/pull/333)
* add build_spec module by @YeungOnion in [#340](https://github.com/conda/rattler/pull/340), [#346](https://github.com/conda/rattler/pull/346)

#### Changed

* use normalized package names where applicable by @baszalmstra in [#285](https://github.com/conda/rattler/pull/285)
* new `StrictVersion` type for VersionSpec ranges. by @tdejager in [#296](https://github.com/conda/rattler/pull/296)
* refactored ratter_libsolv_rs to be conda agnostic by @tdejager & @baszalmstra in [#316](https://github.com/conda/rattler/pull/316), [#309](https://github.com/conda/rattler/pull/309), [#317](https://github.com/conda/rattler/pull/317), [#320](https://github.com/conda/rattler/pull/320), [#319](https://github.com/conda/rattler/pull/319), [#323](https://github.com/conda/rattler/pull/323), [#324](https://github.com/conda/rattler/pull/324), [#328](https://github.com/conda/rattler/pull/328), [#325](https://github.com/conda/rattler/pull/325), [#326](https://github.com/conda/rattler/pull/326), [#335](https://github.com/conda/rattler/pull/335), [#336](https://github.com/conda/rattler/pull/336), [#338](https://github.com/conda/rattler/pull/338), [#343](https://github.com/conda/rattler/pull/343), [#337](https://github.com/conda/rattler/pull/337)
* feat: allow disabling jlap by @baszalmstra in [#327](https://github.com/conda/rattler/pull/327)
* test: added job to check for lfs links by @tdejager in [#331](https://github.com/conda/rattler/pull/331)
* hide implementation detail, version_spec::Constraint by @YeungOnion in [#341](https://github.com/conda/rattler/pull/341)

#### Fixed

* typo in solver error message by @baszalmstra in [#284](https://github.com/conda/rattler/pull/284)
* expose ParseMatchSpecError in rattler_conda_types by @Wackyator in [#286](https://github.com/conda/rattler/pull/286)
* use nvidia-smi on musl targets to detect Cuda by @baszalmstra in [#290](https://github.com/conda/rattler/pull/290)
* typo in snap file by @wolfv in [#291](https://github.com/conda/rattler/pull/291)
* Version::is_dev returning false for dev version (fix #289) by @Wackyator in [#293](https://github.com/conda/rattler/pull/293)
* workaround for `PIP_REQUIRE_VIRTUALENV` env variable by @tusharsadhwani in [#294](https://github.com/conda/rattler/pull/294)
* ensure consistent sorting of locked packages by @baszalmstra in [#295](https://github.com/conda/rattler/pull/295)
* updates to `NamelessMatchSpec` to allow deserializing by @travishathaway in [#299](https://github.com/conda/rattler/pull/299)
* update all dependencies and fix chrono deprecation by @wolfv in [#302](https://github.com/conda/rattler/pull/302)
* shell improvements for powershell env-var escaping and xonsh detection by @wolfv in [#307](https://github.com/conda/rattler/pull/307)
* also export strict version by @wolfv in [#312](https://github.com/conda/rattler/pull/312)
* make FetchRepoDataOptions cloneable by @Wackyator in [#321](https://github.com/conda/rattler/pull/321)
* bump json-patch 1.1.0 to fix stack overflow by @baszalmstra in [#332](https://github.com/conda/rattler/pull/332)
* emscripten is a unix variant by @wolfv in [#339](https://github.com/conda/rattler/pull/339)
* authentication fallback storage location by @ruben-arts in [#347](https://github.com/conda/rattler/pull/347)

### 🐍 Python

Although this release doesn't include a formal release of the python bindings yet, a lot of work has been done to work towards a first version.

* initial version of rattler python bindings by @baszalmstra in [#279](https://github.com/conda/rattler/pull/279)
* bind `Version`, `MatchSpec`, `NamelessMatchSpec` by @Wackyator in [#292](https://github.com/conda/rattler/pull/292)
* add more tests, hash and repr changes by @baszalmstra in [#300](https://github.com/conda/rattler/pull/300)
* add license by @Wackyator in [#301](https://github.com/conda/rattler/pull/301)
* bind channel types to py-rattler by @wolfv in [#313](https://github.com/conda/rattler/pull/313)
* bind everything necessary for shell activation by @wolfv in [#298](https://github.com/conda/rattler/pull/298)
* add mypy checks by @baszalmstra in [#314](https://github.com/conda/rattler/pull/314)
* bind `AuthenticatedClient` by @Wackyator in [#315](https://github.com/conda/rattler/pull/315)
* add `py.typed` file by @baszalmstra in [#318](https://github.com/conda/rattler/pull/318)
* bind `VersionWithSource` by @Wackyator in [#304](https://github.com/conda/rattler/pull/304)

### 🤗 New Contributors
* @Wackyator made their first contribution in [#286](https://github.com/conda/rattler/pull/286)
* @YeungOnion made their first contribution in [#283](https://github.com/conda/rattler/pull/283)
* @tusharsadhwani made their first contribution in [#294](https://github.com/conda/rattler/pull/294)

To all contributors, thank you for your amazing work on Rattler. This project wouldn't exist without you! 🙏

## [0.8.0] - 2023-08-22

### Highlights

This release contains bug fixes.

### Details

#### Added

- retry behavior when downloading package archives by @baszalmstra in ([#281](https://github.com/conda/rattler/pull/281))

#### Fixed

- parsing of local versions in `Constraint`s by @baszalmstra in ([#280](https://github.com/conda/rattler/pull/280))

## [0.7.0] - 2023-08-11

### Highlights

This release mostly contains bug fixes.

### Details

#### Added

- Rattler is now also build for Linux aarch64 in CI by @pavelzw in ([#272](https://github.com/conda/rattler/pull/272))
- `FromStr` for `ShellEnum` by @ruben-arts in ([#258](https://github.com/conda/rattler/pull/258))

#### Changed

- Run activation scripts and capture their output by @baszalmstra in ([#239](https://github.com/conda/rattler/pull/239))
- If memory mapping fails during installation the entire file is read instead by @baszalmstra in ([#273](https://github.com/conda/rattler/pull/273))
- Constraints parsing to improve performance and error messages by @baszalmstra in ([#254](https://github.com/conda/rattler/pull/254))
- Added explicit error in case repodata does not exist on the server by @ruben-arts in ([#256](https://github.com/conda/rattler/pull/256))
- Code signing on apple platform now uses `codesign` binary instead of `apple-codesign` crate by @wolfv in ([#259](https://github.com/conda/rattler/pull/259))

#### Fixed

- `Shell::run_command` ends with a newline by @baszalmstra in ([#262](https://github.com/conda/rattler/pull/262))
- Formatting of environment variable in fish by @ruben-arts in ([#264](https://github.com/conda/rattler/pull/264))
- Suppress stderr and stdout from codesigning by @wolfv in ([#265](https://github.com/conda/rattler/pull/265))
- All crates have at least basic documentation by @wolfv in ([#268](https://github.com/conda/rattler/pull/268))
- Use `default_cache_dir` in the rattler binary by @wolfv in ([#269](https://github.com/conda/rattler/pull/269))
- Corrupted tar files generated by rattler-package-streaming by @johnhany97 in ([#276](https://github.com/conda/rattler/pull/276))
- Superfluous quotes in the `content-hash` of conda lock files by @baszalmstra in ([#277](https://github.com/conda/rattler/pull/277))
- `subdir` and `arch` fields when converting `RepoDataRecord` to a `LockedDependency` in conda-lock format by @wolfv in ([#255](https://github.com/conda/rattler/pull/255))

## [0.6.0] - 2023-07-07

### Highlights

#### New rust based solver implementation

This version of rattler includes a new solver implementation!
@aochagavia worked hard on porting libsolv to rust and integrating that with `rattler_solve`.
The port performs slightly faster or similar to the original C code and does not contain unsafe code, is well documented, and thread-safe.
Our implementation (`rattler_libsolv_rs`) is specific to solving conda packages by leveraging `rattler_conda_types` for matching and parsing.

Some performance benchmarks taken on Apple M2 Max.

|                                    | libsolv-c    | libsolv-rs    |
| ---------------------------------- |--------------|---------------|
| python=3.9                         | 7.3734 ms    | **4.5831 ms** |
| xtensor, xsimd                     | 5.7521 ms    | **2.7643 ms** |
| tensorflow                         | 654.38 ms    | **371.59 ms** |
| quetz                              | **1.2577 s** | 1.3807 s      |
| tensorboard=2.1.1, grpc-cpp=1.39.1 | 474.76 ms    | **132.79 ms** |

> Run `cargo bench libsolv` to check the results on your own machine.

Besides the much improved implementation the new solver also provides much better error messages based on the work from mamba.
When a conflict is detected the incompatibilities are analyzed and displayed with a more user-friendly error message.

```
The following packages are incompatible
|-- asdf can be installed with any of the following options:
    |-- asdf 1.2.3 would require
        |-- C >1, which can be installed with any of the following options:
            |-- C 2.0.0
|-- C 1.0.0 is locked, but another version is required as reported above
```

`rattler-solve` has also been refactored to accommodate this change.
It is now more easily possible to switch between solvers add runtime by writing functions that are generic on the solver.
The solvers now live in a separate module `rattler_solve::libsolv_c` for the original libsolv C implementation and `rattler_solve::libsolv_rs` for the rust version.
Both solvers can be enabled with feature flags. The default features only select `libsolv_c`.

### Caching of activation scripts

This release contains code to execute an activation script and capture the changes it made to the environment.
Caching the result of an activation script can be useful if you need to invoke multiple executables from the same environment.

### Details

#### Added

- Run activation scripts and capture their output by @baszalmstra in ([#239](https://github.com/conda/rattler/pull/239))
- Support for sha256 and md5 field in matchspec by @0xbe7a in ([#241](https://github.com/conda/rattler/pull/241))
- A rust port of libsolv as an additional solver backend by @aochagavia, @baszalmstra in ([#243](https://github.com/conda/rattler/pull/243) & [#253](https://github.com/conda/rattler/pull/253))
- Test cases and benchmarks for solver implementations by @baszalmstra in ([#250](https://github.com/conda/rattler/pull/249) & [#250](https://github.com/conda/rattler/pull/249))
- The ability to add a dependency from `python` on `pip` while loading repodata @wolfv in ([#238](https://github.com/conda/rattler/pull/238))

#### Changed

- Completely refactored version parsing by @baszalmstra in ([#240](https://github.com/conda/rattler/pull/240))
- Refactored solver interface to allow generic use of solver implementations by @baszalmstra in ([#245](https://github.com/conda/rattler/pull/245))
- Also check if credentials stored under wildcard host by @wolfv in ([#252](https://github.com/conda/rattler/pull/252))

#### Fixed

- Compilation issues by @wolfv in ([#244](https://github.com/conda/rattler/pull/244))
- Add missing `From<VersionWithSource>` for `Version` by @baszalmstra in ([#246](https://github.com/conda/rattler/pull/246))
- Optimized libsolv port by removing redundant MatchSpec parsing by @baszalmstra in ([#246](https://github.com/conda/rattler/pull/246))
- Optimized libsolv port by caching matching Solvables by @baszalmstra in ([#251](https://github.com/conda/rattler/pull/251))

## [0.5.0] - 2023-06-30

### Highlights

A bug fix release

### Details

#### Added

- More control over how the PATH is altered during activation ([#232](https://github.com/conda/rattler/pull/232))

#### Fixed

- Reconstructing of RepoData from conda lock files for local channels ([#231](https://github.com/conda/rattler/pull/231))
- Powershell on Linux ([#234](https://github.com/conda/rattler/pull/234))
- Proper parsing of `>2.10*` as `>=2.10` ([#237](https://github.com/conda/rattler/pull/237))

## [0.4.0] - 2023-06-23

### Highlights

A new algorithm was introduced to sort `PackageRecord`s in a topological order based on their dependencies.
Sorting in this way provides a deterministic way of sorting packages in the order in which they should be installed to avoid clobbering.
The original algorithm was extracted from [rattler-server](https://github.com/conda/rattler-server).

Experimental extensions to the conda lock file format have also been introduced to make it possible to completely reproduce the original `RepoDataRecord`s from a lock file.

Fixes were made to the `MatchSpec` and `Version` implementation to catch some corner cases and detecting the current shell has become more robust.

### Details

#### Added

- `PackageRecord::sort_topologically` to perform a topological sort of `PackageRecord`s ([#218](https://github.com/conda/rattler/pull/218))
- Experimental fields to be able to reconstruct `RepoDataRecord` from conda lock files. ([#221](https://github.com/conda/rattler/pull/221))
- Methods to manipulate `Version`s ([#229](https://github.com/conda/rattler/pull/229))

#### Changed

- Refactored shell detection code using `$SHELL` or parent process name ([#219](https://github.com/conda/rattler/pull/219))
- The error message that is thrown when parsing a `Platform` now includes possible options ([#222](https://github.com/conda/rattler/pull/222))
- Completely refactored `Version` implementation to reduce memory footprint and increase readability ([#227](https://github.com/conda/rattler/pull/227))

#### Fixed

- Issue with parsing matchspecs that contain epochs ([#220](https://github.com/conda/rattler/pull/220))
- Zsh activation scripts invoke .sh scripts ([#223](https://github.com/conda/rattler/pull/223))
- Detect the proper powershell parent process ([#224](https://github.com/conda/rattler/pull/224))

## [0.3.0] - 2023-06-15

### Highlights

This release contains lots of fixes, small (breaking) changes, and new features.
The biggest highlights are:

#### JLAP support

JLAP is a file format to incrementally update a cached `repodata.json` without downloading the entire file.
This can save a huge amount of bandwidth for large repodatas that change often (like those from conda-forge).
If you have a previously cached `repodata.json` on your system only small JSON patches are downloaded to bring your cache up to date.
The format was initially proposed through a [CEP](https://github.com/conda/ceps/pull/20) and has been available in conda as an experimental feature since `23.3.0`.

When using rattler you get JLAP support out of the box.
No changes are needed.

#### Support for local `file://`

`file://` based urls are now supported for all functions that use a Url to download certain data.

#### `rattler_networking`

A new crate has been added to facilitate authentication when downloading repodata or packages called `rattler_networking`.

### Details

#### Added

- Support for detecting more platforms ([#135](https://github.com/conda/rattler/pull/135))
- `RepoData` is now cloneable ([#138](https://github.com/conda/rattler/pull/138))
- `RunExportsJson` is now cloneable ([#169](https://github.com/conda/rattler/pull/169))
- `file://` urls are now supported for package extraction functions ([#157](https://github.com/conda/rattler/pull/157))
- `file://` urls are now supported for repodata fetching ([#158](https://github.com/conda/rattler/pull/158))
- Getting started with rattler using micromamba ([#163](https://github.com/conda/rattler/pull/163))
- Add `Platform::arch` function to return the architecture of a given platform ([#166](https://github.com/conda/rattler/pull/166))
- Extracted Python style JSON formatting into [a separate crate](https://github.com/prefix-dev/serde-json-python-formatter) ([#163](https://github.com/conda/rattler/pull/180))
- Added feature to use `rustls` with `rattler_package_streaming` and `rattler_repodata_gateway` ([#179](https://github.com/conda/rattler/pull/179) & [#181](https://github.com/conda/rattler/pull/181))
- Expose `version_spec` module ([#183](https://github.com/conda/rattler/pull/183))
- `NamelessMatchSpec` a variant of `MatchSpec` that does not include a package name [#185](https://github.com/conda/rattler/pull/185))
- `ShellEnum` - a dynamic shell type for dynamic discovery [#187](https://github.com/conda/rattler/pull/187))
- Exposed the `python_entry_point_template` function ([#190](https://github.com/conda/rattler/pull/190))
- Enable deserializing virtual packages ([#198](https://github.com/conda/rattler/pull/198))
- Refactored CI to add macOS arm64 ([#201](https://github.com/conda/rattler/pull/201))
- Support for JLAP when downloading repodata ([#197](https://github.com/conda/rattler/pull/197) & [#214](https://github.com/conda/rattler/pull/214))
- `Clone`, `Debug`, `PartialEq`, `Eq` implementations for conda lock types ([#213](https://github.com/conda/rattler/pull/213))
- `rattler_networking` to enable accessing `repodata.json` and packages that require authentication ([#191](https://github.com/conda/rattler/pull/191))

#### Changed

- `FileMode` is now included with `prefix_placeholder` is set ([#136](https://github.com/conda/rattler/pull/135))
- `rattler_digest` now re-exports commonly used hash types and typed hashes are now used in more placed (instead of strings) [[#137](https://github.com/conda/rattler/pull/137) & [#153](https://github.com/conda/rattler/pull/153)]
- Use `Platform` in to detect running operating system ([#144](https://github.com/conda/rattler/pull/144))
- `paths.json` is now serialized in a deterministic fashion ([#147](https://github.com/conda/rattler/pull/147))
- Determine the `subdir` for the `platform` and `arch` fields when creating a `PackageRecord` from an `index.json` ([#145](https://github.com/conda/rattler/pull/145) & [#152](https://github.com/conda/rattler/pull/152))
- `Activator::activation` now returns the new `PATH` in addition to the script ([#151](https://github.com/conda/rattler/pull/151))
- Use properly typed `chrono::DateTime<chrono::Utc>` for timestamps instead of `u64` ([#157](https://github.com/conda/rattler/pull/157))
- Made `ParseError` public and reuse `ArchiveType` ([#167](https://github.com/conda/rattler/pull/167))
- Allow setting timestamps when creating package archive ([#171](https://github.com/conda/rattler/pull/171))
- `about.json` and `index.json` are now serialized in a deterministic fashion ([#180](https://github.com/conda/rattler/pull/180))
- SHA256 and MD5 hashes are computed on the fly when extracting packages ([#176](https://github.com/conda/rattler/pull/176)
- Change blake2 hash to use blake2b instead of blake2s ([#192](https://github.com/conda/rattler/pull/192)
- LibSolv error messages are now passed through ([#202](https://github.com/conda/rattler/pull/202) & [#210](https://github.com/conda/rattler/pull/210))
- `VersionTree` parsing now uses `nom` instead of a complex regex ([#206](https://github.com/conda/rattler/pull/206)
- `libc` version detection now uses `lld --version` to properly detect the libc version on the host ([#209](https://github.com/conda/rattler/pull/209)
- Improved version parse error messages ([#211](https://github.com/conda/rattler/pull/211)
- Parsing of some complex MatchSpecs ([#217](https://github.com/conda/rattler/pull/217)

#### Fixed

- MatchSpec bracket list parsing can now handle quoted values ([#157](https://github.com/conda/rattler/pull/156))
- Typos and documentation ([#164](https://github.com/conda/rattler/pull/164) & [#188](https://github.com/conda/rattler/pull/188))
- Allow downloading of repodata.json to fail in some cases (only noarch is a required subdir) ([#174](https://github.com/conda/rattler/pull/174))
- Missing feature when using the sparse index ([#182](https://github.com/conda/rattler/pull/182))
- Several small issues or missing functionality ([#184](https://github.com/conda/rattler/pull/184))
- Loosened strictness of comparing packages in `Transaction`s ([#186](https://github.com/conda/rattler/pull/186)
- Missing `noarch: generic` parsing in `links.json` ([#189](https://github.com/conda/rattler/pull/189)
- Ignore trailing .0 in version comparison ([#196](https://github.com/conda/rattler/pull/196)

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

[unreleased]: https://github.com/conda/rattler/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/conda/rattler/releases/tag/v0.1.0
