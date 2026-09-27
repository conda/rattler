/**
 * The stable error codes carried by errors thrown by the bindings.
 *
 * @public
 */
export type RattlerErrorCode =
    | "PARSE_VERSION"
    | "VERSION_EXTEND"
    | "VERSION_BUMP"
    | "PARSE_VERSION_SPEC"
    | "PARSE_CHANNEL"
    | "PARSE_PLATFORM"
    | "PARSE_MATCH_SPEC"
    | "PARSE_PACKAGE_NAME"
    | "PARSE_MD5"
    | "PARSE_SHA256"
    | "SUBDIR_NOT_FOUND"
    | "FETCH"
    | "GATEWAY"
    | "SOLVE"
    | "SERDE";

/**
 * An error thrown by the bindings. The `code` identifies the kind of failure
 * without having to match on the message.
 *
 * @public
 */
export interface RattlerError extends Error {
    /** A stable code identifying the kind of failure. */
    code: RattlerErrorCode;
}

/**
 * Returns `true` if the given value is an error thrown by the bindings.
 *
 * @public
 */
export function isRattlerError(value: unknown): value is RattlerError {
    return (
        value instanceof Error &&
        typeof (value as { code?: unknown }).code === "string"
    );
}
