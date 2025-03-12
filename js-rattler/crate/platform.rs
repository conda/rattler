use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const PLATFORM_D_TS: &'static str = include_str!("platform.d.ts");

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "Arch")]
    pub type JsArch;

    #[wasm_bindgen(typescript_type = "Arch | undefined")]
    pub type JsArchOption;

    #[wasm_bindgen(typescript_type = "Platform")]
    pub type JsPlatform;
}

impl From<String> for JsArch {
    fn from(value: String) -> Self {
        JsValue::from(value).into()
    }
}

impl TryFrom<JsArch> for String {
    type Error = serde_wasm_bindgen::Error;

    fn try_from(value: JsArch) -> Result<Self, Self::Error> {
        serde_wasm_bindgen::from_value(value.obj)
    }
}
