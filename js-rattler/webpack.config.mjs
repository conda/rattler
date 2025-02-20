import path from "path";
import { fileURLToPath } from "url";
import WasmPackPlugin from "@wasm-tool/wasm-pack-plugin";
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
        minimize: false
    },
    resolve: {
        extensions: [".ts", ".js"],
    },
    plugins: [
        new WasmPackPlugin({
            crateDirectory: path.resolve(__dirname, "crate"),
            args: "--log-level warn",
            extraArgs: "--target bundler --mode normal --release",
            forceMode: "production",
        }),
    ],
    experiments: {
        asyncWebAssembly: true,
    },
};

export default [
    merge(options, {
        output: {
            assetModuleFilename: "[name][ext]",
            clean: true,
            path: path.resolve(__dirname, "dist/umd/"),
            filename: "[name].js",
            library: "js-rattler",
            libraryTarget: "umd",
            umdNamedDefine: true,
        },
    }),
    merge(options, {
        output: {
            assetModuleFilename: "[name][ext]",
            clean: true,
            path: path.resolve(__dirname, "dist/module/"),
            filename: "[name].mjs",
            libraryTarget: "module",
            umdNamedDefine: false,
        },
        experiments: {
            outputModule: true,
        },
    }),
];
