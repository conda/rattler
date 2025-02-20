import path from "path";
import { fileURLToPath } from "url";
import { merge } from "webpack-merge";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const options = {
    entry: "./src/index.ts",
    module: {
        rules: [
            {
                test: /\.ts$/,
                exclude: /node_modules/,
                use: "ts-loader",
            },
        ],
    },
    optimization: {
        minimize: false,
    },
    resolve: {
        extensions: [".ts", ".js"],
    },
    experiments: {
        asyncWebAssembly: true,
    },
};

export default [
    merge(options, {
        output: {
            assetModuleFilename: "[name][ext]",
            clean: false,
            path: path.resolve(__dirname, "dist/"),
            filename: "umd.js",
            globalObject: "typeof self !== 'undefined' ? self : this",
            library: "rattler",
            libraryTarget: "umd",
            umdNamedDefine: true,
        },
    }),
    merge(options, {
        output: {
            assetModuleFilename: "[name][ext]",
            clean: false,
            path: path.resolve(__dirname, "dist/"),
            filename: "esm.mjs",
            libraryTarget: "module",
            umdNamedDefine: false,
        },
        experiments: {
            outputModule: true,
        },
    }),
];
