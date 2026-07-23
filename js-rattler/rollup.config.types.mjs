/** Builds a rollup d.ts file */

import dts from "rollup-plugin-dts";

export default {
    // path to your declaration files root
    input: "src/index.ts",
    output: [{ file: "dist/index.d.ts", format: "es" }],
    plugins: [dts()],
};
