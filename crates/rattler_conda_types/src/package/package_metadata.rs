use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
/// This is metadata about the package version that is contained in `.conda` packages only, in the
/// outer zip archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageMetadata {
    /// The version of the conda package format. This is currently always 2.
    pub conda_pkg_format_version: u32,
}

#[wasm_bindgen]
pub struct  JsPackageMetadata {
    internal: PackageMetadata,
}

#[wasm_bindgen]
impl JsPackageMetadata {
    #[wasm_bindgen(constructor)]
    pub fn new(conda_pkg_format_version: u32) -> JsPackageMetadata {
        JsPackageMetadata {
            internal: PackageMetadata {
                conda_pkg_format_version
            }
        }
    }

    #[wasm_bindgen(getter)]
    pub fn conda_pkg_format_version(&self) -> u32 {
        self.internal.conda_pkg_format_version
    }

    #[wasm_bindgen(setter)]
    pub fn set_conda_pkg_format_version(&mut self, new_version: u32) {
        self.internal.conda_pkg_format_version = new_version;
    }

    #[wasm_bindgen(js_name =  "default")]
    pub fn js_default() -> JsPackageMetadata {
        JsPackageMetadata {
            internal: PackageMetadata::default()
        }
    }
}

impl Default for PackageMetadata {
    fn default() -> Self {
        Self {
            conda_pkg_format_version: 2,
        }
    }
}
