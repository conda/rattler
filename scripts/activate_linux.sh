# Ignore requiring a shebang as this is a script meant to be sourced
# shellcheck disable=SC2148

# Setup the mold linker when targeting x86_64-unknown-linux-gnu
set -Eeuo pipefail
export CARGO_TARGET_DIR=".pixi/target"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="clang"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=$CONDA_PREFIX/bin/mold"
