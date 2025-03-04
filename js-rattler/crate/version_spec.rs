use rattler_conda_types::{ParseStrictness, VersionSpec};
use wasm_bindgen::prelude::wasm_bindgen;

use crate::parse_strictness::JsParseStrictness;
use crate::{version::JsVersion, JsResult};

/// Represents a version specification in the conda ecosystem.
///
/// @public
#[wasm_bindgen(js_name = "VersionSpec")]
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

#[wasm_bindgen(js_class = "VersionSpec")]
impl JsVersionSpec {
    /// Constructs a new VersionSpec object from a string representation.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(param_description = "The string representation of the version spec.")]
        version_spec: &str,
        #[wasm_bindgen(param_description = "The strictness of the parser.")]
        parse_strictness: Option<JsParseStrictness>,
    ) -> JsResult<Self> {
        let parse_strictness = parse_strictness
            .map(TryFrom::try_from)
            .transpose()?
            .unwrap_or(ParseStrictness::Lenient);

        let spec = VersionSpec::from_str(version_spec, parse_strictness)?;
        Ok(spec.into())
    }

    /// Returns the string representation of the version spec.
    #[wasm_bindgen(js_name = "toString")]
    pub fn as_str(&self) -> String {
        format!("{}", self.as_ref())
    }

    /// Returns true if the version matches this version spec.
    pub fn matches(
        &self,
        #[wasm_bindgen(param_description = "The version to match")] version: &JsVersion,
    ) -> bool {
        self.as_ref().matches(version.as_ref())
    }
}
