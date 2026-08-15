import { describe, expect, it } from "@jest/globals";
import { createServer } from "node:http";
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
                    extraDepends: { test: ["pytest >=8"] },
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
            expect(numpy?.extraDepends).toEqual({ test: ["pytest >=8"] });
        });
    });

    it("uses extras from the locked package's channel", async () => {
        const repodata = (child: string) =>
            JSON.stringify({
                info: { subdir: "noarch" },
                packages: {},
                "packages.conda": {},
                repodata_version: 1,
                v3: {
                    "tar.bz2": {
                        [`${child}-1.0-0`]: {
                            build: "0",
                            build_number: 0,
                            name: child,
                            subdir: "noarch",
                            version: "1.0",
                        },
                        "parent-1.0-0": {
                            build: "0",
                            build_number: 0,
                            extra_depends: { test: [child] },
                            name: "parent",
                            subdir: "noarch",
                            version: "1.0",
                        },
                    },
                },
            });
        const repodata_by_path = new Map([
            ["/first/noarch/repodata.json", repodata("first-child")],
            ["/second/noarch/repodata.json", repodata("second-child")],
        ]);
        const server = createServer((request, response) => {
            const repodata = repodata_by_path.get(request.url ?? "");
            if (repodata !== undefined) {
                response.setHeader("content-type", "application/json");
                response.end(repodata);
            } else {
                response.statusCode = 404;
                response.end();
            }
        });
        await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
        const address = server.address();
        if (address === null || typeof address === "string") {
            throw new Error("test server did not expose a TCP address");
        }
        const first_channel = `http://127.0.0.1:${address.port}/first`;
        const second_channel = `http://127.0.0.1:${address.port}/second`;

        try {
            const locked_parent = (channel: string) => ({
                build: "0",
                buildNumber: 0n,
                depends: [],
                filename: "parent-1.0-0.tar.bz2",
                packageName: "parent",
                repoName: `${channel}/`,
                subdir: "noarch",
                url: `${channel}/noarch/parent-1.0-0.tar.bz2`,
                version: "1.0",
            });
            const result = await simpleSolve(
                ["parent[extras=[test]]"],
                // The locked artifact is from `first_channel`, despite `second_channel`
                // having higher priority. This makes the assertion exercise URL matching.
                [second_channel, first_channel],
                ["noarch"],
                [locked_parent(first_channel)],
            );

            expect(result.find((pkg) => pkg.packageName === "first-child")).toBeDefined();
            expect(result.find((pkg) => pkg.packageName === "second-child")).toBeUndefined();
            expect(result.find((pkg) => pkg.packageName === "parent")?.extraDepends).toEqual({
                test: ["first-child"],
            });

            // A name/version/build lookup would retain `first_channel`, the last
            // matching record in repodata. Locking `second_channel` proves the
            // metadata is selected by the artifact URL instead.
            const high_priority_locked_result = await simpleSolve(
                ["parent[extras=[test]]"],
                [second_channel, first_channel],
                ["noarch"],
                [locked_parent(second_channel)],
            );

            expect(
                high_priority_locked_result.find((pkg) => pkg.packageName === "second-child"),
            ).toBeDefined();
            expect(
                high_priority_locked_result.find((pkg) => pkg.packageName === "first-child"),
            ).toBeUndefined();
            expect(
                high_priority_locked_result.find((pkg) => pkg.packageName === "parent")
                    ?.extraDepends,
            ).toEqual({ test: ["second-child"] });
        } finally {
            await new Promise<void>((resolve, reject) =>
                server.close((error) => (error === undefined ? resolve() : reject(error))),
            );
        }
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
