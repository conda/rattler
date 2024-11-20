import { JsVersion } from "../pkg";

export class Version {
    private version: JsVersion;

    /**
     * Constructs a new Version object from a string representation.
     * @param version The string representation of the version.
     */
    constructor(version: string) {
        this.version = new JsVersion(version);
    }

    toString(): string {
        return this.version.as_str();
    }
}
