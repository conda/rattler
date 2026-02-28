use rattler_conda_types::{MatchSpec, Matches, ParseMatchSpecOptions, ParseStrictness};
use std::str::FromStr;
use wasm_bindgen::prelude::wasm_bindgen;

use crate::{
    package_record::JsPackageRecord, parse_strictness::JsParseStrictness,
    version_spec::JsVersionSpec, JsResult,
};

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// A query language for conda packages.
///
/// @public
#[wasm_bindgen(js_name = "MatchSpec")]
#[repr(transparent)]
#[derive(Eq, PartialEq)]
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

        let options = ParseMatchSpecOptions::new(parse_strictness).with_exact_names_only(false);
        Ok(MatchSpec::from_str(spec, options)?.into())
    }

    /// Returns the string representation of the match spec.
    #[wasm_bindgen(js_name = "toString")]
    pub fn as_str(&self) -> String {
        format!("{}", self.inner)
    }

    /// Returns the package name (or glob/regex pattern).
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        format!("{}", self.inner.name)
    }

    /// Returns the version spec, if present.
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> Option<JsVersionSpec> {
        self.inner.version.clone().map(Into::into)
    }

    /// Returns the build string matcher, if present.
    #[wasm_bindgen(getter)]
    pub fn build(&self) -> Option<String> {
        self.inner.build.as_ref().map(|b| format!("{b}"))
    }

    /// Returns the build number spec, if present.
    #[wasm_bindgen(getter, js_name = "buildNumber")]
    pub fn build_number(&self) -> Option<String> {
        self.inner.build_number.as_ref().map(|bn| format!("{bn}"))
    }

    /// Returns the channel name, if present.
    #[wasm_bindgen(getter)]
    pub fn channel(&self) -> Option<String> {
        self.inner.channel.as_ref().map(|c| c.name().to_string())
    }

    /// Returns the channel subdirectory, if present.
    #[wasm_bindgen(getter)]
    pub fn subdir(&self) -> Option<String> {
        self.inner.subdir.clone()
    }

    /// Returns the namespace, if present.
    #[wasm_bindgen(getter)]
    pub fn namespace(&self) -> Option<String> {
        self.inner.namespace.clone()
    }

    /// Returns the specific filename, if present.
    #[wasm_bindgen(getter, js_name = "fileName")]
    pub fn file_name(&self) -> Option<String> {
        self.inner.file_name.clone()
    }

    /// Returns the URL, if present.
    #[wasm_bindgen(getter)]
    pub fn url(&self) -> Option<String> {
        self.inner.url.as_ref().map(|u| u.to_string())
    }

    /// Returns the license, if present.
    #[wasm_bindgen(getter)]
    pub fn license(&self) -> Option<String> {
        self.inner.license.clone()
    }

    /// Returns the MD5 hash as a hex string, if present.
    #[wasm_bindgen(getter)]
    pub fn md5(&self) -> Option<String> {
        self.inner.md5.as_ref().map(|h| bytes_to_hex(h.as_slice()))
    }

    /// Returns the SHA-256 hash as a hex string, if present.
    #[wasm_bindgen(getter)]
    pub fn sha256(&self) -> Option<String> {
        self.inner
            .sha256
            .as_ref()
            .map(|h| bytes_to_hex(h.as_slice()))
    }

    /// Returns the selected optional extras, if present.
    #[wasm_bindgen(getter)]
    pub fn extras(&self) -> Option<Vec<String>> {
        self.inner.extras.clone()
    }

    /// Returns the track-features, if present.
    #[wasm_bindgen(getter, js_name = "trackFeatures")]
    pub fn track_features(&self) -> Option<Vec<String>> {
        self.inner.track_features.clone()
    }

    /// Returns true if the given PackageRecord matches this spec.
    pub fn matches(
        &self,
        #[wasm_bindgen(param_description = "The package record to match against.")]
        record: &JsPackageRecord,
    ) -> bool {
        self.inner.matches(record.as_ref())
    }
}
