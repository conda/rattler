import { NonEmptyString } from "./typeUtils";
import {
    PackageName,
    PackageNameLiteral,
    PackageNameChar,
    PackageNameOrLiteral,
    ContainsOnlyPackageNameChars,
} from "../pkg";

export {
    PackageName,
    PackageNameLiteral,
    PackageNameChar,
    PackageNameOrLiteral,
    ContainsOnlyPackageNameChars,
} from "../pkg";

/**
 * A **branded type** representing a **normalized** package name.
 *
 * - A `NormalizedPackageName` is always **lowercase**.
 * - It extends `PackageName`, ensuring that it still follows package name rules.
 * - Can be obtained by calling `normalizePackageName()`.
 *
 * @example
 *
 * ```ts
 * const normalized: NormalizedPackageName =
 *     "valid-package" as NormalizedPackageName;
 * ```
 *
 * @public
 */
export type NormalizedPackageName = Lowercase<PackageName>;

/**
 * A type that accepts:
 *
 * - A **NormalizedPackageName** (a runtime-validated string).
 * - A **string literal** that satisfies `Lowercase<PackageNameLiteral<S>>`.
 *
 * This is useful for functions that accept both validated runtime values and
 * compile-time checked literals.
 *
 * @example
 *
 * ```ts
 * function processNormalizedPackage(name: NormalizedPackageNameOrLiteral) { ... }
 *
 * processNormalizedPackage("valid-package"); // ✅ Allowed (checked at compile-time)
 * processNormalizedPackage("Invalid-Package"); // ❌ Compile-time error
 * ```
 *
 * @param S - The input string type.
 * @public
 */
export type NormalizedPackageNameOrLiteral<S extends string = string> =
    | NormalizedPackageName
    | (S extends Lowercase<PackageNameLiteral<S>> ? S : never);

/**
 * **Normalizes a package name to lowercase.**
 *
 * - If given a **string literal**, it is validated at compile time.
 * - If given a **runtime-validated** `PackageName`, it is accepted directly.
 * - Returns a `NormalizedPackageName` with all characters converted to lowercase.
 *
 * @example
 *
 * ```ts
 * const normalized = normalizePackageName("Valid-Package"); // "valid-package"
 * ```
 *
 * @param name - The package name to normalize.
 * @returns The normalized package name.
 * @public
 */
export function normalizePackageName<T extends string>(
    name: PackageNameOrLiteral<T>,
): NormalizedPackageName {
    return name.toLowerCase() as NormalizedPackageName;
}

/**
 * **Checks if a string is a valid `PackageName`.**
 *
 * - Returns `true` if `input` matches the allowed package name format.
 * - If `true`, TypeScript narrows the type to `PackageName<string>`.
 *
 * @example
 *
 * ```ts
 * if (isPackageName(userInput)) {
 *     const validName: PackageName = userInput;
 * }
 * ```
 *
 * @param input - The string to validate.
 * @returns `true` if valid, otherwise `false`.
 * @public
 */
export function isPackageName(input: string): input is PackageName {
    return /^[A-Za-z0-9_.-]+$/.test(input);
}

/**
 * **Checks if a string is a valid `NormalizedPackageName`.**
 *
 * - A normalized package name must be **lowercase**.
 * - If `true`, TypeScript narrows the type to `NormalizedPackageName<string>`.
 *
 * @example
 *
 * ```ts
 * if (isNormalizedPackageName(userInput)) {
 *     const validNormalizedName: NormalizedPackageName = userInput;
 * }
 * ```
 *
 * @param input - The string to validate.
 * @returns `true` if valid, otherwise `false`.
 * @public
 */
export function isNormalizedPackageName(
    input: string,
): input is NormalizedPackageName {
    return /^[a-z0-9_.-]+$/.test(input);
}

/**
 * Parses a string and returns it as a `PackageName` if it is valid.
 *
 * @param input - The string to parse.
 * @returns The parsed `PackageName`.
 * @throws Will throw an error if the input is not a valid package name.
 */
export function parsePackageName(input: string): PackageName {
    if (!isPackageName(input)) {
        throw new Error(`Invalid package name: ${input}`);
    }
    return input as PackageName;
}
