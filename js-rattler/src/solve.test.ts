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
});
