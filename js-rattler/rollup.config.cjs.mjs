/** Build node cjs distribution */

import { wasm } from "@rollup/plugin-wasm";
import typescript from "@rollup/plugin-typescript";
import { nodeResolve } from "@rollup/plugin-node-resolve";
import commonjs from "@rollup/plugin-commonjs";
import esmShim from "@rollup/plugin-esm-shim";

export default {
    input: "src/index.ts",
    output: {
        file: "dist/rattler.node.cjs",
        sourcemap: false,
        format: "commonjs",
    },
    plugins: [
        esmShim(),
        commonjs(),
        wasm({
            targetEnv: "auto-inline",
            sync: ["pkg/js_rattler_bg.wasm"],
        }),
        nodeResolve(),
        typescript({
            sourceMap: false,
            declaration: false,
            declarationMap: false,
            inlineSources: false,
            tsconfig: "./tsconfig.rollup.json",
        }),
    ],
};
