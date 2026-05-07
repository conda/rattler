use rattler_conda_types::{NoArchType, RawNoArchType};
use serde::de::Error;
use wasm_bindgen::{prelude::wasm_bindgen, JsValue};
use wasm_bindgen_futures::{js_sys, js_sys::JsString};

#[wasm_bindgen(typescript_custom_section)]
const NOARCH_TYPE_D_TS: &'static str = include_str!("noarch_type.d.ts");

#[wasm_bindgen]
#[rustfmt::skip] // Otherwise rustfmt strips the literals
extern "C" {
    #[wasm_bindgen(typescript_type = "NoArchType")]
    pub type JsNoArchType;

    #[wasm_bindgen(thread_local_v2, static_string)]
    static GENERIC: JsString = "generic";

    #[wasm_bindgen(thread_local_v2, static_string)]
    static PYTHON: JsString = "python";
}

impl From<NoArchType> for JsNoArchType {
    fn from(value: NoArchType) -> Self {
        let value = match value.0 {
            None => JsValue::UNDEFINED,
            Some(RawNoArchType::GenericV1) => JsValue::FALSE,
            Some(RawNoArchType::GenericV2) => GENERIC.with(|str| str.into()),
            Some(RawNoArchType::Python) => PYTHON.with(|str| str.into()),
        };
        JsNoArchType::from(value)
    }
}

impl TryFrom<JsNoArchType> for NoArchType {
    type Error = serde_wasm_bindgen::Error;

    fn try_from(value: JsNoArchType) -> Result<Self, Self::Error> {
        if let Some(str) = value.obj.as_string() {
            if str == "generic" {
                return Ok(NoArchType(Some(RawNoArchType::GenericV2)));
            } else if str == "python" {
                return Ok(NoArchType(Some(RawNoArchType::Python)));
            }
        } else if value.obj.is_truthy() {
            return Ok(NoArchType(Some(RawNoArchType::GenericV1)));
        } else if value.obj.is_falsy() {
            return Ok(NoArchType(None));
        }

        Err(serde_wasm_bindgen::Error::custom("Invalid NoArchType"))
    }
}
