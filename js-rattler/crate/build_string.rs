use rattler_conda_types::package::BuildString;
use wasm_bindgen::prelude::*;

use crate::JsResult;

/// An opaque conda build string with CEP26 validation.
///
/// @public
#[wasm_bindgen(js_name = "BuildString")]
pub struct JsBuildString {
    pub(crate) inner: BuildString,
}

#[wasm_bindgen(js_class = "BuildString")]
impl JsBuildString {
    /// Validates 1–64 ASCII letters, digits, underscores, dots or plus signs.
    #[wasm_bindgen(constructor)]
    pub fn new(value: String) -> JsResult<Self> {
        Ok(Self {
            inner: value.parse::<BuildString>()?,
        })
    }

    /// Preserves the value without validation, including legacy empty builds
    /// and wheel tags. Use only when bypassing CEP26 is intentional.
    #[wasm_bindgen(js_name = "newUnchecked")]
    pub fn new_unchecked(value: String) -> Self {
        Self {
            inner: BuildString::new_unchecked(value),
        }
    }

    /// Returns the original build string.
    #[wasm_bindgen(js_name = "toString")]
    pub fn as_string(&self) -> String {
        self.inner.to_string()
    }
}
