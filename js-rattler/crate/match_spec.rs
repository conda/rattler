use rattler_conda_types::{
    MatchSpec, Matches, PackageNameMatcher, ParseMatchSpecOptions, ParseStrictness, StringMatcher,
};
use serde::Deserialize;
use std::str::FromStr;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::JsValue;

use crate::{package_record::JsPackageRecord, version_spec::JsVersionSpec, JsResult};

#[wasm_bindgen(typescript_custom_section)]
const MATCH_SPEC_OPTIONS_TS: &'static str = r#"
/**
 * Options for parsing a MatchSpec string.
 *
 * @public
 */
export interface MatchSpecOptions {
    /** When `true`, the parser rejects some ambiguous version specs. @defaultValue false */
    strict?: boolean;
    /** When `true`, only exact package names are allowed (no globs or regex). @defaultValue false */
    exactNamesOnly?: boolean;
    /** When `true`, extras syntax is enabled (e.g., `pkg[extras=[foo,bar]]`). @defaultValue false */
    experimentalExtras?: boolean;
    /** When `true`, conditionals syntax is enabled. @defaultValue false */
    experimentalConditionals?: boolean;
}
"#;

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn default_exact_names_only() -> bool {
    false
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParseOpts {
    #[serde(default)]
    strict: bool,
    #[serde(default = "default_exact_names_only")]
    exact_names_only: bool,
    #[serde(default)]
    experimental_extras: bool,
    #[serde(default)]
    experimental_conditionals: bool,
}

impl Default for ParseOpts {
    fn default() -> Self {
        ParseOpts {
            strict: false,
            exact_names_only: false,
            experimental_extras: false,
            experimental_conditionals: false,
        }
    }
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
        #[wasm_bindgen(param_description = "Parse options.")] options: Option<JsValue>,
    ) -> JsResult<Self> {
        let opts: ParseOpts = options
            .map(|v| serde_wasm_bindgen::from_value(v))
            .transpose()?
            .unwrap_or_default();

        let parse_opts = ParseMatchSpecOptions::new(if opts.strict {
            ParseStrictness::Strict
        } else {
            ParseStrictness::Lenient
        })
        .with_exact_names_only(opts.exact_names_only)
        .with_experimental_extras(opts.experimental_extras)
        .with_experimental_conditionals(opts.experimental_conditionals);

        Ok(MatchSpec::from_str(spec, parse_opts)?.into())
    }

    /// Returns the string representation of the match spec.
    #[wasm_bindgen(js_name = "toString")]
    pub fn as_str(&self) -> String {
        self.inner.to_string()
    }

    /// Returns the package name (or glob/regex pattern).
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        self.inner.name.to_string()
    }

    /// Sets the package name or glob/regex pattern.
    pub fn set_name(&mut self, name: &str) -> JsResult<()> {
        self.inner.name = name.parse::<PackageNameMatcher>()?;
        Ok(())
    }

    /// Returns the version spec, if present.
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> Option<JsVersionSpec> {
        self.inner.version.clone().map(Into::into)
    }

    /// Sets the version spec.
    #[wasm_bindgen(setter)]
    pub fn set_version(&mut self, version: Option<JsVersionSpec>) {
        self.inner.version = version.map(Into::into);
    }

    /// Returns the build string matcher, if present.
    #[wasm_bindgen(getter)]
    pub fn build(&self) -> Option<String> {
        self.inner.build.as_ref().map(|b| b.to_string())
    }

    /// Sets the build string matcher.
    pub fn set_build(&mut self, build: Option<String>) -> JsResult<()> {
        self.inner.build = build.map(|b| b.parse::<StringMatcher>()).transpose()?;
        Ok(())
    }

    /// Returns the build number spec, if present.
    #[wasm_bindgen(getter, js_name = "buildNumber")]
    pub fn build_number(&self) -> Option<String> {
        self.inner.build_number.as_ref().map(|bn| bn.to_string())
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

    /// Sets the channel subdirectory.
    #[wasm_bindgen(setter)]
    pub fn set_subdir(&mut self, subdir: Option<String>) {
        self.inner.subdir = subdir;
    }

    /// Returns the namespace, if present.
    #[wasm_bindgen(getter)]
    pub fn namespace(&self) -> Option<String> {
        self.inner.namespace.clone()
    }

    /// Sets the namespace.
    #[wasm_bindgen(setter)]
    pub fn set_namespace(&mut self, namespace: Option<String>) {
        self.inner.namespace = namespace;
    }

    /// Returns the specific filename, if present.
    #[wasm_bindgen(getter, js_name = "fileName")]
    pub fn file_name(&self) -> Option<String> {
        self.inner.file_name.clone()
    }

    /// Sets the specific filename.
    #[wasm_bindgen(setter, js_name = "fileName")]
    pub fn set_file_name(&mut self, file_name: Option<String>) {
        self.inner.file_name = file_name;
    }

    /// Returns the URL, if present.
    #[wasm_bindgen(getter)]
    pub fn url(&self) -> Option<String> {
        self.inner.url.as_ref().map(|u| u.to_string())
    }

    /// Sets the URL.
    pub fn set_url(&mut self, url: Option<String>) -> JsResult<()> {
        self.inner.url = url.map(|u| u.parse::<url::Url>()).transpose()?;
        Ok(())
    }

    /// Returns the license, if present.
    #[wasm_bindgen(getter)]
    pub fn license(&self) -> Option<String> {
        self.inner.license.clone()
    }

    /// Sets the license.
    #[wasm_bindgen(setter)]
    pub fn set_license(&mut self, license: Option<String>) {
        self.inner.license = license;
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

    /// Sets the selected optional extras.
    #[wasm_bindgen(setter)]
    pub fn set_extras(&mut self, extras: Option<Vec<String>>) {
        self.inner.extras = extras;
    }

    /// Returns the track-features, if present.
    #[wasm_bindgen(getter, js_name = "trackFeatures")]
    pub fn track_features(&self) -> Option<Vec<String>> {
        self.inner.track_features.clone()
    }

    /// Sets the track-features.
    #[wasm_bindgen(setter, js_name = "trackFeatures")]
    pub fn set_track_features(&mut self, track_features: Option<Vec<String>>) {
        self.inner.track_features = track_features;
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
