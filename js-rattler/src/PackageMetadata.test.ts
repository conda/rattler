import { describe, expect, it } from "@jest/globals";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
    parseAboutJson,
    parseIndexJson,
    parsePathsJson,
    parseRunExportsJson,
} from "./PackageMetadata";
import { isRattlerError } from "./RattlerError";

function testData(name: string): string {
    return readFileSync(join(process.cwd(), "..", "test-data", name), "utf8");
}

function errorCode(f: () => unknown): string | undefined {
    try {
        f();
    } catch (e) {
        return isRattlerError(e) ? e.code : "NOT_A_RATTLER_ERROR";
    }
    return undefined;
}

describe("parseIndexJson", () => {
    it("parses an index.json file", () => {
        const index = parseIndexJson(
            testData("conda-22.11.1-py38haa244fe_1-index.json"),
        );
        expect(index.name).toBe("conda");
        expect(index.version).toBe("22.11.1");
        expect(index.build).toBe("py38haa244fe_1");
        expect(index.build_number).toBe(1);
        expect(index.subdir).toBe("win-64");
        expect(index.depends).toContain("python >=3.8,<3.9.0a0");
        expect(index.constrains).toContain("conda-build >=3");
        expect(index.license).toBe("BSD-3-Clause");
        expect(typeof index.timestamp).toBe("number");
    });
    it("accepts an already parsed object", () => {
        const index = parseIndexJson({
            name: "foo",
            version: "1.0",
            build: "h123_0",
            build_number: 0,
            noarch: "python",
        });
        expect(index.name).toBe("foo");
        expect(index.depends).toEqual([]);
        expect(index.noarch).toBe("python");
    });
    it("rejects an invalid package name", () => {
        expect(
            errorCode(() =>
                parseIndexJson({
                    name: "foo!",
                    version: "1.0",
                    build: "h123_0",
                    build_number: 0,
                }),
            ),
        ).toBe("SERDE");
    });
    it("reports malformed json with PARSE_JSON", () => {
        expect(errorCode(() => parseIndexJson("{"))).toBe("PARSE_JSON");
    });
});

describe("parseAboutJson", () => {
    it("parses an about.json file", () => {
        const about = parseAboutJson(testData("dummy-about.json"));
        expect(about).toEqual({
            channels: ["https://conda.anaconda.org/conda-forge"],
            description: "A dummy description.",
            dev_url: "https://github.com/conda/rattler",
            doc_url: "https://conda.github.io/rattler/py-rattler/",
            home: "http://github.com/conda/rattler",
            license: "BSD-3-Clause",
            source_url: "https://github.com/conda/rattler",
            summary: "A dummy summary.",
        });
    });
    it("keeps several urls and extra metadata", () => {
        const about = parseAboutJson({
            home: ["https://a.example/", "https://b.example/"],
            extra: { recipe_maintainers: ["alice", "bob"], nested: { a: 1 } },
        });
        expect(about.home).toEqual([
            "https://a.example/",
            "https://b.example/",
        ]);
        expect(about.extra).toEqual({
            recipe_maintainers: ["alice", "bob"],
            nested: { a: 1 },
        });
    });
});

describe("parsePathsJson", () => {
    it("parses a paths.json file", () => {
        const paths = parsePathsJson(
            testData("conda-22.9.0-py38haa244fe_2-paths.json"),
        );
        expect(paths.paths_version).toBe(1);
        expect(paths.paths).toHaveLength(420);
        const placeholder = paths.paths.find(
            (entry) => entry._path === "Lib/site-packages/xontrib/conda.xsh",
        );
        expect(placeholder).toEqual({
            _path: "Lib/site-packages/xontrib/conda.xsh",
            path_type: "hardlink",
            file_mode: "text",
            prefix_placeholder: "D:/bld/conda_1667595064120/_h_env",
            sha256: "d1e97a11a77000c4f74ce5efd733a1d78618b44fbff3808f92907d701db25be1",
            size_in_bytes: 7409,
        });
    });
    it("rejects an unknown path type", () => {
        expect(
            errorCode(() =>
                parsePathsJson({
                    paths: [{ _path: "foo", path_type: "pipe" }],
                    paths_version: 1,
                }),
            ),
        ).toBe("SERDE");
    });
});

describe("parseRunExportsJson", () => {
    it("parses a run_exports.json file", () => {
        expect(
            parseRunExportsJson(
                testData("python-3.10.6-h2c4edbf_0_cpython-run_exports.json"),
            ),
        ).toEqual({
            noarch: ["python"],
            weak: ["python_abi 3.10.* *_cp310"],
        });
    });
    it("omits empty fields", () => {
        expect(parseRunExportsJson("{}")).toEqual({});
    });
});
