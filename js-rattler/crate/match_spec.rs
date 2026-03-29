use rattler_conda_types::{MatchSpec, ParseStrictness};
use wasm_bindgen::prelude::wasm_bindgen;

use crate::parse_strictness::JsParseStrictness;
use crate::JsResult;

/// Represents a match specification in the conda ecosystem.
/// A match spec defines a query for matching conda packages by name, version,
/// build string, channel, and other attributes.
///
/// @public
#[wasm_bindgen(js_name = "MatchSpec")]
#[repr(transparent)]
pub struct JsMatchSpec {
    inner: MatchSpec,
}

impl From<MatchSpec> for JsMatchSpec {
    fn from(value: MatchSpec) -> Self {
        JsMatchSpec { inner: value }
    }
}

impl From<JsMatchSpec> for MatchSpec {
    fn from(value: JsMatchSpec) -> Self {
        value.inner
    }
}

impl AsRef<MatchSpec> for JsMatchSpec {
    fn as_ref(&self) -> &MatchSpec {
        &self.inner
    }
}

#[wasm_bindgen(js_class = "MatchSpec")]
impl JsMatchSpec {
    /// Constructs a new MatchSpec from a string representation.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(param_description = "The string representation of the match spec.")]
        spec: &str,
        #[wasm_bindgen(param_description = "The strictness of the parser.")]
        parse_strictness: Option<JsParseStrictness>,
    ) -> JsResult<Self> {
        let parse_strictness = parse_strictness
            .map(TryFrom::try_from)
            .transpose()?
            .unwrap_or(ParseStrictness::Lenient);

        let match_spec = MatchSpec::from_str(spec, parse_strictness)?;
        Ok(match_spec.into())
    }

    /// Returns the string representation of the match spec.
    #[wasm_bindgen(js_name = "toString")]
    pub fn as_str(&self) -> String {
        format!("{}", self.as_ref())
    }

    /// Returns the name of the package, or `undefined` if the name is a glob/regex pattern.
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> Option<String> {
        match &self.inner.name {
            rattler_conda_types::PackageNameMatcher::Exact(name) => Some(name.as_normalized().to_string()),
            _ => None,
        }
    }

    /// Returns the version spec as a string, or `undefined` if not specified.
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> Option<String> {
        self.inner.version.as_ref().map(|v| v.to_string())
    }

    /// Returns the build string matcher, or `undefined` if not specified.
    #[wasm_bindgen(getter)]
    pub fn build(&self) -> Option<String> {
        self.inner.build.as_ref().map(|b| b.to_string())
    }

    /// Returns the channel name, or `undefined` if not specified.
    #[wasm_bindgen(getter)]
    pub fn channel(&self) -> Option<String> {
        self.inner.channel.as_ref().map(|c| c.name().to_string())
    }

    /// Returns the subdir, or `undefined` if not specified.
    #[wasm_bindgen(getter)]
    pub fn subdir(&self) -> Option<String> {
        self.inner.subdir.clone()
    }

    /// Returns the namespace, or `undefined` if not specified.
    #[wasm_bindgen(getter)]
    pub fn namespace(&self) -> Option<String> {
        self.inner.namespace.clone()
    }
}
