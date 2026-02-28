import { describe, expect, it } from "@jest/globals";
import { MatchSpec } from "./MatchSpec";
import { PackageRecord } from "./PackageRecord";

function makeRecord(
    name: string,
    version: string,
    build = "py_0",
    build_number = 0,
) {
    return new PackageRecord({
        name,
        version,
        build,
        build_number,
        depends: [],
        subdir: "linux-64",
    });
}

describe("MatchSpec", () => {
    describe("constructor", () => {
        it("should parse a simple name-only spec", () => {
            expect(() => new MatchSpec("foo")).not.toThrow();
        });

        it("should parse a spec with version", () => {
            expect(() => new MatchSpec("foo >=1.2.3")).not.toThrow();
            expect(() => new MatchSpec("foo==1.0")).not.toThrow();
        });

        it("should parse a spec with channel", () => {
            expect(() => new MatchSpec("conda-forge::foo >=1.0")).not.toThrow();
        });

        it("should parse a spec with bracket syntax", () => {
            expect(
                () => new MatchSpec('foo[version=">=1.0",subdir="linux-64"]'),
            ).not.toThrow();
        });

        it("should throw on completely invalid input in strict mode", () => {
            expect(() => new MatchSpec("", "strict")).toThrow();
        });
    });

    describe("toString", () => {
        it("should return the canonical string representation", () => {
            expect(new MatchSpec("foo").toString()).toBe("foo");
            expect(new MatchSpec("foo >=1.2.3").toString()).toBe("foo >=1.2.3");
        });

        it("should canonicalize a spec with channel and version", () => {
            const spec = new MatchSpec("conda-forge::foo >=1.0");
            expect(spec.toString()).toBe("conda-forge::foo >=1.0");
        });
    });

    describe("name", () => {
        it("should return the package name", () => {
            expect(new MatchSpec("foo").name).toBe("foo");
            expect(new MatchSpec("bar >=2.0").name).toBe("bar");
        });

        it("should return a wildcard for a wildcard name", () => {
            expect(new MatchSpec("*").name).toBe("*");
        });
    });

    describe("version", () => {
        it("should return undefined when no version is specified", () => {
            expect(new MatchSpec("foo").version).toBeUndefined();
        });

        it("should return a VersionSpec when a version is specified", () => {
            const spec = new MatchSpec("foo >=1.2.3");
            expect(spec.version).toBeDefined();
            expect(spec.version!.toString()).toBe(">=1.2.3");
        });

        it("should handle exact version specs", () => {
            const spec = new MatchSpec("foo ==1.0");
            expect(spec.version).toBeDefined();
            expect(spec.version!.toString()).toBe("==1.0");
        });

        it("should handle wildcard version specs", () => {
            const spec = new MatchSpec("foo 1.2.*");
            expect(spec.version).toBeDefined();
        });
    });

    describe("build", () => {
        it("should return undefined when no build string is specified", () => {
            expect(new MatchSpec("foo").build).toBeUndefined();
        });

        it("should return the build string when specified", () => {
            const spec = new MatchSpec("foo 1.0 py37_0");
            expect(spec.build).toBe("py37_0");
        });

        it("should return a glob build string", () => {
            const spec = new MatchSpec('foo[build="py3*"]');
            expect(spec.build).toBe("py3*");
        });
    });

    describe("channel", () => {
        it("should return undefined when no channel is specified", () => {
            expect(new MatchSpec("foo").channel).toBeUndefined();
        });

        it("should return the channel name when specified", () => {
            const spec = new MatchSpec("conda-forge::foo");
            expect(spec.channel).toBe("conda-forge");
        });
    });

    describe("subdir", () => {
        it("should return undefined when no subdir is specified", () => {
            expect(new MatchSpec("foo").subdir).toBeUndefined();
        });

        it("should return the subdir when specified via bracket syntax", () => {
            const spec = new MatchSpec('foo[subdir="linux-64"]');
            expect(spec.subdir).toBe("linux-64");
        });

        it("should return the subdir when specified via channel/subdir syntax", () => {
            const spec = new MatchSpec("conda-forge/linux-64::foo");
            expect(spec.subdir).toBe("linux-64");
        });
    });

    describe("namespace", () => {
        it("should return undefined when no namespace is specified", () => {
            expect(new MatchSpec("foo").namespace).toBeUndefined();
        });
    });

    describe("buildNumber", () => {
        it("should return undefined when no build number is specified", () => {
            expect(new MatchSpec("foo").buildNumber).toBeUndefined();
        });

        it("should return the build number spec when specified", () => {
            const spec = new MatchSpec('foo[build_number=">=3"]');
            expect(spec.buildNumber).toBeDefined();
        });
    });

    describe("license", () => {
        it("should return undefined when no license is specified", () => {
            expect(new MatchSpec("foo").license).toBeUndefined();
        });

        it("should return the license when specified", () => {
            const spec = new MatchSpec('foo[license="MIT"]');
            expect(spec.license).toBe("MIT");
        });
    });

    describe("url", () => {
        it("should return undefined when no url is specified", () => {
            expect(new MatchSpec("foo").url).toBeUndefined();
        });
    });

    describe("md5", () => {
        it("should return undefined when no md5 is specified", () => {
            expect(new MatchSpec("foo").md5).toBeUndefined();
        });

        it("should return the md5 hex string when specified", () => {
            const spec = new MatchSpec(
                "foo[md5=dede6252c964db3f3e41c7d30d07f6bf]",
            );
            expect(spec.md5).toBe("dede6252c964db3f3e41c7d30d07f6bf");
        });
    });

    describe("sha256", () => {
        it("should return undefined when no sha256 is specified", () => {
            expect(new MatchSpec("foo").sha256).toBeUndefined();
        });

        it("should return the sha256 hex string when specified", () => {
            const spec = new MatchSpec(
                "foo[sha256=01ba4719c80b6fe911b091a7c05124b64eeece964e09c058ef8f9805daca546b]",
            );
            expect(spec.sha256).toBe(
                "01ba4719c80b6fe911b091a7c05124b64eeece964e09c058ef8f9805daca546b",
            );
        });
    });

    describe("matches", () => {
        it("should match a record with the same name", () => {
            const spec = new MatchSpec("foo");
            const record = makeRecord("foo", "1.0.0");
            expect(spec.matches(record)).toBe(true);
        });

        it("should not match a record with a different name", () => {
            const spec = new MatchSpec("foo");
            const record = makeRecord("bar", "1.0.0");
            expect(spec.matches(record)).toBe(false);
        });

        it("should match a record satisfying the version constraint", () => {
            const spec = new MatchSpec("foo >=1.2.3");
            expect(spec.matches(makeRecord("foo", "1.2.3"))).toBe(true);
            expect(spec.matches(makeRecord("foo", "2.0.0"))).toBe(true);
            expect(spec.matches(makeRecord("foo", "1.2.2"))).toBe(false);
        });

        it("should match a wildcard name spec against any package", () => {
            const spec = new MatchSpec("*");
            expect(spec.matches(makeRecord("anything", "99.0"))).toBe(true);
        });

        it("should match when build string matches", () => {
            const spec = new MatchSpec("foo 1.0 py37_0");
            expect(spec.matches(makeRecord("foo", "1.0", "py37_0"))).toBe(true);
            expect(spec.matches(makeRecord("foo", "1.0", "py38_0"))).toBe(
                false,
            );
        });

        it("should match a glob build string", () => {
            const spec = new MatchSpec('foo[build="py3*"]');
            expect(spec.matches(makeRecord("foo", "1.0", "py37_0"))).toBe(true);
            expect(spec.matches(makeRecord("foo", "1.0", "py38_0"))).toBe(true);
            expect(spec.matches(makeRecord("foo", "1.0", "py27_0"))).toBe(
                false,
            );
        });
    });
});
