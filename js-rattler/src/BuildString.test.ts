import { describe, expect, it } from "@jest/globals";
import { BuildString, PackageRecord } from "./index";

function record() {
    return new PackageRecord({
        name: "foo",
        version: "1.0",
        build: "0",
        build_number: 0,
        subdir: "noarch",
    });
}

const invalidBuilds = ["", "py3-none-any", "a b", "é", "a".repeat(65)];

describe("BuildString", () => {
    it.each(["0", "py313_h123_0", "a.Z+1", "a".repeat(64)])(
        "accepts valid build %j as a string or object",
        (value) => {
            const build = new BuildString(value);
            expect(String(build)).toBe(value);
            const pkg = record();
            pkg.build = value;
            expect(pkg.build).toBe(value);
            pkg.build = build;
            expect(pkg.build).toBe(value);
            expect(build.toString()).toBe(value);
        },
    );

    it.each(invalidBuilds)("rejects invalid build %j", (value) => {
        expect(() => new BuildString(value)).toThrow(
            expect.objectContaining({ code: "PARSE_BUILD_STRING" }),
        );
        const pkg = record();
        expect(() => {
            pkg.build = value;
        }).toThrow(expect.objectContaining({ code: "PARSE_BUILD_STRING" }));
        expect(pkg.build).toBe("0");
    });

    it.each(invalidBuilds)("preserves unchecked build %j", (value) => {
        const build = BuildString.newUnchecked(value);
        const pkg = record();
        pkg.build = build;
        expect(pkg.build).toBe(value);
        // Assignment must not consume the object.
        pkg.build = build;
        expect(build.toString()).toBe(value);
        const json = pkg.toJson();
        expect(json.build).toBe(value);
        expect(new PackageRecord(json).build).toBe(value);
    });

    it("rejects other JS values without changing the record", () => {
        const pkg = record();
        for (const value of [null, undefined, 123, { toString: () => "" }]) {
            expect(() => {
                // @ts-expect-error Only strings and BuildString objects are accepted.
                pkg.build = value;
            }).toThrow(TypeError);
            expect(pkg.build).toBe("0");
        }
    });
});
