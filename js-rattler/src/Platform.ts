/**
 * All platform names supported by this library.
 *
 * @public
 */
export const platformNames = [
    "noarch",
    "unknown",
    "linux-32",
    "linux-64",
    "linux-aarch64",
    "linux-armv6l",
    "linux-armv7l",
    "linux-loong64",
    "linux-ppc64le",
    "linux-ppc64",
    "linux-ppc",
    "linux-s390x",
    "linux-riscv32",
    "linux-riscv64",
    "freebsd-64",
    "osx-64",
    "osx-arm64",
    "win-32",
    "win-64",
    "win-arm64",
    "emscripten-wasm32",
    "wasi-wasm32",
    "zos-z",
] as const;

/**
 * A type that represents a valid platform.
 *
 * @public
 */
export type Platform = (typeof platformNames)[number];

/**
 * A type guard that identifies if an input value is a `Platform`
 *
 * @public
 */
export function isPlatform(maybePlatform: unknown): maybePlatform is Platform {
    return (
        typeof maybePlatform === "string" &&
        platformNames.includes(maybePlatform as Platform)
    );
}

/**
 * All architecture names supported by this library.
 *
 * @public
 */
export const archNames = [
    "x86",
    "x86_64",
    "aarch64",
    "arm64",
    "armv6l",
    "armv7l",
    "loong64",
    "ppc64le",
    "ppc64",
    "ppc",
    "s390x",
    "riscv32",
    "riscv64",
    "wasm32",
    "z",
] as const;

/**
 * A type that represents a valid architecture.
 *
 * @public
 */
export type Arch = (typeof archNames)[number];

/**
 * A type guard that identifies if an input value is an `Arch`
 *
 * @public
 */
export function isArch(maybeArch: unknown): maybeArch is Platform {
    return (
        typeof maybeArch === "string" && archNames.includes(maybeArch as Arch)
    );
}

/**
 * Returns the architecture of a certain platform
 *
 * @param platform - The platform
 * @public
 */
export function platformArch(platform: Platform): Arch | null {
    switch (platform) {
        case "noarch":
            return null;
        case "unknown":
            return null;
        case "linux-32":
            return "x86";
        case "linux-64":
            return "x86_64";
        case "linux-aarch64":
            return "aarch64";
        case "linux-armv6l":
            return "armv6l";
        case "linux-armv7l":
            return "armv7l";
        case "linux-loong64":
            return "loong64";
        case "linux-ppc64le":
            return "ppc64le";
        case "linux-ppc64":
            return "ppc64";
        case "linux-ppc":
            return "ppc";
        case "linux-s390x":
            return "s390x";
        case "linux-riscv32":
            return "riscv32";
        case "linux-riscv64":
            return "riscv64";
        case "freebsd-64":
            return "x86_64";
        case "osx-64":
            return "x86_64";
        case "osx-arm64":
            return "arm64";
        case "win-32":
            return "x86";
        case "win-64":
            return "x86_64";
        case "win-arm64":
            return "arm64";
        case "emscripten-wasm32":
            return "wasm32";
        case "wasi-wasm32":
            return "wasm32";
        case "zos-z":
            return "z";
    }
}
