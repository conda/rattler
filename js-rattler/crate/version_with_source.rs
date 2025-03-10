use rattler_conda_types::VersionWithSource;
use std::str::FromStr;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen_futures::js_sys::JsString;

use crate::{version::JsVersion, JsError};

/// Holds a version and the string it was created from. This is useful if you
/// want to retain the original string the version was created from. This might
/// be useful in cases where you have multiple strings that are represented by
/// the same `Version` but you still want to be able to distinguish them.
///
/// The string `1.0` and `1.01` represent the same version. When you print the
/// parsed version though it will come out as `1.0`. You loose the original
/// representation. This struct stores the original source string.
///
/// @public
#[wasm_bindgen(js_name = "VersionWithSource")]
#[repr(transparent)]
#[derive(Clone, Eq, PartialEq)]
pub struct JsVersionWithSource {
    inner: VersionWithSource,
}

impl From<VersionWithSource> for JsVersionWithSource {
    fn from(value: VersionWithSource) -> Self {
        JsVersionWithSource { inner: value }
    }
}

impl From<JsVersionWithSource> for VersionWithSource {
    fn from(value: JsVersionWithSource) -> Self {
        value.inner
    }
}

impl AsRef<VersionWithSource> for JsVersionWithSource {
    fn as_ref(&self) -> &VersionWithSource {
        &self.inner
    }
}

#[wasm_bindgen(js_class = "VersionWithSource")]
impl JsVersionWithSource {
    #[wasm_bindgen(constructor)]
    pub fn new(source: &str) -> Result<Self, JsError> {
        Ok(Self {
            inner: VersionWithSource::from_str(source)?,
        })
    }

    #[wasm_bindgen(getter)]
    pub fn version(&self) -> JsVersion {
        JsVersion::from(self.inner.version().clone())
    }

    #[wasm_bindgen(getter)]
    pub fn source(&self) -> JsString {
        let source: &str = &self.inner.as_str();
        JsString::from(source)
    }
}
