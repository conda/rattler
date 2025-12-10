# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

`rattler_repodata_gateway` is a crate that provides functionality to interact with Conda repodata. It handles downloading, caching, and querying `repodata.json` files from conda channels. This crate is part of the larger Rattler workspace, which provides Rust implementations for working with the conda ecosystem.

## Build & Test Commands

This is a workspace member of the Rattler monorepo. All commands should be run from the workspace root (`/Users/travishathaway/dev/rattler`), not from this crate's directory.

### Building
```bash
# From workspace root
pixi run build

# Or directly with cargo
cargo build -p rattler_repodata_gateway
```

### Testing
```bash
# Run all tests in workspace (from root)
pixi run test

# Run tests for this crate only
cargo nextest run -p rattler_repodata_gateway --no-default-features --features=sparse,gateway

# Run specific test
cargo nextest run -p rattler_repodata_gateway test_name
```

### Linting & Formatting
```bash
# From workspace root
pixi run lint-fast    # Fast linters (no clippy)
pixi run lint-slow    # Includes clippy
pixi run cargo-fmt    # Format code
pixi run cargo-clippy # Run clippy
```

## Architecture

### Feature Gates

This crate uses feature flags extensively:

- **`sparse`**: Enables sparse repodata loading, which allows loading only specific package records from `repodata.json` instead of parsing the entire file. This is a memory optimization for large repositories.
- **`gateway`**: Enables the high-level `Gateway` API for querying repodata across multiple channels with caching and deduplication.
- **`indicatif`**: Adds progress reporting using the `indicatif` crate.

Default features: `rustls-tls`

### Module Structure

**`fetch/`**: Low-level repodata fetching and caching
- Handles HTTP requests, cache management, and file locking
- Supports JLAP (incremental updates) for efficient repodata updates
- Provides both cached and non-cached fetch strategies
- Entry point: `fetch::fetch_repo_data()` function

**`sparse/`**: Sparse repodata loading (feature-gated)
- `SparseRepoData` allows querying specific packages from repodata.json without loading everything
- Memory-mapped file access for efficient partial reads
- Supports different package format selection strategies (tar.bz2, conda, or both)

**`gateway/`**: High-level API for multi-channel queries (feature-gated)
- `Gateway` is the main entry point - a thread-safe, reference-counted struct
- Request deduplication: multiple concurrent requests for the same repodata are coalesced
- Supports both remote and local channels
- `GatewayBuilder` provides fluent configuration API
- Subdirectories (`subdir/`) represent channel+platform combinations (e.g., conda-forge/linux-64)
- Sharded subdirectories support for large channels

**`reporter/`**: Progress reporting traits
- `DownloadReporter`, `JLAPReporter`, `Reporter` traits
- Allows custom progress reporting implementations

### Async Architecture

Heavy use of `async/await` with Tokio runtime:
- All I/O operations are async
- File operations use `tokio::fs`
- HTTP operations use `reqwest` with middleware support
- Cross-platform support including WASM (uses `wasmtimer` on WASM)

### Caching Strategy

Repodata is cached locally with:
- HTTP cache semantics (ETags, Last-Modified headers)
- File-based locking to prevent concurrent writes
- Blake2 hashing for integrity verification
- Compression support (gzip, bzip2, zstd)
- JLAP support for incremental updates

### Typical Flow

1. **Query**: User calls `Gateway::query()` with channels and platforms
2. **Subdir Selection**: Gateway identifies required subdirectories (channel+platform pairs)
3. **Deduplication**: Concurrent requests for same subdir are coalesced using `BarrierCell`
4. **Fetch**: If not cached or stale, fetch repodata from remote or extract from local channel
5. **Parse**: Parse JSON into `RepoDataRecord` structs
6. **Cache**: Store parsed data and update cache metadata
7. **Query**: Filter records based on `MatchSpec` criteria
8. **Return**: Return matching `RepoDataRecord` objects

### Important Types

- `Gateway`: Main entry point for high-level queries
- `GatewayBuilder`: Builder pattern for Gateway configuration
- `RepoData`: Parsed repodata with package records
- `SparseRepoData`: Memory-efficient partial repodata loading
- `ChannelConfig`: Configuration for a channel (URL, cache settings)
- `SubdirSelection`: Strategy for selecting which subdirectories to query
- `RepoDataRecord`: A package record with associated channel/subdir metadata

## Integration with Rattler Workspace

This crate depends on several other Rattler crates:
- `rattler_conda_types`: Core types (PackageRecord, MatchSpec, Channel, etc.)
- `rattler_networking`: HTTP client with authentication and middleware
- `rattler_digest`: Hash computation
- `rattler_cache`: Package caching infrastructure
- `rattler_package_streaming`: Extract packages for run_exports

When making changes, be aware of these dependencies and their versioning in the workspace.

## Platform-Specific Code

Uses conditional compilation for platform-specific features:
- `#[cfg(not(target_arch = "wasm32"))]`: Native-only code (file locking, run_exports extraction)
- `#[cfg(target_arch = "wasm32")]`: WASM-specific implementations
- Separate `tokio/` and `wasm/` submodules in `remote_subdir/` and `sharded_subdir/`

## Testing

Tests use:
- `rstest` for parameterized tests
- `insta` for snapshot testing
- `assert_matches` for pattern matching assertions
- `axum` and `tower-http` for mock HTTP servers in integration tests
