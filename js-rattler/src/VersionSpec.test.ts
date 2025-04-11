import { describe, expect, it } from "@jest/globals";
import { Version } from "./Version";
import { VersionSpec } from "./VersionSpec";

describe("VersionSpec", () => {
    describe("constructor", () => {
        it("should parse a spec from string", () => {
            expect(new VersionSpec(">=1.2.3").toString()).toBe(">=1.2.3");
            expect(new VersionSpec(">=1.2.3,<3").toString()).toBe(">=1.2.3,<3");
        });
    });
    describe("toString", () => {
        it("should return the string representation of the version", () => {
            expect(new VersionSpec(">=1.2.3").toString()).toBe(">=1.2.3");
        });
    });
    describe("matches", () => {
        it("should return true if the version matches the spec", () => {
            expect(
                new VersionSpec(">=1.2.3").matches(new Version("1.2.3")),
            ).toBe(true);
            expect(
                new VersionSpec(">=1.2.3").matches(new Version("1.2.2")),
            ).toBe(false);
            expect(
                new VersionSpec(">=1.2.3,<3").matches(new Version("1.2.3")),
            ).toBe(true);
            expect(
                new VersionSpec(">=1.2.3,<3").matches(new Version("3.0.0")),
            ).toBe(false);
        });
    });
});
