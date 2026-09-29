#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/publish-placeholder-crate.sh [--dry-run] <crate-name>

Publish an empty 0.0.0 crate to reserve its name before configuring crates.io
Trusted Publishing. Authentication must already be configured with `cargo login`
or the CARGO_REGISTRY_TOKEN environment variable.
EOF
}

dry_run=false
if [[ ${1:-} == "--dry-run" ]]; then
  dry_run=true
  shift
fi

if [[ $# -ne 1 ]]; then
  usage >&2
  exit 2
fi

crate_name=$1
if [[ ! $crate_name =~ ^[A-Za-z][A-Za-z0-9_-]*$ ]]; then
  echo "Invalid crate name: $crate_name" >&2
  echo "Use letters, digits, hyphens, or underscores, starting with a letter." >&2
  exit 2
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required to publish the placeholder crate." >&2
  exit 1
fi

placeholder_dir=$(mktemp -d "${TMPDIR:-/tmp}/rattler-placeholder-crate.XXXXXX")
trap 'rm -rf -- "$placeholder_dir"' EXIT
mkdir -p "$placeholder_dir/src"

cat >"$placeholder_dir/Cargo.toml" <<EOF
[package]
name = "$crate_name"
version = "0.0.0"
edition = "2024"
description = "Placeholder for a crate in the rattler project"
license = "BSD-3-Clause"
repository = "https://github.com/conda/rattler"
publish = ["crates-io"]

[lib]
path = "src/lib.rs"
EOF

cat >"$placeholder_dir/src/lib.rs" <<EOF
//! Placeholder for the \`$crate_name\` crate.
//!
//! A functional release will replace this package.
EOF

cat >"$placeholder_dir/README.md" <<EOF
# $crate_name

This is a placeholder release for a crate in the
[rattler](https://github.com/conda/rattler) project.
EOF

echo "Generated $crate_name 0.0.0 in $placeholder_dir"
echo "The initial publish needs a crates.io token with the publish-new scope."
echo "Create one at: https://crates.io/settings/tokens"
echo "Then authenticate with: cargo login"
echo

publish_args=(publish --manifest-path "$placeholder_dir/Cargo.toml")
if $dry_run; then
  publish_args+=(--dry-run)
fi
cargo "${publish_args[@]}"

if $dry_run; then
  echo
  echo "Dry run complete; $crate_name was not uploaded."
  exit 0
fi

echo
echo "Published: https://crates.io/crates/$crate_name"
echo "Configure Trusted Publishing: https://crates.io/crates/$crate_name/settings"
echo "Documentation: https://crates.io/docs/trusted-publishing"
echo
echo "On the crate settings page:"
echo "  1. Find Trusted Publishing and click Add trusted publisher."
echo "  2. Select GitHub Actions."
echo "  3. Repository owner: conda"
echo "  4. Repository name: rattler"
echo "  5. Workflow filename: release-rust.yaml"
echo "  6. Environment: release"
echo "  7. Save the trusted publisher."
