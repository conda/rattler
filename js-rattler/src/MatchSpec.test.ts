import { describe, expect, it } from "@jest/globals";
import { MatchSpec, PackageRecord, BuildNumberSpec } from "./index";

describe("MatchSpec", () => {
    it("should parse a basic match spec", () => {
        const spec = new MatchSpec("python >=3.10");
        expect(spec.name).toBe("python");
        expect(spec.to_string()).toBe("python >=3.10");
    });

    it("should handle options in constructor", () => {
        const spec = MatchSpec.fromOptions({
            name: "python",
            version: ">=3.10",
            build: "py310*",
        });
        expect(spec.name).toBe("python");
        expect(spec.version?.toString()).toBe(">=3.10");
        expect(spec.build).toBe("py310*");
    });

    it("should allow getting and setting fields", () => {
        const spec = new MatchSpec("python");
        expect(spec.name).toBe("python");

        spec.build = "py310*";
        expect(spec.build).toBe("py310*");

        spec.name = "python-test";
        expect(spec.to_string()).toContain("python-test");

        spec.subdir = "linux-64";
        expect(spec.subdir).toBe("linux-64");

        spec.channel = "conda-forge";
        expect(spec.channel).toBe("conda-forge");
    });

    it("should match PackageRecord", () => {
        const spec = new MatchSpec("python >=3.10");
        const record = new PackageRecord({
            name: "python",
            version: "3.11.0",
            build: "h123",
            build_number: 0,
            subdir: "linux-64",
        });
        expect(spec.matches(record)).toBe(true);

        const recordOld = new PackageRecord({
            name: "python",
            version: "3.9.0",
            build: "h123",
            build_number: 0,
            subdir: "linux-64",
        });
        expect(spec.matches(recordOld)).toBe(false);
    });

    it("should handle buildNumber", () => {
        const spec = new MatchSpec("python");
        const buildNumber = new BuildNumberSpec(">=1");
        spec.buildNumber = buildNumber;
        expect(spec.buildNumber?.toString()).toBe(">=1");
    });

    it("should handle additional fields", () => {
        const spec = new MatchSpec("python");

        spec.fileName = "python-3.10.0.tar.bz2";
        expect(spec.fileName).toBe("python-3.10.0.tar.bz2");

        spec.license = "MIT";
        expect(spec.license).toBe("MIT");

        spec.trackFeatures = ["feature1", "feature2"];
        expect(spec.trackFeatures).toEqual(["feature1", "feature2"]);
    });

    it("should handle hashes", () => {
        const spec = new MatchSpec("python");
        const md5 = "0123456789abcdef0123456789abcdef";
        spec.md5 = md5;
        expect(spec.md5).toBe(md5);

        const sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        spec.sha256 = sha256;
        expect(spec.sha256).toBe(sha256);
    });
});
