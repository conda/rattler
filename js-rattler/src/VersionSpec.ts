import { ParseStrictness, JsVersionSpec } from "./stub";
import { Version } from "./Version";

/**
 * Represents a version specification in the conda ecosystem.
 *
 * @public
 */
export class VersionSpec {
    /** @internal */
    native: JsVersionSpec;

    /**
     * Constructs a new VersionSpec object from a string representation.
     *
     * @param spec - The string representation of the version spec.
     * @param strictness - The strictness used when parsing the version spec.
     */
    constructor(
        spec: string,
        strictness: ParseStrictness = ParseStrictness.Strict,
    ) {
        this.native = new JsVersionSpec(spec, strictness);
    }

    /**
     * Constructs a new instance from a rust native object.
     *
     * @internal
     */
    private static fromNative(version: JsVersionSpec): VersionSpec {
        const result: VersionSpec = Object.create(VersionSpec.prototype);
        result.native = version;
        return result;
    }

    /** Returns the string representation of the version. */
    toString(): string {
        return this.native.as_str();
    }

    /** Returns true if the version matches this version spec. */
    matches(version: Version): boolean {
        return this.native.matches(version.native);
    }
}
