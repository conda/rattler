use crate::version::JsVersion;
use crate::{JsParseStrictness, JsResult};
use rattler_conda_types::VersionSpec;
use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen]
#[repr(transparent)]
#[derive(Eq, PartialEq)]
pub struct JsVersionSpec {
    inner: VersionSpec,
}

impl From<VersionSpec> for JsVersionSpec {
    fn from(value: VersionSpec) -> Self {
        JsVersionSpec { inner: value }
    }
}

impl From<JsVersionSpec> for VersionSpec {
    fn from(value: JsVersionSpec) -> Self {
        value.inner
    }
}

impl AsRef<VersionSpec> for JsVersionSpec {
    fn as_ref(&self) -> &VersionSpec {
        &self.inner
    }
}

#[wasm_bindgen]
impl JsVersionSpec {
    #[wasm_bindgen(constructor)]
    pub fn new(version_spec: &str, parse_strictness: JsParseStrictness) -> JsResult<Self> {
        let spec = VersionSpec::from_str(version_spec, parse_strictness.into())?;
        Ok(spec.into())
    }

    pub fn as_str(&self) -> String {
        format!("{}", self.as_ref())
    }

    pub fn matches(&self, version: &JsVersion) -> bool {
        self.as_ref().matches(version.as_ref())
    }
}
