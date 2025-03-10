/**
 * All platform names supported by this library.
 *
 * @public
 */
export declare const platformNames: readonly [
    "noarch",
    "linux-32",
    "linux-64",
    "linux-aarch64",
    "linux-armv6l",
    "linux-armv7l",
    "linux-ppc64le",
    "linux-ppc64",
    "linux-ppc",
    "linux-s390x",
    "linux-riscv32",
    "linux-riscv64",
    "osx-64",
    "osx-arm64",
    "win-32",
    "win-64",
    "win-arm64",
    "emscripten-wasm32",
    "wasi-wasm32",
    "zos-z",
];

/**
 * A type that represents a valid platform.
 *
 * @public
 */
export declare type Platform = (typeof platformNames)[number];

/**
 * A type that represents a valid architecture.
 *
 * @public
 */
export declare type Arch = (typeof archNames)[number];

/**
 * All architecture names supported by this library.
 *
 * @public
 */
export declare const archNames: readonly [
    "x86",
    "x86_64",
    "aarch64",
    "arm64",
    "armv6l",
    "armv7l",
    "ppc64le",
    "ppc64",
    "ppc",
    "s390x",
    "riscv32",
    "riscv64",
    "wasm32",
    "z",
];
