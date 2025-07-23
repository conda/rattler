import { PackageRecord } from "./PackageRecord";
import { describe, expect, it } from "@jest/globals";
import { Version, VersionWithSource } from "../pkg";
import "./areVersionsEqual";
import { parsePackageName } from "./PackageName";

describe("PackageRecord", () => {
    it("creation from json does not error", () => {
        expect(() => {
            return new PackageRecord({
                build: "py36h1af98f8_1",
                build_number: 1,
                depends: [],
                license: "MIT",
                license_family: "MIT",
                md5: "d65ab674acf3b7294ebacaec05fc5b54",
                name: "foo",
                sha256: "1154fceeb5c4ee9bb97d245713ac21eb1910237c724d2b7103747215663273c2",
                size: 414494,
                subdir: "linux-64",
                timestamp: 1605110689658,
                version: "3.0.2",
            });
        }).not.toThrow();
    });
    it("creation from json returns the same json", () => {
        expect(
            new PackageRecord({
                build: "py36h1af98f8_1",
                build_number: 1,
                depends: [],
                license: "MIT",
                license_family: "MIT",
                md5: "d65ab674acf3b7294ebacaec05fc5b54",
                name: "foo",
                sha256: "1154fceeb5c4ee9bb97d245713ac21eb1910237c724d2b7103747215663273c2",
                size: 414494,
                subdir: "linux-64",
                timestamp: 1605110689658,
                version: "3.0.2",
            }).toJson(),
        ).toEqual({
            build: "py36h1af98f8_1",
            build_number: 1,
            depends: [],
            license: "MIT",
            license_family: "MIT",
            md5: "d65ab674acf3b7294ebacaec05fc5b54",
            name: "foo",
            sha256: "1154fceeb5c4ee9bb97d245713ac21eb1910237c724d2b7103747215663273c2",
            size: 414494,
            subdir: "linux-64",
            timestamp: 1605110689658,
            version: "3.0.2",
        });
    });
    it("creation to fail on invalid json", () => {
        expect(() => {
            return new PackageRecord({
                build: "py36h1af98f8_1",
                build_number: 1,
                name: "foo",
                subdir: "linux-64",
                version: "3.0.2",
                sha256: "invalid",
            });
        }).toThrow();
    });
    it("creation from json creates valid record", () => {
        const record = new PackageRecord({
            build: "py36h1af98f8_1",
            build_number: 1,
            depends: ["bar"],
            constrains: ["baz"],
            license: "MIT",
            license_family: "MIT",
            md5: "d65ab674acf3b7294ebacaec05fc5b54",
            name: "foo",
            sha256: "1154fceeb5c4ee9bb97d245713ac21eb1910237c724d2b7103747215663273c2",
            size: 414494,
            subdir: "linux-64",
            timestamp: 1605110689658,
            version: "03.0.2",
        });
        expect(record.name).toBe("foo");
        expect(record.build).toBe("py36h1af98f8_1");
        expect(record.buildNumber).toBe(1);
        expect(record.depends).toEqual(["bar"]);
        expect(record.constrains).toEqual(["baz"]);
        expect(record.license).toEqual("MIT");
        expect(record.licenseFamily).toEqual("MIT");
        expect(record.md5).toEqual("d65ab674acf3b7294ebacaec05fc5b54");
        expect(record.sha256).toEqual(
            "1154fceeb5c4ee9bb97d245713ac21eb1910237c724d2b7103747215663273c2",
        );
        expect(record.size).toEqual(414494);
        expect(record.subdir).toEqual("linux-64");
        expect(record.timestamp).toEqual(new Date(1605110689658));
        expect(record.version.source).toEqual("03.0.2");
        expect(record.version.version).toEqual(new Version("3.0.2"));
    });

    const record = new PackageRecord({
        build: "py36h1af98f8_1",
        build_number: 1,
        name: "foo",
        subdir: "linux-64",
        version: "03.0.2",
    });

    it("name can be modified", () => {
        record.name = parsePackageName("bar");
        expect(record.name).toBe("bar");
    });
    it("buildNumber can be modified", () => {
        record.buildNumber = 113;
        expect(record.buildNumber).toBe(113);
    });
    it("build can be modified", () => {
        record.build = "py36h1af98f8_2";
        expect(record.build).toBe("py36h1af98f8_2");
    });
    it("subdir can be modified", () => {
        record.subdir = "osx-64";
        expect(record.subdir).toBe("osx-64");
    });
    it("version can be modified", () => {
        record.version = new VersionWithSource("3.00.3");
        expect(record.version.source).toBe("3.00.3");
        expect(record.version.version).toEqual(new Version("3.0.3"));
    });
    it("depends can be modified", () => {
        record.depends = ["bar", "baz"];
        expect(record.depends).toEqual(["bar", "baz"]);
    });
    it("constrains can be modified", () => {
        record.constrains = ["bar", "baz"];
        expect(record.constrains).toEqual(["bar", "baz"]);
    });
    it("license can be modified", () => {
        record.license = "BSD-3-Clause";
        expect(record.license).toEqual("BSD-3-Clause");
    });
    it("licenseFamily can be modified", () => {
        record.licenseFamily = "BSD";
        expect(record.licenseFamily).toEqual("BSD");
    });
    it("md5 can be modified", () => {
        record.md5 = "d65ab674acf3b7294ebacaec05fc5b55";
        expect(record.md5).toEqual("d65ab674acf3b7294ebacaec05fc5b55");
    });
    it("only accepts valid md5 hashes", () => {
        expect(() => {
            record.md5 = "invalid";
        }).toThrow("invalid is not a valid hex encoded MD5 hash");
    });
    it("sha256 can be modified", () => {
        record.sha256 =
            "1154fceeb5c4ee9bb97d245713ac21eb1910237c724d2b7103747215663273c3";
        expect(record.sha256).toEqual(
            "1154fceeb5c4ee9bb97d245713ac21eb1910237c724d2b7103747215663273c3",
        );
    });
    it("only accepts valid sha256 hashes", () => {
        expect(() => {
            record.sha256 = "invalid";
        }).toThrow("invalid is not a valid hex encoded SHA256 hash");
    });
    it("size can be modified", () => {
        record.size = 414495;
        expect(record.size).toEqual(414495);
    });
    it("timestamp can be modified", () => {
        record.timestamp = new Date(1605110689659);
        expect(record.timestamp).toEqual(new Date(1605110689659));
    });
    it("legacy_bz2_md5 can be modified", () => {
        record.legacyBz2Md5 = "d65ab674acf3b7294ebacaec05fc5b55";
        expect(record.legacyBz2Md5).toEqual("d65ab674acf3b7294ebacaec05fc5b55");
    });
    it("only accepts valid legacy_bz2_md5 hashes", () => {
        expect(() => {
            record.legacyBz2Md5 = "invalid";
        }).toThrow("invalid is not a valid hex encoded MD5 hash");
    });
    it("legacy_bz2_size can be modified", () => {
        record.legacyBz2Size = 414495;
        expect(record.legacyBz2Size).toEqual(414495);
    });
    it("noarch can be modified", () => {
        record.noarch = "generic";
        expect(record.noarch).toEqual("generic");
    });
    it("pythonSitePackagesPath can be modified", () => {
        record.pythonSitePackagesPath = "foo/bar";
        expect(record.pythonSitePackagesPath).toEqual("foo/bar");
    });
    it("trackFeatures can be modified", () => {
        record.trackFeatures = ["foo"];
        expect(record.trackFeatures).toEqual(["foo"]);
    });
    it("platform can be modified", () => {
        record.platform = "linux";
        expect(record.platform).toEqual("linux");
    });
    it("arch can be modified", () => {
        record.arch = "x86_64";
        expect(record.arch).toEqual("x86_64");
    });
    it("features can be modified", () => {
        record.features = "foo";
        expect(record.features).toEqual("foo");
    });
});
