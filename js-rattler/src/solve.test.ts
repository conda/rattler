import { describe, expect, it } from "@jest/globals";
import { simpleSolve } from "./solve";

describe("solving", () => {
    it("python should be solvable", () => {
        return simpleSolve(
            ["python"],
            [
                "https://prefix.dev/emscripten-forge-dev",
                "https://prefix.dev/conda-forge",
            ],
            ["emscripten-wasm32", "noarch"],
            [],
        ).then((result) => {
            const python = result.find((pkg) => pkg.packageName === "python");
            expect(python).toBeDefined();
        });
    });

    it("python should yield three packages and numpy 2.2.0 should be returned", () => {
        return simpleSolve(
            ["python", "numpy"],
            [
                "https://prefix.dev/emscripten-forge-dev",
                "https://prefix.dev/conda-forge",
            ],
            ["emscripten-wasm32", "noarch"],
            [
                {
                    build: "h7223423_0",
                    buildNumber: 0n,
                    depends: [
                        "emscripten-abi >=3.1.73,<3.1.74.0a0",
                        "python_abi 3.13.* *_cp313",
                    ],
                    filename: "numpy-2.2.0-h7223423_0.tar.bz2",
                    packageName: "numpy",
                    repoName: "https://prefix.dev/emscripten-forge-dev/",
                    subdir: "emscripten-wasm32",
                    url: "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/numpy-2.2.0-h7223423_0.tar.bz2",
                    version: "2.2.0",
                },
            ],
        ).then((result) => {
            const python = result.find((pkg) => pkg.packageName === "python");
            expect(python).toBeDefined();

            const numpy = result.find((pkg) => pkg.packageName === "numpy");
            expect(numpy).toBeDefined();
            expect(numpy?.version).toBe("2.2.0");
            expect(numpy?.url).toBe(
                "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/numpy-2.2.0-h7223423_0.tar.bz2",
            );
        });
    });

    it("numpy 2.2.0 should be returned", () => {
        return simpleSolve(
            ["numpy"],
            [
                "https://prefix.dev/emscripten-forge-dev",
                "https://prefix.dev/conda-forge",
            ],
            ["emscripten-wasm32", "noarch"],
            [
                {
                    build: "h7223423_0",
                    buildNumber: 0n,
                    depends: [
                        "emscripten-abi >=3.1.73,<3.1.74.0a0",
                        "python_abi 3.13.* *_cp313",
                    ],
                    filename: "numpy-2.2.0-h7223423_0.tar.bz2",
                    packageName: "numpy",
                    repoName: "https://prefix.dev/emscripten-forge-dev/",
                    subdir: "emscripten-wasm32",
                    url: "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/numpy-2.2.0-h7223423_0.tar.bz2",
                    version: "2.2.0",
                },
            ],
        ).then((result) => {
            const urls = result.map((pkg) => pkg.url);
            expect(urls).toContain(
                "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/numpy-2.2.0-h7223423_0.tar.bz2",
            );
        });
    });

    it("numpy=2.2.6 should be returned", () => {
        return simpleSolve(
            ["numpy=2.2.6"],
            [
                "https://prefix.dev/emscripten-forge-dev",
                "https://prefix.dev/conda-forge",
            ],
            ["emscripten-wasm32", "noarch"],
            [
                {
                    build: "h7223423_0",
                    buildNumber: 0n,
                    depends: [
                        "emscripten-abi >=3.1.73,<3.1.74.0a0",
                        "python_abi 3.13.* *_cp313",
                    ],
                    filename: "numpy-2.2.0-h7223423_0.tar.bz2",
                    packageName: "numpy",
                    repoName: "https://prefix.dev/emscripten-forge-dev/",
                    subdir: "emscripten-wasm32",
                    url: "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/numpy-2.2.0-h7223423_0.tar.bz2",
                    version: "2.2.0",
                },
            ],
        ).then((result) => {
            const urls = result.map((pkg) => pkg.url).sort();
            const numpy = result.find((pkg) => pkg.packageName === "numpy");
            expect(numpy).toBeDefined();
            expect(numpy?.version).toBe("2.2.6");
        });
    });
});
