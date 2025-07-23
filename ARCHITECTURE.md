# Rattler (monorepo) Architecture

Rattler is a suite of Rust crates providing Conda functionality (indexing, solving, installing, etc.).
Its design reflects clear separation of concerns: each crate has a focused responsibility with well-defined interfaces.
This modularity enables reuse (e.g. Python/JS bindings reuse core crates), and isolates dependencies (e.g. a solve engine crate doesn’t depend on networking).
High-level tools (like the `rattler` CLI or *pixi*) orchestrate these crates: for example, solving and then installing an environment uses the `rattler_solve`, `rattler_index`, and `rattler` crates together.
The **“Components”** section of the README lists all crates and their roles, guiding new contributors through the architecture.
Overall, Rattler’s architecture emphasizes **simplicity and orthogonality**: each crate does one job (e.g. *“download/stream packages”* or *“activate environments”*) so that the system is easier to understand, test, and extend.

* **Core idea:** Provide strong types, memory efficient data-structures and performant algorithms to interact with the conda package ecosystem from Rust (and other languages) with a clean API.
  Rattler is *not* a monolithic tool but a library, so it splits functionality into crates that can be independently used or replaced.
* **High-level flow:** An end-to-end action (e.g. “solve and install Python 3.12”) involves several crates: network fetch (`rattler_networking`), read channel index (`rattler_repodata_gateway`), solve constraints (`rattler_solve`), download packages (`rattler_package_streaming`), cache files (`rattler_cache`), and finally set up the environment (`rattler_shell`, `rattler_pty`, etc.) under the orchestration of the main `rattler` crate.
* **Modularity:** By isolating things like **networking**, **solving**, and **package handling**, Rattler allows alternative implementations (e.g. one could plug in a different solver backend or a mock network layer) and limits the scope of each crate.
* **Bindings & Tools:** Most crates have **Python/JS bindings** via `py-rattler` and `@conda-org/rattler`.
  The example binary (`rattler-bin`) demonstrates how to wire all crates into a working package manager (see *“showcase”* in docs).

References: Rattler’s README and docs enumerate each crate’s role, and a Conda blog post notes its Rust foundations and fast solver.

## rattler\_conda\_types

This crate defines the core data models used across Rattler (versions, package specs, channel names, etc.).
Its types (e.g. `Version`, `MatchSpec`, repository metadata structs) are the *building blocks* for all higher-level crates.
By centralizing these definitions, Rattler ensures consistency (everyone uses the same `Version` type, same repo format, etc.) and avoids duplication.
Design decisions include **ser/deser support** (so these types can be read/written from TOML/JSON) and **source attribution** (e.g. tracking which channel a package record came from).
Historically, this crate evolved from Rust parsers of Conda’s YAML metadata; it abstracts Conda’s naming and versioning conventions into type-safe Rust structs.
In summary, `rattler_conda_types` is the foundation of the codebase, capturing “what a Conda package/environment is” so that other crates can manipulate these concepts reliably.

## rattler\_package\_streaming

This crate handles downloading and unpacking Conda package archives.
It provides functionality to fetch a `.tar.bz2` or `.conda` file (from a URL or local cache), stream it through decompression, and expose its contents (files, metadata).
Key design points: it abstracts I/O so that higher layers just ask “download this package” without worrying about compression details.
By separating package streaming, other parts (like `rattler_index` or `rattler_solve`) can deal with packages without having to worry about the internal workings of different package formats.
In practice, `rattler_package_streaming` is used by the install routines in the main `rattler` crate to fetch packages after solving.
This crate does not perform any dependency solving itself—it simply **handles package archives** (hence its name) and can be used independently wherever package files need processing.

## rattler\_repodata\_gateway

This crate is responsible for fetching and processing repodata (the *index* of available packages in a channel).
It downloads `repodata.json` (and optionally legacy index formats), parses it, and provides APIs to query package metadata (names, versions, dependencies, etc.).
Separating this crate lets the solver and install logic assume they already have the index loaded.
For example, during environment creation, Rattler will use `rattler_repodata_gateway` to load the channel indexes before solving.
Historically, this reflects Conda’s own design: Repodata is distinct from package archives, so Rattler cleanly mirrors that by having a dedicated “gateway” for the repodata layer.

## rattler\_shell

The `rattler_shell` crate encapsulates environment activation and command execution.
Its job is to modify a process’s environment (PATH, environment variables, etc.) and spawn subprocesses inside a given Conda environment prefix.
Design-wise, it knows about Conda’s activation scripts and directory layout, and it provides helpers to apply activation to the current process or a child process.
By extracting this logic into its own crate, Rattler ensures that creating an activated environment (e.g. “run Python from this env”) is a self-contained concern.
Other crates (like `rattler_menuinst`) also rely on `rattler_shell` to find prefixes or Python executables.
In practice, when the main `rattler` tool installs an environment, it will finally call into `rattler_shell` to set up the environment variables and launch the requested application.
This clear separation reflects Conda’s concept of an “activated shell” and keeps OS-specific details (Windows vs Unix paths, etc.) contained.

## rattler\_solve

The `rattler_solve` crate provides the dependency solver.
It is a backend-agnostic interface for solving the package satisfiability (SAT) problem: given requested package specs and a channel index, produce a list of concrete package versions to install.
Rattler’s solver wraps either the **libsolv** C library or the newer **resolvo** Rust solver.
Design goals include: no direct Conda/Python dependency (pure Rust), and flexibility to swap solver engines.
The API separates model (adding packages and constraints) from the solving step.
Other crates (and the `rattler` crate) call into `rattler_solve` when they need to figure out which packages satisfy a user’s request.
Its design is influenced by libsolv’s structures, but encapsulates them so that `rattler_solve` acts as a pure Rust library.
Because solving can be computationally intensive, it is a separate crate to avoid imposing solver dependencies on unrelated code.
Documentation notes it is “backend agnostic”, reflecting this pluggable design.

## rattler\_virtual\_packages

This crate detects system “virtual” packages (like `__unix`, `__linux`, `__glibc`, etc.) and represents them as packages for the solver.
Its job is to query the OS (CPU architecture, OS, glibc version, CPU features, etc.) and produce metadata so the solver knows what the system provides.
For example, if glibc 2.31 is present, it might add a virtual package `__glibc=2.31`.
The design is a simple mapping of system queries to `rattler_conda_types::PackageRecord`s; it has no external dependencies beyond syscalls.
By isolating this into `rattler_virtual_packages`, Rattler cleanly separates platform-detection from solving logic.
During solving (e.g. in `rattler_solve`), these virtual packages are simply additional “available” packages that represent host constraints.
This avoids hardcoding any OS logic in the solver or other crates.

## rattler\_index

This crate is for **building local channels** (indexes) from a set of package files.
Given a directory of `.tar.bz2` or `.conda` files, `rattler_index` creates the proper `repodata.json` and other index files so that it can serve as a Conda channel.
It parses package metadata, copies package files into place, and writes the index.
The main purpose is to support offline/local workflows and to mirror packages (e.g. “I have a local cache of packages, make it look like a channel”).
Design choices: it reuses `rattler_conda_types` for package metadata and writes JSON/TOML; it does not itself handle networking or solving.
In practice, tools like the main `rattler` binary or `rattler-build` can use `rattler_index` to manage local caches or construct channels from built packages.
Splitting it out means package installation can rely on standard repodata formats even for local sources.

## rattler (core crate)

The `rattler` crate is the high-level API that ties together solving, downloading, and installation.
It provides functions like `rattler::solve` and `rattler::install` that users (and the Python bindings) call.
Internally, it orchestrates the other crates: calling `rattler_solve` to get a solution, then using `rattler_package_streaming` (and `rattler_repodata_gateway`, `rattler_virtual_packages`, etc.) to perform the actual setup.
It is **async** so it can fetch packages in parallel.
Design-wise, `rattler` focuses on “how to create an environment from scratch” – for example, it will merge lockfile records into a solve, or apply activation scripts once installation is done.
This crate depends on most of the core crates but hides their complexity behind a simpler interface.
It exists so that other languages (via bindings) or higher-level tools (like Pixi) have a single entry point for environment management.
Its name “rattler” as a crate (with lower-case) reflects it being the main package, whereas the binary `rattler` (or `rattler-bin`) is an example application using this crate.

## rattler-lock

`rattler-lock` handles lockfile generation and parsing for reproducible builds.
Similar to `cargo.lock`, it can write a lockfile (likely TOML) that records exact package versions and sources for an environment.
It ensures that given a solve result, one can later recreate the same installation.
Design points: it defines a lockfile schema, and code to write/read it.
Other crates feed it data (the list of `PackageRecord`s from solving) and it produces a serialized lockfile.
This crate allows tools like `rattler-build` or other environment managers to use Rattler with pinned dependencies.
By having lockfiles in a dedicated crate, all aspects of serialization and versioning are isolated.
(Note: in code this crate is often named `rattler_lock` internally, but represents “lockfiles” functionality.)
Its existence underlines the emphasis on reproducibility in Rattler’s ecosystem.

## rattler-networking

This crate provides common network utilities for Rattler: HTTP requests, authentication, and cloud integrations.
It wraps libraries like `reqwest` and adds Conda-specific features (e.g. `.netrc` support, token redaction, S3/GCS clients).
Design highlights include pluggable auth (so private channels or cloud buckets can be accessed) and caching layers (optional).
For example, downloading a package or repodata typically goes through `rattler_networking`, which will handle credentials and retries.
By centralizing networking code, other crates stay focused on their domain (e.g. `rattler_repodata_gateway` just says “fetch URL” without detailing how).
Versioned optional features allow AWS/GCP SDKs to be pulled in only when needed.
In short, `rattler-networking` abstracts *“how we fetch stuff from the internet”* so that user-facing code can remain clean.

## rattler-bin

This is an example binary (CLI tool) that demonstrates using all Rattler crates together.
It’s not a library crate per se, but it’s in the workspace and serves as a reference for end-to-end usage.
Design-wise it shows how one might implement a package manager CLI using Rattler: parsing user commands, invoking `rattler::solve` and `rattler::install`, and using `rattler_shell` to run programs.
As such, `rattler-bin` is mostly for illustration and testing; it helps maintainers and new contributors see how the pieces fit together.
(In documentation this is sometimes called the *“showcase”* of Rattler.)
Its architecture is that of a typical command-line Rust app, but its key role is educational rather than functional in the library.

<!-- FIXME: Create diagram maybe -->
```
rattler-bin v0.1.0
├── rattler v0.34.7
│   ├── rattler_cache v0.3.26
│   │   ├── rattler_conda_types v0.35.6
│   │   │   ├── rattler_digest v1.1.4
│   │   │   ├── rattler_macros v1.0.11
│   │   │   └── rattler_redaction v0.1.12
│   │   ├── rattler_digest v1.1.4
│   │   ├── rattler_networking v0.25.6
│   │   └── rattler_package_streaming v0.22.45
│   │       ├── rattler_conda_types v0.35.6
│   │       ├── rattler_digest v1.1.4
│   │       ├── rattler_networking v0.25.6
│   │       └── rattler_redaction v0.1.12
│   ├── rattler_conda_types v0.35.6
│   ├── rattler_digest v1.1.4
│   ├── rattler_menuinst v0.2.17
│   │   ├── rattler_conda_types v0.35.6
│   │   └── rattler_shell v0.24.4
│   │       ├── rattler_conda_types v0.35.6
│   │       └── rattler_pty v0.2.4
│   ├── rattler_networking v0.25.6
│   ├── rattler_package_streaming v0.22.45
│   └── rattler_shell v0.24.4
├── rattler_cache v0.3.26
├── rattler_conda_types v0.35.6
├── rattler_menuinst v0.2.17
├── rattler_networking v0.25.6
├── rattler_repodata_gateway v0.23.7
│   ├── rattler_cache v0.3.26
│   ├── rattler_conda_types v0.35.6
│   ├── rattler_digest v1.1.4
│   ├── rattler_networking v0.25.6
│   └── rattler_redaction v0.1.12
├── rattler_solve v2.1.6
│   ├── rattler_conda_types v0.35.6
│   ├── rattler_digest v1.1.4
│   └── rattler_libsolv_c v1.2.3
└── rattler_virtual_packages v2.0.19
    └── rattler_conda_types v0.35.6
```

## rattler\_cache

The `rattler_cache` crate manages on-disk caching of Conda data.
It provides mechanisms to store downloaded packages, repodata, and run-export metadata in a cache directory (e.g. `$XDG_CACHE_HOME/rattler`).
Key modules include `package_cache` (for caching package archives after download) and `run_exports_cache` (for caching files extracted from packages).
The goal is to avoid redundant network or disk work: once a package is fetched, it can be reused from cache on subsequent runs.
By using this crate, the main Rattler tools can transparently speed up repeated operations.
In design terms, it defines standard cache locations and index formats, but does not itself download anything (it’s a layer between the filesystem and higher crates).
While not listed explicitly in the README, its presence is implied by Rattler’s focus on performance and reusability of data.

## rattler\_libsolv\_c

This crate provides low-level bindings to the libsolv C library (the traditional Conda SAT solver backend).
It wraps libsolv so that `rattler_solve` can use it via a safer Rust API.
Design points: it uses `bindgen`/`cc` to compile libsolv and exposes a minimal interface for the solver.
The rationale is to leverage libsolv’s battle-tested dependency solving while keeping the Rust codebase lightweight.
It does not depend on other Rattler crates (only C libs) and is kept minimal.
Newer Rattler versions may use the Rust-based *resolvo* solver instead, but having `rattler_libsolv_c` ensures compatibility and choice.
This crate effectively isolates all C code in one place.

## rattler\_digest

The `rattler_digest` crate offers hashing utilities used by multiple crates.
Its job is to compute and verify cryptographic hashes (SHA256, MD5, etc.) of packages and metadata, as required by Conda’s package format.
It provides simple functions to read from URLs or bytes and produce a `HashAlg`.
The design is a small wrapper around the `digest` family of crates.
By having a dedicated crate, Rattler ensures all hash computations are consistent (same algorithms and encoding) and avoids duplicating that code.
It is used by `rattler_package_streaming` (to check file integrity) and by lockfile generation.
Documentation calls it “a simple crate used by rattler crates to compute different hashes”.

## rattler\_macros

`rattler_macros` contains procedural macros for the Rattler project.
Currently it has only one macro that checks if fields of a struct or enum are alphabetically sorted.

## rattler\_menuinst

This crate handles installing and removing menu entries (Windows start menu shortcuts) for Conda packages.
It reads `Menu/*.json` files in a package and updates the system menu according to the metadata.
The design targets Windows and uses crates like `windows-registry` to modify shortcuts.
For example, after installing a package with menu entries, `rattler_menuinst` will write the appropriate `.lnk` files and register them.
It depends on `rattler_conda_types` (to read menu metadata) and `rattler_shell` (to locate the prefix).
Abstracting this into its own crate means menu logic is separate from core install logic.
If a package manager wants to support GUI apps on Windows, it can invoke `rattler_menuinst`; otherwise it can ignore it.

## rattler\_pty

The `rattler_pty` crate provides pseudo-terminal support to spawn interactive processes.
It offers an abstraction over Unix PTYs (and possibly Windows ConPTY) so that Rattler’s shell features can handle interactive programs (e.g. showing progress bars or colored output).
The design includes a `PtySession` struct (on Unix) that forks a process under a PTY and streams I/O.
By isolating this into a crate, Rattler can support terminal interactivity without polluting other code with low-level details.
It’s a small utility crate; in code it defines things like `PtyProcess` and options, which the `rattler_shell` or CLI might use when launching a program.
This separation allows the Rattler core to remain non-platform-specific, with only this crate handling PTY quirks.

## rattler\_redaction

This crate is for scrubbing sensitive information from URLs and text (e.g. removing API tokens).
It provides functions that take a URL or header and redact secrets before logging or error messages.
For instance, a private channel token in a URL will be replaced by `<hidden>`.
The design is straightforward: regex patterns or URL parsing to identify credentials.
By doing this in a dedicated crate, any part of Rattler that deals with URLs can call it to sanitize output.
This enhances security by ensuring no accidental leakage of tokens in logs.

## rattler\_config

`rattler_config` handles configuration management for Rattler and related tools.
It defines the format and parsing of `rattler.toml` (or similar) files that store settings like default channels, proxy settings, or feature flags.
The design choice here is to have one consistent config format for all Rattler-based applications (replacing older Pixi config crates).
By centralizing it, tools like `rattler-build` and Pixi can rely on the same config behavior.
This crate provides typed structs (via Serde) for config sections and fallbacks (e.g. environment variables).
Splitting config into its own crate allows other crates to depend on it without pulling in entire core libraries; for example, `rattler_networking` might use config to know default auth tokens.

## path\_resolver

The `path_resolver` crate (recently added) implements a **trie-based data structure** for tracking relative file paths across packages.
Its main use-case is when packages overwrite each other’s files: it can determine which package “owns” each installed file and how to remap or clobber them.
The architecture uses a trie keyed by relative paths, storing the order of insertion (which package came first).
For example, if two packages both provide `bin/foo`, `path_resolver` helps figure out that the second clobbers the first.
Its design is purely algorithmic (no external deps besides collections). The docs summarize it succinctly.
