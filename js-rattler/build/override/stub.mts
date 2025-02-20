import mod from "../pkg/js_rattler_bg.wasm";

import {
    initSync,
    JsVersion,
    JsVersionSpec,
    ParseStrictness,
} from "../pkg/js_rattler";

await initSync({ module: await mod() });

export { JsVersion, JsVersionSpec, ParseStrictness };
