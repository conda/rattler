use rattler_conda_types::{MatchSpec, Matches, ParseMatchSpecOptions, ParseStrictness};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

use crate::{
    JsResult, package_name::JsPackageName, package_record::JsPackageRecord,
    repo_data_record::JsRepoDataRecord, version_spec::JsVersionSpec,
};

#[wasm_bindgen(typescript_custom_section)]
const MATCH_SPEC_OPTIONS_TS: &'static str = r#"
/**
 * Options that control how a `MatchSpec` is parsed.
 *
 * @public
 */
export type MatchSpecOptions = {
    /** The strictness of the parser. Defaults to `"lenient"`. */
    strictness?: ParseStrictness;
    /**
     * Only accept exact package names. When `false`, the name may also be a
     * glob (`foo*`) or an anchored regex (`^foo.*$`). Defaults to `true`.
     */
    exactNamesOnly?: boolean;
    /** Accept the extras syntax (`foo[extras=[bar]]`). Defaults to `true`. */
    extras?: boolean;
    /** Accept the conditionals syntax (`foo[when="python >=3.6"]`). Defaults to `true`. */
    conditionals?: boolean;
    /** Accept the flags syntax (`foo[flags=[cuda]]`). Defaults to `true`. */
    flags?: boolean;
};
"#;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "MatchSpecOptions")]
    pub type JsMatchSpecOptions;
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MatchSpecOptions {
    #[serde(default)]
    strictness: Option<Strictness>,
    #[serde(default)]
    exact_names_only: Option<bool>,
    #[serde(default)]
    extras: Option<bool>,
    #[serde(default)]
    conditionals: Option<bool>,
    #[serde(default)]
    flags: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Strictness {
    Strict,
    Lenient,
}

impl From<MatchSpecOptions> for ParseMatchSpecOptions {
    fn from(value: MatchSpecOptions) -> Self {
        let strictness = match value.strictness {
            Some(Strictness::Strict) => ParseStrictness::Strict,
            Some(Strictness::Lenient) | None => ParseStrictness::Lenient,
        };
        ParseMatchSpecOptions::new(strictness)
            .with_exact_names_only(value.exact_names_only.unwrap_or(true))
            .with_extras(value.extras.unwrap_or(true))
            .with_conditionals(value.conditionals.unwrap_or(true))
            .with_flags(value.flags.unwrap_or(true))
    }
}

/// A match spec is a query language for conda packages. It selects package
/// records by name, version, build string, channel and more, for example
/// `numpy >=1.20`, `conda-forge::python 3.12.* *_cpython` or
/// `pytest[version=">=8"]`.
///
/// @public
#[wasm_bindgen(js_name = "MatchSpec")]
#[repr(transparent)]
#[derive(Clone, Eq, PartialEq)]
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
    /// Parses a match spec from its string representation.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(param_description = "The string representation of the match spec.")]
        spec: &str,
        #[wasm_bindgen(param_description = "Options that control how the spec is parsed.")]
        options: Option<JsMatchSpecOptions>,
    ) -> JsResult<Self> {
        let options: Option<MatchSpecOptions> = match options {
            Some(options) => serde_wasm_bindgen::from_value(options.into())?,
            None => None,
        };
        let options = ParseMatchSpecOptions::from(options.unwrap_or_default());
        Ok(MatchSpec::from_str(spec, options)?.into())
    }

    /// Returns the string representation of the match spec.
    #[wasm_bindgen(js_name = "toString")]
    pub fn as_str(&self) -> String {
        self.inner.to_string()
    }

    /// The name matcher as written in the spec. This is either an exact
    /// package name, a glob (`foo*`) or an anchored regex (`^foo.*$`).
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        self.inner.name.to_string()
    }

    /// The normalized package name if the spec selects a single package by
    /// its exact name, `undefined` for glob and regex names.
    #[wasm_bindgen(
        getter,
        js_name = "exactName",
        unchecked_return_type = "PackageName | undefined"
    )]
    pub fn exact_name(&self) -> Option<String> {
        self.inner
            .name
            .as_exact()
            .map(|name| name.as_normalized().to_string())
    }

    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`).
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> Option<JsVersionSpec> {
        self.inner.version.clone().map(Into::into)
    }

    /// The build string matcher of the package (e.g. `py37_0`, `py*`).
    #[wasm_bindgen(getter)]
    pub fn build(&self) -> Option<String> {
        self.inner.build.as_ref().map(ToString::to_string)
    }

    /// The build number spec of the package (e.g. `1`, `>=2`).
    #[wasm_bindgen(getter, js_name = "buildNumber")]
    pub fn build_number(&self) -> Option<String> {
        self.inner.build_number.as_ref().map(ToString::to_string)
    }

    /// The file name of the package.
    #[wasm_bindgen(getter, js_name = "fileName")]
    pub fn file_name(&self) -> Option<String> {
        self.inner.file_name.clone()
    }

    /// The selected optional dependency groups of the package.
    #[wasm_bindgen(getter)]
    pub fn extras(&self) -> Option<Vec<String>> {
        self.inner.extras.clone()
    }

    /// The package flags the selected records must carry.
    #[wasm_bindgen(getter)]
    pub fn flags(&self) -> Option<Vec<String>> {
        self.inner
            .flags
            .as_ref()
            .map(|flags| flags.iter().map(ToString::to_string).collect())
    }

    /// The base url of the channel of the package, if one was specified.
    #[wasm_bindgen(getter)]
    pub fn channel(&self) -> Option<String> {
        self.inner
            .channel
            .as_ref()
            .map(|channel| channel.base_url.to_string())
    }

    /// The subdir of the channel.
    #[wasm_bindgen(getter)]
    pub fn subdir(&self) -> Option<String> {
        self.inner.subdir.clone()
    }

    /// The namespace of the package (currently not used).
    #[wasm_bindgen(getter)]
    pub fn namespace(&self) -> Option<String> {
        self.inner.namespace.clone()
    }

    /// The hex encoded md5 hash of the package.
    #[wasm_bindgen(getter)]
    pub fn md5(&self) -> Option<String> {
        self.inner.md5.map(hex::encode)
    }

    /// The hex encoded sha256 hash of the package.
    #[wasm_bindgen(getter)]
    pub fn sha256(&self) -> Option<String> {
        self.inner.sha256.map(hex::encode)
    }

    /// The url of the package.
    #[wasm_bindgen(getter)]
    pub fn url(&self) -> Option<String> {
        self.inner.url.as_ref().map(ToString::to_string)
    }

    /// The license of the package.
    #[wasm_bindgen(getter)]
    pub fn license(&self) -> Option<String> {
        self.inner.license.clone()
    }

    /// The license family of the package.
    #[wasm_bindgen(getter, js_name = "licenseFamily")]
    pub fn license_family(&self) -> Option<String> {
        self.inner.license_family.clone()
    }

    /// The condition under which this match spec applies
    /// (e.g. `python >=3.12`).
    #[wasm_bindgen(getter)]
    pub fn condition(&self) -> Option<String> {
        self.inner.condition.as_ref().map(ToString::to_string)
    }

    /// The track features of the package.
    #[wasm_bindgen(getter, js_name = "trackFeatures")]
    pub fn track_features(&self) -> Option<Vec<String>> {
        self.inner.track_features.clone()
    }

    /// Returns `true` if the name of this spec matches the given package
    /// name.
    #[wasm_bindgen(js_name = "matchesName")]
    pub fn matches_name(
        &self,
        #[wasm_bindgen(param_description = "The package name to match")] name: JsPackageName,
    ) -> JsResult<bool> {
        let name: String = serde_wasm_bindgen::from_value(name.into())?;
        let name = rattler_conda_types::PackageName::try_from(name)?;
        Ok(self.inner.name.matches(&name))
    }

    /// Returns `true` if the package record matches this spec.
    pub fn matches(
        &self,
        #[wasm_bindgen(param_description = "The record to match")] record: &JsPackageRecord,
    ) -> bool {
        self.inner.matches(record.as_ref())
    }

    /// Returns `true` if the repodata record matches this spec. Unlike
    /// `matches`, this also compares the `url` of the spec with the url of
    /// the record.
    #[wasm_bindgen(js_name = "matchesRepoDataRecord")]
    pub fn matches_repo_data_record(
        &self,
        #[wasm_bindgen(param_description = "The record to match")] record: &JsRepoDataRecord,
    ) -> bool {
        self.inner
            .matches(AsRef::<rattler_conda_types::RepoDataRecord>::as_ref(record))
    }
}
