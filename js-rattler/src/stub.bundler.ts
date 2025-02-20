import init, {
    JsVersion,
    JsVersionSpec,
    ParseStrictness,
} from "../pkg/js_rattler";

import wasmModule from "../pkg/js_rattler_bg.wasm";

await init(wasmModule());

export { JsVersion, JsVersionSpec, ParseStrictness };
