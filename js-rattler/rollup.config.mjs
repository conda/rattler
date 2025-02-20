/** Build node distribution */

import { wasm } from "@rollup/plugin-wasm";
import typescript from "@rollup/plugin-typescript";
import { nodeResolve } from "@rollup/plugin-node-resolve";
import commonjs from "@rollup/plugin-commonjs";
import esmShim from "@rollup/plugin-esm-shim";

const commonPlugins = [
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
]

export default [
    {
        input: "src/index.ts",
        output: {
            file: "dist/rattler.node.mjs",
            sourcemap: false,
            format: "esm",
        },
        plugins: [
            esmShim(),
            ...commonPlugins,
        ],
    },
    {
        input: "src/index.ts",
        output: {
            file: "dist/rattler.node.cjs",
            sourcemap: false,
            format: "commonjs",
        },
        plugins: [
            ...commonPlugins,
        ],
    },
];
