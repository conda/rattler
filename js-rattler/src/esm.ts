export * from "./index";

import mod from "../pkg/js_rattler_bg.wasm";

//@ts-ignore
import { initSync } from "../pkg/js_rattler";

//@ts-ignore
initSync({ module: mod() });
