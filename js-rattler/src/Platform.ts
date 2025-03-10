import { Platform, platformNames, Arch, archNames } from "../pkg";
export { Platform, platformNames, Arch, archNames } from "../pkg";

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
