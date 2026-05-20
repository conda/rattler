---
name: update-skills
description: Update the skills in this repository. Use when updating the py-rattler and the rattler-bin skill.
---

# py-rattler Skill Update Instructions

py-rattler skill with API documentation from `py-rattler/docs`.

## Source of Truth

The Python source files live at `py-rattler/rattler/` in the rattler repository. The Rust FFI types are defined in `py-rattler/src/` but the Python wrappers in `py-rattler/rattler/` are the authoritative API surface.

When updating, compute the diff between the last py-rattler version and the currently checked out version and update the skill accordingly.

## Structure

- `SKILL.md` - Hub file with navigation table, quick start, and import reference
- `references/solving-installing.md` - `solve()`, `solve_with_sparse_repodata()`, `install()`
- `references/match-spec.md` - `MatchSpec`, `NamelessMatchSpec`, `PackageNameMatcher`
- `references/version.md` - `Version`, `VersionSpec`
- `references/channels-platforms.md` - `Channel`, `ChannelConfig`, `ChannelPriority`, `Platform`
- `references/virtual-packages.md` - `VirtualPackage`, `GenericVirtualPackage`, `VirtualPackageOverrides`, `Override`
- `references/networking.md` - `Client`, all middleware classes, `fetch_repo_data()`
- `references/gateway-repodata.md` - `Gateway`, `SourceConfig`, `RepoDataSource`, `RepoData`, `SparseRepoData`
- `references/package-records.md` - `PackageRecord`, `RepoDataRecord`, `NoArchType`
- `references/package-metadata.md` - `PackageName`, `IndexJson`, `AboutJson`, `RunExportsJson`, `PathsJson`, related types
- `references/lock-files.md` - `LockFile`, `Environment`, `LockChannel`, locked package types, `PackageHashes`
- `references/shell-activation.md` - `activate()`, `Shell`, `ActivationVariables`, `ActivationResult`
- `references/package-streaming.md` - `extract`, `download_*` functions
- `references/indexing.md` - `index_fs()`, `index_s3()`, `S3Credentials`
- `references/prefix-records.md` - `PrefixRecord`, `PrefixPaths`, `PrefixPathsEntry`, link types
- `references/pty.md` - `PtyProcess`, `PtySession`, `PtyProcessOptions`
- `references/exceptions.md` - All exception classes

## Content Guidelines

- **Every public class, method, and property** should be documented. When new API is added, add it to the appropriate reference file.
- **Include full function signatures** with all parameters, types, and defaults. Users of this skill are AI coding agents that need exact signatures.
- **Use tables** for parameter lists and property lists — they are compact and scannable.
- **Include code examples** for non-obvious usage patterns but keep them minimal. One example per concept is enough.
- **Keep SKILL.md as a hub** — it should have the navigation table, a quick start example, and the import reference. Detailed API docs go in reference files.

## What to Watch for When Updating

- New modules or classes added to `rattler/__init__.py` exports
- New parameters on `solve()`, `install()`, or `Gateway`
- New middleware classes in `rattler/networking/`
- Changes to the `RepoDataSource` protocol
- New lock file package types (e.g., new ecosystems beyond conda and PyPI)
- New `PackageFormatSelection` enum values
- Breaking changes to constructor signatures or property types
- Deprecated APIs

## What NOT to Include

- Internal/private APIs (methods prefixed with `_`)
- Implementation details of the Rust FFI layer
- The Rust source code or PyO3 bindings
- Detailed algorithmic explanations of the solver internals

# rattler CLI Skill Update Instructions

Skill for the `rattler` CLI (the `rattler-bin` crate).

## Source of Truth

Source lives at `crates/rattler-bin/` in the conda/rattler repository. Subcommands are defined in `crates/rattler-bin/src/commands/*.rs`. The CLI is also documented through `rattler --help` and `rattler <subcommand> --help`.

When updating, check out the rattler repo and read the `src/commands/*.rs` sources directly. The Rust source is the authoritative API surface — help text is derived from clap doc comments.

## Structure

- `SKILL.md` - Hub file with a command index, install note, and navigation table
- `references/environments.md` - `create`, `run`, `list`, `shell-hook`, `install-menu`, `remove-menu`
- `references/packages.md` - `search`, `inspect`, `fetch-file`, `download`, `extract`, `link`
- `references/upload.md` - `upload` and all subcommands (prefix, anaconda, quetz, artifactory, cloudsmith, s3)
- `references/auth.md` - `auth login` / `auth logout`
- `references/misc.md` - `virtual-packages`, `completion`

## Content Guidelines

- Document every subcommand's purpose, arguments, and options.
- Use tables for option lists — they're compact and scannable.
- Include short example invocations (one per subcommand is usually enough).
- Call out defaults (target prefix `.prefix`, default channel `conda-forge`, etc.).
- Keep SKILL.md as a hub — detailed command docs go in reference files.

## What to Watch for When Updating

- New subcommands added to `Command` in `src/main.rs`
- New options on existing subcommands (check `Opt` struct fields)
- New upload targets under `rattler_upload`
- Changes to default values (default channel, default prefix)
- New auth storage / OAuth flow options

## What NOT to Include

- Internal Rust types, `miette` error types, or progress-bar plumbing
- Implementation details of the solver, repodata gateway, or package cache
