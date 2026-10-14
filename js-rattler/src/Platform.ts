/**
 * All platform names supported by this library.
 *
 * @public
 */
export const platformNames = [
    "noarch",
    "linux-32",
    "linux-64",
    "linux-aarch64",
    "linux-armv6l",
    "linux-armv7l",
    "linux-loongarch64",
    "linux-ppc64le",
    "linux-ppc64",
    "linux-ppc",
    "linux-s390x",
    "linux-riscv32",
    "linux-riscv64",
    "freebsd-32",
    "freebsd-64",
    "freebsd-arm64",
    "freebsd-ppc64le",
    "freebsd-ppc64",
    "osx-64",
    "osx-arm64",
    "ios-arm64",
    "iossimulator-arm64",
    "iossimulator-64",
    "android-aarch64",
    "android-armv7a",
    "android-64",
    "android-32",
    "win-32",
    "win-64",
    "win-arm64",
    "emscripten-wasm32",
    "emscripten-wasm64",
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
    "armv7a",
    "loongarch64",
    "ppc64le",
    "ppc64",
    "ppc",
    "s390x",
    "riscv32",
    "riscv64",
    "wasm32",
    "wasm64",
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
export function isArch(maybeArch: unknown): maybeArch is Arch {
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
        case "linux-loongarch64":
            return "loongarch64";
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
        case "freebsd-32":
            return "x86";
        case "freebsd-64":
            return "x86_64";
        case "freebsd-arm64":
            return "arm64";
        case "freebsd-ppc64le":
            return "ppc64le";
        case "freebsd-ppc64":
            return "ppc64";
        case "osx-64":
            return "x86_64";
        case "osx-arm64":
            return "arm64";
        case "ios-arm64":
            return "arm64";
        case "iossimulator-arm64":
            return "arm64";
        case "iossimulator-64":
            return "x86_64";
        case "android-aarch64":
            return "aarch64";
        case "android-armv7a":
            return "armv7a";
        case "android-64":
            return "x86_64";
        case "android-32":
            return "x86";
        case "win-32":
            return "x86";
        case "win-64":
            return "x86_64";
        case "win-arm64":
            return "arm64";
        case "emscripten-wasm32":
            return "wasm32";
        case "emscripten-wasm64":
            return "wasm64";
        case "wasi-wasm32":
            return "wasm32";
        case "zos-z":
            return "z";
    }
}
