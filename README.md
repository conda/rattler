# Rattler

![License][license-badge]
[![Build Status][build-badge]][build]
[![Project Chat][chat-badge]][chat-url]
[![docs main][docs-main-badge]][docs-main]

[license-badge]: https://img.shields.io/badge/license-BSD--3--Clause-blue?style=flat-square
[build-badge]: https://img.shields.io/github/actions/workflow/status/mamba-org/rattler/rust-compile.yml?style=flat-square&branch=main
[build]: https://github.com/mamba-org/rattler/actions
[chat-badge]: https://img.shields.io/discord/1082332781146800168.svg?label=&logo=discord&logoColor=ffffff&color=7389D8&labelColor=6A7EC2&style=flat-square
[chat-url]: https://discord.gg/kKV8ZxyzY4
[docs-main-badge]: https://img.shields.io/badge/docs-main-yellow.svg?style=flat-square
[docs-main]: https://mamba-org.github.io/rattler


Rattler is a library that provides common functionality used within the Conda ecosystem.
The goal of the library is to enable programs and other libraries to easily interact with the Conda ecosystem without being dependent on Python.
Its primary use case is as a library that you can use to provide Conda related workflows in your own tools.

Rattler is written in Rust and tries to provide a clean API to its functionalities. 
With the primary goal in mind we aim to provide bindings to different languages to make it easy to integrate Rattler in non-rust projects.

Why Rust?

* Out-of-the-box cross-platform support. (including WASM! 🙀)
* Easy to use and widely available existing crates to do heavy lifting.
* Support for async operations and easy to use multi-threading. 
* Strong memory- & thread safety guarantees.
* Very easy to generate statically linked binaries.
* Strongly typed. This ensures commonly used data formats have a formal specification.
* Support for binding to [Python](https://github.com/PyO3/pyo3), [C](https://github.com/eqrion/cbindgen), [C++](https://github.com/dtolnay/cxx), [Javascript](https://napi.rs/) and other languages.

## Features and plans

### Compatibility

Rattler strives to be compatible with the current ecosystem.

### Common datatypes

Rattler provides types for `Version`, `Channel`, `MatchSpec`, `VersionSpec` and many more commonly used types. 

It also provides data types and parsing of `index.json`, `repodata.json`, `channeldata.json`, and a few other files commonly found in Conda archives.

Parsing from string is also implemented for these types. 
Over the years a lot of complexity crept into these formats which makes this very non-trivial. 
Rattler is able to parse the most commonly used variants.

`Version` ordering is also implemented according to the Conda implementation. 
Common operations on `Version`, `VersionSpec` and `MatchSpec` are also implemented.

🚧 Rattler is able to provide the most common functionalities, but it has not been optimized for speed or size yet. 
A lot of functionality is also still missing. 

PR's are very welcome! 👋

### Solver

The plan is to provide a uniform API to solve an environment that also provides a user configurable way to provide caching for different components.

The API should hide the solver implementation. 
Initially this will use `libsolv` but in the future we could look more into [pubgrub](https://github.com/pubgrub-rs/pubgrub).

#### Challenges:

* Downloading and caching the repodata since different implementations might generate different caches or read from the repodata differently.
* Providing reasonable error messages. [pubgrub](https://github.com/pubgrub-rs/pubgrub) could really help here.

#### Use cases:

- Solve an environment
- Generate lock files

### Efficient repodata fetching 

RepoData is at the core of the Solver. 

- ✅ Download and cache RepoData from channels
- ✅ Parse Repodata from channels
- ✅ Terminal progress
- ✅ ZSTD support
- ✅ [RepoData State CEP](https://github.com/conda-incubator/ceps/pull/46)
- ✅ Improve API to provide better access to cache (libsolv has its own)
- 🚧 Improve API to provide better streaming support of RepoData.

### Virtual packages

Provide a simple API to work with supported virtual packages and natively detect which are present on the system.

Existing virtual packages are correctly discovered but could be extended by integrating `archspec` support.

- ✅ Datatypes for all currently existing virtual packages
- ✅ Detection of virtual packages for the current system
- 🚧 Support override detection with environment variables

### Installation of an environment

Given a set of explicit packages quickly construct or update an environment.

We can leverage some concurrency here to download, extract and install packages at the same time. 
Initial (hacky-dont-take-my-word-for-it) tests already showed a 20% performance improvement over micromamba.

### "Package streaming"

Anything that has to do with Conda package "archives".

- ✅ Provide an API to make it easy to download and extract a Conda package regardless of format.
- 🚧 Provide an API to *create* Conda packages from a directory.
- ✅ Extract only the `info` section of Conda packages without reading the entire file.
- 🚧 Stream only the `info` section of Conda packages from a URL.
- 🚧 Support downloading packages from other Source like S3 and OCI registries
- 🚧 Support authentication methods
