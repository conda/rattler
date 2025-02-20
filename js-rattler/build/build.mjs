/** We provide package for nodejs commonjs and universal esm. */

import * as fs from "node:fs/promises";

import { exec } from "@actions/exec";
import path from "node:path";

const packageRoot = path.resolve(import.meta.dirname, "..");

await exec("npm", ["exec", "rimraf", "dist", "pkg"]);

await fs.copyFile(
    path.resolve(packageRoot, "./build/override/stub.mts"),
    path.resolve(packageRoot, "./src/stub.ts"),
);
await exec("npm", ["run", "build:esm"]);

await fs.copyFile(
    path.resolve(packageRoot, "./build/override/stub.cts"),
    path.resolve(packageRoot, "./src/stub.ts"),
);
await exec("npm", ["run", "build:nodejs"]);

await exec("npm", ["exec", "api-extractor", "run", "--verbose"]);

await fs.copyFile(
    path.resolve(packageRoot, "pkg/js_rattler_bg.wasm"),
    path.resolve(packageRoot, "dist/js_rattler_bg.wasm"),
);
