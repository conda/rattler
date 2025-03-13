use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen(typescript_custom_section)]
const PACKAGE_NAME_D_TS: &'static str = include_str!("package_name.d.ts");

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "PackageName")]
    pub type JsPackageName;
}
