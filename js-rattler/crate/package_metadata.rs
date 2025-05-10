use rattler_conda_types::package::PackageMetadata;
use serde::{Deserialize, Serialize};
use wasm_bindgen::{prelude::wasm_bindgen, JsValue};

#[wasm_bindgen]
#[derive(Serialize, Deserialize)]
pub struct JsPackageMetadata {
    #[wasm_bindgen(skip)]
    inner: PackageMetadata,
}

#[wasm_bindgen]
impl JsPackageMetadata {
    #[wasm_bindgen(constructor)]
    pub fn new(conda_pkg_format_version: Option<f64>) -> Self {
        Self {
            inner: PackageMetadata {
                conda_pkg_format_version: conda_pkg_format_version.unwrap_or(2.0) as u64,
            },
        }
    }

    #[wasm_bindgen(getter)]
    pub fn conda_pkg_format_version(&self) -> f64 {
        self.inner.conda_pkg_format_version as f64
    }

    #[wasm_bindgen(setter)]
    pub fn set_conda_pkg_format_version(&mut self, version: f64) {
        self.inner.conda_pkg_format_version = version as u64;
    }

    #[wasm_bindgen(js_name = toJSON)]
    pub fn to_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    #[wasm_bindgen(js_name = fromJSON)]
    pub fn from_json(json: &str) -> Result<JsPackageMetadata, JsValue> {
        let inner: PackageMetadata =
            serde_json::from_str(json).map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(Self { inner })
    }
}
