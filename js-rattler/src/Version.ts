import { JsVersion } from "../pkg";

export class Version {
    native: JsVersion;

    /**
     * Constructs a new Version object from a string representation.
     *
     * @param version The string representation of the version.
     */
    constructor(version: string) {
        this.native = new JsVersion(version);
    }

    /**
     * Constructs a new instance from a rust native object.
     *
     * @private
     */
    private static fromNative(version: JsVersion): Version {
        const result: Version = Object.create(Version.prototype);
        result.native = version;
        return result;
    }

    /**
     * Compares this version with another version. Returns `true` if the
     * versions are considered equal.
     *
     * Note that two version strings can be considered equal even if they are
     * not exactly the same. For example, `1.0` and `1` are considered equal.
     *
     * @param other The version to compare with.
     */
    equals(other: Version): boolean {
        return this.native.equals(other.native);
    }

    /**
     * Returns the string representation of the version.
     *
     * An attempt is made to return the version in the same format as the input
     * string but this is not guaranteed.
     */
    toString(): string {
        return this.native.as_str();
    }

    /** The epoch part of the version. E.g. `1` in `1!2.3`. */
    public get epoch(): number | undefined {
        return this.native.epoch;
    }

    /** `true` if the version has a local part. E.g. `2.3` in `1+2.3`. */
    public get hasLocal(): boolean {
        return this.native.has_local;
    }

    /**
     * `true` if the version is considered a development version.
     *
     * A development version is a version that contains the string `dev` in the
     * version string.
     */
    public get isDev(): boolean {
        return this.native.is_dev;
    }

    /**
     * Returns the major and minor part of the version if the version does not
     * represent a typical major minor version. If any of the parts are not a
     * single number, undefined is returned.
     */
    public asMajorMinor(): [number, number] | undefined {
        const parts = this.native.as_major_minor();
        if (!parts) {
            return undefined;
        }
        return [parts[0], parts[1]];
    }

    /**
     * Returns true if this version starts with the other version. This is
     * defined as the other version being a prefix of this version.
     *
     * @param other The version to check if this version starts with.
     */
    public startsWith(other: Version): boolean {
        return this.native.starts_with(other.native);
    }

    /**
     * Returns true if this version is compatible with the other version.
     *
     * @param other The version to check if this version is compatible with.
     */
    public compatibleWith(other: Version): boolean {
        return this.native.compatible_with(other.native);
    }

    /**
     * Pop the last `n` segments from the version.
     *
     * @param n The number of segments to pop from the version.
     */
    public popSegments(n: number): Version | undefined {
        let native = this.native.pop_segments(n);
        if (!native) {
            return undefined;
        }
        return Version.fromNative(native);
    }

    /**
     * Extend the version to the given length by adding zeros. If the version is
     * already at the specified length or longer the original version will be
     * returned.
     *
     * @param length The length to extend the version to.
     */
    public extendToLength(length: number): Version {
        return Version.fromNative(this.native.extend_to_length(length));
    }

    /**
     * Returns a new version with the segments from start to end (exclusive).
     *
     * Returns undefined if the start or end index is out of bounds.
     *
     * @param start The start index of the segment.
     * @param end The end index of the segment.
     */
    public withSegments(start: number, end: number): Version | undefined {
        let native = this.native.with_segments(start, end);
        if (!native) {
            return undefined;
        }
        return Version.fromNative(native);
    }

    /** The number of segments in the version. */
    public get length(): number {
        return this.native.length;
    }

    /** Returns the version without the local part. E.g. `1+2.3` becomes `1`. */
    public stripLocal(): Version {
        return Version.fromNative(this.native.strip_local());
    }

    /**
     * Returns a new version where the major segment of this version has been
     * bumped.
     */
    public bumpMajor(): Version {
        return Version.fromNative(this.native.bump_major());
    }

    /**
     * Returns a new version where the minor segment of this version has been
     * bumped.
     */
    public bumpMinor(): Version {
        return Version.fromNative(this.native.bump_minor());
    }

    /**
     * Returns a new version where the patch segment of this version has been
     * bumped.
     */
    public bumpPatch(): Version {
        return Version.fromNative(this.native.bump_patch());
    }

    /**
     * Returns a new version where the last segment of this version has been
     * bumped.
     */
    public bumpLast(): Version {
        return Version.fromNative(this.native.bump_last());
    }

    /**
     * Returns a new version where the given segment of this version has been
     * bumped.
     */
    public bumpSegment(segment: number): Version {
        return Version.fromNative(this.native.bump_segment(segment));
    }

    /**
     * Returns a new version where the last segment is an "alpha" segment (ie.
     * `.0a0`)
     */
    public withAlpha(): Version {
        return Version.fromNative(this.native.with_alpha());
    }

    /**
     * Compare 2 versions.
     *
     * Returns `-1` if self<other, `0` if self == other, `1` if self > other
     */
    public compare(other: Version): number {
        return this.native.compare(other.native);
    }
}
