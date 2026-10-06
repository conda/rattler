import { describe, expect, it } from "@jest/globals";
import { MatchSpec } from "./MatchSpec";
import { PackageRecord } from "./PackageRecord";
import { RepoDataRecord } from "./RepoDataRecord";
import { Version } from "./Version";
import { parsePackageName } from "./PackageName";
import { isRattlerError } from "./RattlerError";

describe("MatchSpec", () => {
    describe("constructor", () => {
        it("parses a spec", () => {
            const spec = new MatchSpec("conda-forge::numpy >=1.20,<2 py312*");
            expect(spec.name).toBe("numpy");
            expect(spec.exactName).toBe("numpy");
            expect(spec.version?.toString()).toBe(">=1.20,<2");
            expect(spec.build).toBe("py312*");
            expect(spec.channel).toBe(
                "https://conda.anaconda.org/conda-forge/",
            );
            expect(spec.toString()).toBe("conda-forge::numpy >=1.20,<2 py312*");
        });
        it("parses bracket fields", () => {
            const spec = new MatchSpec(
                'pytest[version=">=8", build_number=">=2", license=MIT, extras=[dev]]',
            );
            expect(spec.version?.toString()).toBe(">=8");
            expect(spec.buildNumber).toBe(">=2");
            expect(spec.license).toBe("MIT");
            expect(spec.extras).toEqual(["dev"]);
            expect(spec.build).toBeUndefined();
            expect(spec.channel).toBeUndefined();
            expect(spec.md5).toBeUndefined();
        });
        it("rejects name globs by default", () => {
            expect(() => new MatchSpec("numpy*")).toThrow();
        });
        it("accepts name globs when exactNamesOnly is false", () => {
            const spec = new MatchSpec("numpy* >=1", { exactNamesOnly: false });
            expect(spec.name).toBe("numpy*");
            expect(spec.exactName).toBeUndefined();
            expect(spec.matchesName(parsePackageName("numpy-base"))).toBe(true);
            expect(spec.matchesName(parsePackageName("scipy"))).toBe(false);
        });
        it("can reject the extras syntax", () => {
            expect(
                () => new MatchSpec("pytest[extras=[dev]]", { extras: false }),
            ).toThrow();
        });
        it("throws a PARSE_MATCH_SPEC error on invalid input", () => {
            let error: unknown;
            try {
                new MatchSpec("foo[md5=xyz]");
            } catch (e) {
                error = e;
            }
            expect(isRattlerError(error)).toBe(true);
            expect((error as { code: string }).code).toBe("PARSE_MATCH_SPEC");
        });
        it("rejects invalid options", () => {
            expect(
                () =>
                    new MatchSpec("foo", {
                        strictness: "very" as unknown as "strict",
                    }),
            ).toThrow();
        });
    });

    describe("matches", () => {
        const record = new PackageRecord({
            name: "foo",
            version: "1.2.3",
            build: "py312_0",
            build_number: 0,
            subdir: "linux-64",
        });
        it("matches a package record", () => {
            expect(new MatchSpec("foo >=1.2").matches(record)).toBe(true);
            expect(new MatchSpec("foo <1").matches(record)).toBe(false);
            expect(new MatchSpec("foo * py311*").matches(record)).toBe(false);
            expect(new MatchSpec("bar").matches(record)).toBe(false);
        });
        it("matches a repodata record including its url", () => {
            const url =
                "https://conda.anaconda.org/conda-forge/linux-64/foo-1.2.3-py312_0.conda";
            const repoDataRecord = new RepoDataRecord({
                name: "foo",
                version: "1.2.3",
                build: "py312_0",
                build_number: 0,
                subdir: "linux-64",
                fn: "foo-1.2.3-py312_0.conda",
                url,
            });
            const sameUrl = new MatchSpec(`foo[url="${url}"]`);
            const otherUrl = new MatchSpec(
                'foo[url="https://example.com/foo-1.2.3-py312_0.conda"]',
            );
            expect(sameUrl.url).toBe(url);
            expect(sameUrl.matchesRepoDataRecord(repoDataRecord)).toBe(true);
            expect(otherUrl.matchesRepoDataRecord(repoDataRecord)).toBe(false);
            expect(
                new MatchSpec("foo >=1").matchesRepoDataRecord(repoDataRecord),
            ).toBe(true);
        });
        it("exposes its version spec", () => {
            const version = new MatchSpec("foo >=1.2").version;
            expect(version?.matches(new Version("1.3"))).toBe(true);
            expect(version?.matches(new Version("1.1"))).toBe(false);
        });
    });
});
