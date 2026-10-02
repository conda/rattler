import { describe, expect, it } from "@jest/globals";
import { MatchSpec } from "./MatchSpec";
import { PackageRecord } from "./PackageRecord";
import { isRattlerError } from "./RattlerError";

const record = (
    overrides: Partial<ConstructorParameters<typeof PackageRecord>[0]> = {},
) =>
    new PackageRecord({
        name: "numpy",
        version: "2.1.0",
        build: "py313h1234_0",
        build_number: 0,
        subdir: "linux-64",
        depends: ["python >=3.13,<3.14.0a0"],
        ...overrides,
    });

describe("MatchSpec", () => {
    describe("constructor", () => {
        it("parses a bare name", () => {
            const spec = new MatchSpec("numpy");
            expect(spec.name).toBe("numpy");
            expect(spec.version).toBeUndefined();
            expect(spec.build).toBeUndefined();
        });
        it("parses a name with a version and build", () => {
            const spec = new MatchSpec("numpy >=2 py313*");
            expect(spec.name).toBe("numpy");
            expect(spec.version?.toString()).toBe(">=2");
            expect(spec.build).toBe("py313*");
        });
        it("normalizes the package name", () => {
            expect(new MatchSpec("NumPy").name).toBe("numpy");
        });
        it("parses channel and subdir", () => {
            const spec = new MatchSpec("conda-forge/linux-64::numpy 1.*");
            expect(spec.channel).toBe(
                "https://conda.anaconda.org/conda-forge/",
            );
            expect(spec.subdir).toBe("linux-64");
            expect(spec.version?.toString()).toBe("1.*");
        });
        it("parses bracket fields", () => {
            const spec = new MatchSpec(
                'numpy[version=">=1.0", build_number=">=3", license="BSD-3-Clause"]',
            );
            expect(spec.version?.toString()).toBe(">=1.0");
            expect(spec.buildNumber).toBe(">=3");
            expect(spec.license).toBe("BSD-3-Clause");
        });
        it("parses repodata v3 syntax", () => {
            expect(new MatchSpec("python[extras=[foo,bar]]").extras).toEqual([
                "foo",
                "bar",
            ]);
            expect(new MatchSpec("python[flags=[cuda]]").flags).toEqual([
                "cuda",
            ]);
            expect(new MatchSpec('python[when="numpy >=2"]').condition).toBe(
                "numpy>=2",
            );
        });
        it("rejects a glob name", () => {
            expect(() => new MatchSpec("py*")).toThrow();
        });
        it("parses hashes as hex strings", () => {
            const sha =
                "1154fceeb5c4ee9bb97d245713ac21eb1910237c724d2b7103747215663273c2";
            expect(new MatchSpec(`numpy[sha256=${sha}]`).sha256).toBe(sha);
            expect(
                new MatchSpec("numpy[md5=d65ab674acf3b7294ebacaec05fc5b54]")
                    .md5,
            ).toBe("d65ab674acf3b7294ebacaec05fc5b54");
        });
        it("throws a coded error on invalid input", () => {
            expect.assertions(2);
            try {
                new MatchSpec("numpy >=");
            } catch (err) {
                expect(isRattlerError(err)).toBe(true);
                if (isRattlerError(err))
                    expect(err.code).toBe("PARSE_MATCH_SPEC");
            }
        });
        it("honors strict parsing", () => {
            expect(() => new MatchSpec("numpy 1.0.*", "strict")).not.toThrow();
            expect(() => new MatchSpec("numpy >=1.0.*", "strict")).toThrow();
            expect(
                () => new MatchSpec("numpy >=1.0.*", "lenient"),
            ).not.toThrow();
        });
    });
    describe("toString", () => {
        it("round trips", () => {
            for (const text of [
                "numpy",
                "numpy >=2",
                "numpy >=2 py313*",
                "conda-forge::numpy",
            ]) {
                expect(new MatchSpec(text).toString()).toBe(text);
            }
        });
    });
    describe("matches", () => {
        it("matches a PackageRecord", () => {
            expect(new MatchSpec("numpy >=2").matches(record())).toBe(true);
            expect(new MatchSpec("numpy >=3").matches(record())).toBe(false);
            expect(new MatchSpec("numpy >=2 py313*").matches(record())).toBe(
                true,
            );
            expect(new MatchSpec("numpy >=2 py312*").matches(record())).toBe(
                false,
            );
            expect(new MatchSpec("numpy[build=py313*]").matches(record())).toBe(
                true,
            );
            expect(new MatchSpec("scipy").matches(record())).toBe(false);
        });
        it("matches plain record JSON", () => {
            const json = record().toJson();
            expect(new MatchSpec("numpy >=2").matchesJson(json)).toBe(true);
            expect(new MatchSpec("numpy <2").matchesJson(json)).toBe(false);
        });
        it("matches repodata record JSON including the url", () => {
            const json = {
                ...record().toJson(),
                fn: "numpy-2.1.0-py313h1234_0.conda",
                url: "https://conda.anaconda.org/conda-forge/linux-64/numpy-2.1.0-py313h1234_0.conda",
                channel: "https://conda.anaconda.org/conda-forge/",
            };
            expect(new MatchSpec("numpy").matchesJson(json)).toBe(true);
            expect(
                new MatchSpec(
                    "numpy[url=https://conda.anaconda.org/conda-forge/linux-64/numpy-2.1.0-py313h1234_0.conda]",
                ).matchesJson(json),
            ).toBe(true);
            expect(
                new MatchSpec(
                    "numpy[url=https://example.com/other.conda]",
                ).matchesJson(json),
            ).toBe(false);
        });
        it("throws a coded error on an invalid record", () => {
            expect.assertions(1);
            try {
                new MatchSpec("numpy").matchesJson({ name: "numpy" } as any);
            } catch (err) {
                if (isRattlerError(err)) expect(err.code).toBe("SERDE");
            }
        });
    });
});
