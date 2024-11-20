import path from "path";
import {fileURLToPath} from 'url';
import WasmPackPlugin from '@wasm-tool/wasm-pack-plugin';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

export default {
    entry: './src/index.ts',
    module: {
        rules: [
            {
                test: /\.ts$/,
                exclude: /node_modules/,
                use: 'ts-loader',
            },
        ],
    },
    resolve: {
        extensions: ['.ts', '.js'],
    },
    output: {
        assetModuleFilename: '[name][ext]',
        clean: true,
        path: path.resolve(__dirname, 'dist'),
        filename: '[name].js',
        library: 'js-rattler',
        libraryTarget: 'umd',
        umdNamedDefine: true
    },
    plugins: [
        new WasmPackPlugin({
            crateDirectory: path.resolve(__dirname, "crate"),
            args: '--log-level warn',
            extraArgs: "--target web --mode normal --release",
            forceMode: "production",
        }),
    ],
    experiments: {
        asyncWebAssembly: true
    }
}
