/**
 * Ensures that a given type is `true`. Used for compile-time assertions.
 *
 * @example
 *
 * ```ts
 * type Test = IsTrue<true>; // Passes
 * type TestError = IsTrue<false>; // Type error
 * ```
 *
 * @internal
 */
export type IsTrue<T extends true> = T;

/**
 * Ensures that a given type is `false`. Used for compile-time assertions.
 *
 * @example
 *
 * ```ts
 * type Test = IsFalse<false>; // Passes
 * type TestError = IsFalse<true>; // Type error
 * ```
 *
 * @internal
 */
export type IsFalse<T extends false> = T;

/**
 * Checks if two types `A` and `B` are exactly the same.
 *
 * - If `A` and `B` are identical, resolves to `true`.
 * - Otherwise, resolves to `false`.
 *
 * @example
 *
 * ```ts
 * type Same = IsSame<"foo", "foo">; // true
 * type Different = IsSame<"foo", "bar">; // false
 * ```
 *
 * @internal
 */
export type IsSame<A, B> = A extends B ? (B extends A ? true : false) : false;

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
export type NonEmptyString<T extends string> = T extends "" ? never : T;
