import { describe, expect, it } from "@jest/globals";
import { RepoDataRecord, RepoDataRecordJson } from "./RepoDataRecord";
import { Version } from "./Version";
import { isRattlerError } from "./RattlerError";
import "./areVersionsEqual";

const json: RepoDataRecordJson = {
    name: "foo",
    version: "1.2.3",
    build: "py312_0",
    build_number: 0,
    subdir: "linux-64",
    depends: ["python >=3.12"],
    license: "MIT",
    timestamp: 1700000000000,
    fn: "foo-1.2.3-py312_0.conda",
    url: "https://conda.anaconda.org/conda-forge/linux-64/foo-1.2.3-py312_0.conda",
    channel: "https://conda.anaconda.org/conda-forge/",
};

function record(overrides: Partial<RepoDataRecordJson>): RepoDataRecord {
    return new RepoDataRecord({ ...json, ...overrides });
}

describe("RepoDataRecord", () => {
    it("round trips its json representation", () => {
        expect(new RepoDataRecord(json).toJson()).toEqual(json);
    });
    it("exposes the package record fields", () => {
        const r = new RepoDataRecord(json);
        expect(r.name).toBe("foo");
        expect(r.version.version).toEqual(new Version("1.2.3"));
        expect(r.build).toBe("py312_0");
        expect(r.buildNumber).toBe(0);
        expect(r.subdir).toBe("linux-64");
        expect(r.depends).toEqual(["python >=3.12"]);
        expect(r.license).toBe("MIT");
        expect(r.timestamp).toEqual(new Date(1700000000000));
    });
    it("exposes the archive fields", () => {
        const r = new RepoDataRecord(json);
        expect(r.fileName).toBe("foo-1.2.3-py312_0.conda");
        expect(r.url).toBe(
            "https://conda.anaconda.org/conda-forge/linux-64/foo-1.2.3-py312_0.conda",
        );
        expect(r.channel).toBe("https://conda.anaconda.org/conda-forge/");
    });
    it("archive fields can be modified", () => {
        const r = new RepoDataRecord(json);
        r.fileName = "foo-1.2.3-py312_0.tar.bz2";
        r.url = "https://example.com/foo-1.2.3-py312_0.tar.bz2";
        r.channel = undefined;
        r.build = "py312_1";
        expect(r.toJson()).toMatchObject({
            fn: "foo-1.2.3-py312_0.tar.bz2",
            url: "https://example.com/foo-1.2.3-py312_0.tar.bz2",
            build: "py312_1",
        });
        expect(r.channel).toBeUndefined();
    });
    it("rejects an invalid file name or url", () => {
        const r = new RepoDataRecord(json);
        for (const [set, code] of [
            [() => (r.fileName = "foo.zip"), "PARSE_FILE_NAME"],
            [() => (r.url = "not a url"), "PARSE_URL"],
        ] as const) {
            let error: unknown;
            try {
                set();
            } catch (e) {
                error = e;
            }
            expect(isRattlerError(error)).toBe(true);
            expect((error as { code: string }).code).toBe(code);
        }
    });
    it("rejects json without an archive url", () => {
        const { url: _url, ...withoutUrl } = json;
        expect(
            () => new RepoDataRecord(withoutUrl as RepoDataRecordJson),
        ).toThrow();
    });
    it("sorts by name, version, build number and timestamp", () => {
        const records = [
            record({ version: "1.10.0" }),
            record({ name: "bar" }),
            record({ version: "1.2.3", build_number: 2 }),
            record({ version: "1.9" }),
            record({ timestamp: 1600000000000 }),
        ];
        records.sort((a, b) => a.compare(b));
        expect(
            records.map(
                (r) =>
                    `${r.name} ${r.version.source} ${r.buildNumber} ${r.timestamp?.getTime()}`,
            ),
        ).toEqual([
            "bar 1.2.3 0 1700000000000",
            "foo 1.2.3 0 1600000000000",
            "foo 1.2.3 2 1700000000000",
            "foo 1.9 0 1700000000000",
            "foo 1.10.0 0 1700000000000",
        ]);
    });
});
