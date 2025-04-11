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
export declare type PackageName = string & {
    [PACKAGE_NAME_BRAND]: void;
};

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
export declare type PackageNameChar =
    | "a"
    | "b"
    | "c"
    | "d"
    | "e"
    | "f"
    | "g"
    | "h"
    | "i"
    | "j"
    | "k"
    | "l"
    | "m"
    | "n"
    | "o"
    | "p"
    | "q"
    | "r"
    | "s"
    | "t"
    | "u"
    | "v"
    | "w"
    | "x"
    | "y"
    | "z"
    | "A"
    | "B"
    | "C"
    | "D"
    | "E"
    | "F"
    | "G"
    | "H"
    | "I"
    | "J"
    | "K"
    | "L"
    | "M"
    | "N"
    | "O"
    | "P"
    | "Q"
    | "R"
    | "S"
    | "T"
    | "U"
    | "V"
    | "W"
    | "X"
    | "Y"
    | "Z"
    | "0"
    | "1"
    | "2"
    | "3"
    | "4"
    | "5"
    | "6"
    | "7"
    | "8"
    | "9"
    | "_"
    | "-"
    | ".";

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
export declare type PackageNameLiteral<S extends string> =
    ContainsOnlyPackageNameChars<S> & NonEmptyString<S>;

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
export declare type PackageNameOrLiteral<S extends string = string> =
    | PackageName
    | (S extends PackageNameLiteral<S> ? S : never);

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
export declare type ContainsOnlyPackageNameChars<S extends string> =
    S extends ""
        ? ""
        : S extends `${infer First}${infer Rest}`
          ? First extends PackageNameChar
              ? ContainsOnlyPackageNameChars<Rest> extends never
                  ? never
                  : S
              : never
          : never;

/**
 * Ensures that a string is non-empty.
 *
 * - If `T` is an empty string, it resolves to `never`.
 * - Otherwise, it resolves to `T`.
 *
 * @example
 *
 * ```ts
 * type Valid = NonEmptyString<"hello">; // 'hello'
 * type Invalid = NonEmptyString<"">; // never
 * ```
 *
 * @public
 */
export declare type NonEmptyString<T extends string> = T extends "" ? never : T;
