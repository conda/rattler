import { BuildString, PackageRecord as WasmPackageRecord } from "../pkg";

export { NoArchType, PackageRecordJson } from "../pkg";

/**
 * A package record. JSON construction preserves legacy metadata without
 * validation.
 *
 * @public
 */
export class PackageRecord extends WasmPackageRecord {
    /** The build string of the package. */
    override get build(): string {
        return super.build;
    }

    /** Validates strings; accepts BuildString objects without revalidation. */
    override set build(value: string | BuildString) {
        if (value instanceof BuildString) {
            super.setBuildString(value);
        } else if (typeof value === "string") {
            super.build = value;
        } else {
            throw new TypeError("expected a string or BuildString");
        }
    }
}
