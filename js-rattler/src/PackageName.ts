import { NonEmptyString } from "./typeUtils";

/**
 * Defines the allowed characters for any package name.
 *
 * Allowed characters:
 *
 * - Lowercase letters (`a-z`)
 * - Uppercase letters (`A-Z`)
 * - Digits (`0-9`)
 * - Underscore (`_`)
 * - Dash (`-`)
 * - Dot (`.`)
 *
 * @public
 */
// prettier-ignore
export type PackageNameChar =
    | "a" | "b" | "c" | "d" | "e" | "f" | "g" | "h" | "i" | "j" | "k" | "l" | "m"
    | "n" | "o" | "p" | "q" | "r" | "s" | "t" | "u" | "v" | "w" | "x" | "y" | "z"
    | "A" | "B" | "C" | "D" | "E" | "F" | "G" | "H" | "I" | "J" | "K" | "L" | "M"
    | "N" | "O" | "P" | "Q" | "R" | "S" | "T" | "U" | "V" | "W" | "X" | "Y" | "Z"
    | "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
    | "_"
    | "-"
    | ".";

/**
 * Checks whether a string consists only of valid package name characters.
 *
 * - If `S` contains only allowed characters, it resolves to `S`.
 * - Otherwise, it resolves to `never`.
 *
 * @example
 *
 * ```ts
 * type Valid = ContainsOnlyPackageNameChars<"valid-name">; // 'valid-name'
 * type Invalid = ContainsOnlyPackageNameChars<"invalid!">; // never
 * ```
 *
 * @public
 */
export type ContainsOnlyPackageNameChars<S extends string> = S extends ""
    ? "" // Empty string is valid
    : S extends `${infer First}${infer Rest}`
      ? First extends PackageNameChar
          ? ContainsOnlyPackageNameChars<Rest> extends never
              ? never
              : S
          : never
      : never;

/**
 * Ensures that a string is a valid package name.
 *
 * - If `S` contains only valid characters and is not empty, it resolves to `S`.
 * - Otherwise, it resolves to `never`.
 *
 * @example
 *
 * ```ts
 * type Valid = PackageNameLiteral<"valid-name">; // 'valid-name'
 * type Invalid = PackageNameLiteral<"invalid!">; // never
 * type Empty = PackageNameLiteral<"">; // never
 * ```
 *
 * @public
 */
export type PackageNameLiteral<S extends string> =
    ContainsOnlyPackageNameChars<S> & NonEmptyString<S>;

/** A unique symbol used for branding `PackageName` types. */
declare const PACKAGE_NAME_BRAND: unique symbol;

/**
 * A **branded type** representing a validated package name.
 *
 * - This type is **enforced at runtime** using `isPackageName()`.
 * - Ensures that a package name conforms to the expected format.
 *
 * @example
 *
 * ```ts
 * const pkg: PackageName = "valid-package" as PackageName;
 * ```
 *
 * @public
 */
export type PackageName = string & { [PACKAGE_NAME_BRAND]: void };

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
 * - A **PackageName** (a runtime-validated string).
 * - A **string literal** that satisfies `PackageNameLiteral<S>`.
 *
 * This is useful for functions that accept both validated runtime values and
 * compile-time checked literals.
 *
 * @example
 *
 * ```ts
 * function processPackage(name: PackageNameOrLiteral) { ... }
 *
 * processPackage("valid-package"); // ✅ Allowed (checked at compile-time)
 * processPackage("invalid!"); // ❌ Compile-time error
 * ```
 *
 * @param S - The input string type.
 * @public
 */
export type PackageNameOrLiteral<S extends string = string> =
    | PackageName
    | (S extends PackageNameLiteral<S> ? S : never);

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
