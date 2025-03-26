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
        ).then((result) =>
            expect(result.map((pkg) => pkg.url).sort()).toStrictEqual([
                "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/python-3.13.1-h_dd8ba0c_4_cp313.conda",
                "https://prefix.dev/emscripten-forge-dev/emscripten-wasm32/python_abi-3.13.1-0_cp313.tar.bz2",
                "https://prefix.dev/emscripten-forge-dev/noarch/emscripten-abi-3.1.73-h267e887_7.tar.bz2",
            ]),
        );
    });
});
