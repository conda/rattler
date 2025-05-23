import { describe, expect, it } from "@jest/globals";
import { simpleSolve } from "./solve";

describe("solving", () => {
    it("python should yield three packages", () => {
        return simpleSolve(
            ["python"],
            [
                "https://prefix.dev/emscripten-forge-dev",
                "https://prefix.dev/conda-forge",
            ],
            ["emscripten-wasm32", "noarch"],
            [],
        ).then((result) => {
            const expectedPrefixes = [
                "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/python-",
                "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/python_abi-",
                "https://prefix.dev/emscripten-forge-dev/noarch/emscripten-abi-",
            ];

            const urls = result.map((pkg) => pkg.url).sort();
            expect(urls.length).toBe(expectedPrefixes.length);

            urls.forEach((url, index) => {
                expect(url.startsWith(expectedPrefixes[index])).toBe(true);
            });
        });
    });

    it("python should yield three packages but numpy should not be solved again", () => {
        let big = 1n;
        let buildNumber = Number(big);
        return simpleSolve(
            ["python", "numpy"],
            [
                "https://prefix.dev/emscripten-forge-dev",
                "https://prefix.dev/conda-forge",
            ],
            ["emscripten-wasm32", "noarch"],
            [
                {
                    build: "py313h6394566_1",
                    buildNumber,
                    filename: "numpy-2.2.6-py313h6394566_1.tar.bz2",
                    packageName: "numpy",
                    repoName: "https://prefix.dev/emscripten-forge-dev/",
                    url: "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/numpy-2.2.6-py313h6394566_1.tar.bz2",
                    version: "2.2.6",
                },
            ],
        ).then((result) => {
            const expectedPrefixes = [
                "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/python-",
                "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/python_abi-",
                "https://prefix.dev/emscripten-forge-dev/noarch/emscripten-abi-",
            ];

            const urls = result.map((pkg) => pkg.url).sort();
            expect(urls.length).toBe(expectedPrefixes.length);

            urls.forEach((url, index) => {
                expect(url.startsWith(expectedPrefixes[index])).toBe(true);
            });
        });
    });

    it("there should be 4 packages with python and numpy", () => {
        let big = 1n;
        let buildNumber = Number(big);
        return simpleSolve(
            ["python", "numpy<2.2.6"],
            [
                "https://prefix.dev/emscripten-forge-dev",
                "https://prefix.dev/conda-forge",
            ],
            ["emscripten-wasm32", "noarch"],
            [
                {
                    build: "py313h6394566_1",
                    buildNumber,
                    filename: "numpy-2.2.6-py313h6394566_1.tar.bz2",
                    packageName: "numpy",
                    repoName: "https://prefix.dev/emscripten-forge-dev/",
                    url: "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/numpy-2.2.6-py313h6394566_1.tar.bz2",
                    version: "2.2.6",
                },
            ],
        ).then((result) => {
            const expectedPrefixes = [
                "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/numpy-",
                "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/python-",
                "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/python_abi-",
                "https://prefix.dev/emscripten-forge-dev/noarch/emscripten-abi-",
            ];

            const urls = result.map((pkg) => pkg.url).sort();
            expect(urls.length).toBe(expectedPrefixes.length);

            urls.forEach((url, index) => {
                expect(url.startsWith(expectedPrefixes[index])).toBe(true);
            });
        });
    });
});
