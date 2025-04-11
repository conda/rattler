import { describe, expect, it } from "@jest/globals";
import { Version } from "./Version";

// Comment to trigger test run

describe("Version", () => {
    describe("constructor", () => {
        it("should parse a version from string", () => {
            expect(new Version("1.2.3").toString()).toBe("1.2.3");
            expect(new Version("2!2024.1alpha3+4.5.6").toString()).toBe(
                "2!2024.1alpha3+4.5.6",
            );
        });
    });
    describe("toString", () => {
        it("should return the string representation of the version", () => {
            expect(new Version("1.2.3").toString()).toBe("1.2.3");
            expect(new Version("2!2024.1alpha3+4.5.6").toString()).toBe(
                "2!2024.1alpha3+4.5.6",
            );
        });
        it("should retain separators", () => {
            expect(new Version("1.2.3").toString()).toBe("1.2.3");
            expect(new Version("1-2-3").toString()).toBe("1-2-3");
            expect(new Version("1_2_3").toString()).toBe("1_2_3");
        });
    });
    describe("epoch", () => {
        it("should return the epoch part of the version", () => {
            expect(new Version("1.2.3").epoch).toBeUndefined();
        });
        it("should return undefined if the version does not have an epoch part", () => {
            expect(new Version("2!2024.1alpha3+4.5.6").epoch).toBe(2);
        });
    });
    describe("hasLocal", () => {
        it("should return true if the version has a local part", () => {
            expect(new Version("1.2.3").hasLocal).toBeFalsy();
        });
        it("should return false if the version does not have a local part", () => {
            expect(new Version("2!2024.1alpha3+4.5.6").hasLocal).toBeTruthy();
        });
    });
    describe("isDev", () => {
        it("should return true if the version is considered a development version", () => {
            expect(new Version("1.2.3").isDev).toBeFalsy();
        });
        it("should return false if the version is not considered a development version", () => {
            expect(new Version("2.0dev").isDev).toBeTruthy();
        });
    });
    describe("asMajorMinor", () => {
        it("should return the major and minor part of the version if they are present", () => {
            expect(new Version("1.2.3").asMajorMinor()).toEqual([1, 2]);
            expect(new Version("2!2024.1+4.5.6").asMajorMinor()).toEqual([
                2024, 1,
            ]);
        });
        it("should return undefined if the major and minor part are not present", () => {
            expect(new Version("1").asMajorMinor()).toBeUndefined();
        });
        it("should return undefined if the major and minor part are not numbers", () => {
            expect(new Version("1a.2").asMajorMinor()).toBeUndefined();
            expect(new Version("1.2b").asMajorMinor()).toBeUndefined();
        });
    });
    describe("startsWith", () => {
        it("should return true if this version starts with the other version", () => {
            expect(
                new Version("1.2.3").startsWith(new Version("1.2")),
            ).toBeTruthy();
            expect(
                new Version("1.2.3").startsWith(new Version("1.2.3")),
            ).toBeTruthy();
            expect(
                new Version("1.2.3").startsWith(new Version("1")),
            ).toBeTruthy();
        });
        it("should return false if this version does not start with the other version", () => {
            expect(
                new Version("1.2.3").startsWith(new Version("1.3")),
            ).toBeFalsy();
            expect(
                new Version("1.2.3").startsWith(new Version("1.2.4")),
            ).toBeFalsy();
            expect(
                new Version("1.2.3").startsWith(new Version("2")),
            ).toBeFalsy();
            expect(
                new Version("1.2.3").startsWith(new Version("1!1.2.3")),
            ).toBeFalsy();
        });
    });
    describe("compatibleWith", () => {
        it("should return true if this version is compatible with the other version", () => {
            expect(
                new Version("1.2.3").compatibleWith(new Version("1.2")),
            ).toBeTruthy();
            expect(
                new Version("1.2.3").compatibleWith(new Version("1.2.3")),
            ).toBeTruthy();
            expect(
                new Version("1.2.3").compatibleWith(new Version("1")),
            ).toBeTruthy();
        });
        it("should return false if this version is not compatible with the other version", () => {
            expect(
                new Version("1.2.3").compatibleWith(new Version("1.3")),
            ).toBeFalsy();
            expect(
                new Version("1.2.3").compatibleWith(new Version("1.2.4")),
            ).toBeFalsy();
            expect(
                new Version("1.2.3").compatibleWith(new Version("2")),
            ).toBeFalsy();
            expect(
                new Version("1.2.3").compatibleWith(new Version("1!1.2.3")),
            ).toBeFalsy();
        });
    });
    describe("equals", () => {
        it("should return true if the versions are equal", () => {
            expect(
                new Version("1.2.3").equals(new Version("1.2.3")),
            ).toBeTruthy();
            expect(new Version("1").equals(new Version("1.0"))).toBeTruthy();
            expect(new Version("1").equals(new Version("1-0"))).toBeTruthy();
        });
        it("should return false if the versions are not equal", () => {
            expect(
                new Version("1.2.3").equals(new Version("1.2.4")),
            ).toBeFalsy();
        });
    });
    describe("popSegments", () => {
        it("should pop the last n segments from the version", () => {
            expect(new Version("1.2.3").popSegments(1)?.toString()).toBe("1.2");
            expect(new Version("1.2.3").popSegments(2)?.toString()).toBe("1");
        });
        it("should return undefined if the version has less or equal than n segments", () => {
            expect(new Version("1.2.3").popSegments(3)).toBeUndefined();
            expect(new Version("1.2.3").popSegments(4)).toBeUndefined();
        });
    });
    describe("extendToLength", () => {
        it("should extend the version to the specified length", () => {
            expect(new Version("1.2.3").extendToLength(4).toString()).toBe(
                "1.2.3.0",
            );
            expect(new Version("1.2").extendToLength(5).toString()).toBe(
                "1.2.0.0.0",
            );
            expect(new Version("1").extendToLength(3).toString()).toBe("1.0.0");
            expect(new Version("1-beta").extendToLength(3).toString()).toBe(
                "1-beta.0",
            );
        });
        it("should return the original version if the version is already at the specified length", () => {
            expect(new Version("1.2.3").extendToLength(3).toString()).toBe(
                "1.2.3",
            );
            expect(new Version("1.2.3").extendToLength(1).toString()).toBe(
                "1.2.3",
            );
        });
    });
    describe("withSegments", () => {
        it("should return a new version with the segments from start to end", () => {
            expect(new Version("1.2.3").withSegments(0, 2)?.toString()).toBe(
                "1.2",
            );
            expect(new Version("1.2.3").withSegments(1, 3)?.toString()).toBe(
                "2.3",
            );
        });
        it("should return undefined if the start or end index is out of bounds", () => {
            expect(new Version("1.2.3").withSegments(0, 4)).toBeUndefined();
            expect(new Version("1.2.3").withSegments(4, 5)).toBeUndefined();
        });
        it("should return undefined if the length of the resulting version is 0", () => {
            expect(new Version("1.2.3").withSegments(0, 0)).toBeUndefined();
        });
    });
    describe("length", () => {
        it("should return the number of segments in the version", () => {
            expect(new Version("1.2.3").length).toBe(3);
            expect(new Version("1.2").length).toBe(2);
            expect(new Version("1").length).toBe(1);
            expect(new Version("1!1").length).toBe(1);
            expect(new Version("1!1.alpha2").length).toBe(2);
        });
    });
    describe("stripLocal", () => {
        it("should return the version without the local part", () => {
            expect(new Version("1.2.3").stripLocal().toString()).toBe("1.2.3");
            expect(new Version("1.2.3+local").stripLocal().toString()).toBe(
                "1.2.3",
            );
        });
    });
    describe("bumpMajor", () => {
        it("should return a new version where the major segment has been bumped", () => {
            expect(new Version("1").bumpMajor().toString()).toBe("2");
            expect(new Version("1.2.3").bumpMajor().toString()).toBe("2.0.0");
            expect(new Version("1!1.2.3").bumpMajor().toString()).toBe(
                "1!2.0.0",
            );
        });
    });
    describe("bumpMinor", () => {
        it("should return a new version where the minor segment has been bumped", () => {
            expect(new Version("1").bumpMinor().toString()).toBe("1.1");
            expect(new Version("1.2.3").bumpMinor().toString()).toBe("1.3.0");
            expect(new Version("1!1.2.3").bumpMinor().toString()).toBe(
                "1!1.3.0",
            );
        });
    });
    describe("bumpPatch", () => {
        it("should return a new version where the patch segment has been bumped", () => {
            expect(new Version("1").bumpPatch().toString()).toBe("1.0.1");
            expect(new Version("1.2.3").bumpPatch().toString()).toBe("1.2.4");
            expect(new Version("1!1.2.3").bumpPatch().toString()).toBe(
                "1!1.2.4",
            );
        });
    });
    describe("bumpLast", () => {
        it("should return a new version where the last segment has been bumped", () => {
            expect(new Version("1").bumpLast().toString()).toBe("2");
            expect(new Version("1.2.3").bumpLast().toString()).toBe("1.2.4");
            expect(new Version("1!1.2.3").bumpLast().toString()).toBe(
                "1!1.2.4",
            );
        });
    });
    describe("bumpSegment", () => {
        it("should return a new version where the given segment has been bumped", () => {
            expect(new Version("1").bumpSegment(0).toString()).toBe("2");
            expect(new Version("1.2.3").bumpSegment(1).toString()).toBe(
                "1.3.3",
            );
            expect(new Version("1!1.2.3").bumpSegment(2).toString()).toBe(
                "1!1.2.4",
            );
        });
    });
    describe("withAlpha", () => {
        it("should return a new version with the alpha part", () => {
            expect(new Version("1.2.3").withAlpha().toString()).toBe(
                "1.2.3.0a0",
            );
            expect(new Version("1.2.3a").withAlpha().toString()).toBe("1.2.3a");
        });
    });

    describe("compare", () => {
        it("should compare version with expected order", () => {
            expect(new Version("1.2.3").compare(new Version("1.2.3"))).toBe(0);
            expect(new Version("1.2.0").compare(new Version("1.2.3"))).toBe(-1);
            expect(new Version("1.2.4").compare(new Version("1.2.3"))).toBe(1);
        });
    });
});
