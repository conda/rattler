import { wasm } from "@rollup/plugin-wasm";
import typescript from "@rollup/plugin-typescript";
import { nodeResolve } from "@rollup/plugin-node-resolve";
import commonjs from '@rollup/plugin-commonjs';

export default {
  input: "src/index.ts",
  output: {
    file: 'dist/rattler.node.mjs',
    sourcemap: false,
    format: "esm",
  },
  plugins: [
    commonjs(),
    wasm({
      targetEnv: "auto-inline",
      sync: ['pkg/js_rattler_bg.wasm']
    }),
    nodeResolve(),
    typescript({
      sourceMap: false,
      declaration: false,
      declarationMap: false,
      inlineSources: false,
      tsconfig: './tsconfig.rollup.json',
    }),
  ],
};
