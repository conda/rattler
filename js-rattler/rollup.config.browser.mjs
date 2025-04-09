import { wasm } from "@rollup/plugin-wasm";
import typescript from "@rollup/plugin-typescript";
import { nodeResolve } from "@rollup/plugin-node-resolve";
import commonjs from "@rollup/plugin-commonjs";

export default {
  input: "src/esm.ts",
  output: {
    file: "dist/rattler.browser.js",
    format: "iife",
    name: "Rattler", // This will be the global variable name
    sourcemap: true,
  },
  plugins: [
    commonjs(),
    wasm({
      targetEnv: "auto-inline",
      sync: ["pkg/js_rattler_bg.wasm"],
    }),
    nodeResolve(),
    typescript({
      sourceMap: true,
      declaration: false,
      declarationMap: false,
      inlineSources: false,
      tsconfig: "./tsconfig.rollup.json",
    }),
  ],
};
