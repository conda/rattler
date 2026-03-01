use rattler_conda_types::{BuildNumber, BuildNumberSpec};
use wasm_bindgen::prelude::wasm_bindgen;

use crate::JsResult;

/// Represents a build number constraint, e.g. `>=3` or `==7`.
///
/// @public
#[wasm_bindgen(js_name = "BuildNumberSpec")]
#[repr(transparent)]
#[derive(Eq, PartialEq)]
pub struct JsBuildNumberSpec {
    inner: BuildNumberSpec,
}

impl From<BuildNumberSpec> for JsBuildNumberSpec {
    fn from(value: BuildNumberSpec) -> Self {
        JsBuildNumberSpec { inner: value }
    }
}

impl From<JsBuildNumberSpec> for BuildNumberSpec {
    fn from(value: JsBuildNumberSpec) -> Self {
        value.inner
    }
}

impl AsRef<BuildNumberSpec> for JsBuildNumberSpec {
    fn as_ref(&self) -> &BuildNumberSpec {
        &self.inner
    }
}

#[wasm_bindgen(js_class = "BuildNumberSpec")]
impl JsBuildNumberSpec {
    /// Parses a build number spec string, e.g. `">=3"` or `"==7"`.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(param_description = "The string representation of the build number spec.")]
        spec: &str,
    ) -> JsResult<Self> {
        Ok(spec.parse::<BuildNumberSpec>()?.into())
    }

    /// Returns true if the given build number satisfies this constraint.
    pub fn matches(
        &self,
        #[wasm_bindgen(param_description = "The build number to test.")] build_number: BuildNumber,
    ) -> bool {
        self.inner.matches(&build_number)
    }

    /// Returns the string representation, e.g. `">=3"`.
    #[wasm_bindgen(js_name = "toString")]
    pub fn as_str(&self) -> String {
        self.inner.to_string()
    }
}
