
use rattler_conda_types::{
    MatchSpec, Matches, PackageRecord, ParseMatchSpecOptions, ParseStrictness, RepoDataRecord,
    RepodataRevision,
};
use wasm_bindgen::{JsValue, prelude::wasm_bindgen};

use crate::{
    JsResult, package_name::JsPackageName, package_record::JsPackageRecord,
    parse_strictness::JsParseStrictness, version_spec::JsVersionSpec,
};

/// A match spec selects packages from a channel: a package name, optionally
/// narrowed by a version spec, a build string, a channel, a subdir and other
/// fields, in the syntax conda and pixi use (`numpy >=2`, `python 3.13.*`,
/// `conda-forge::pytorch[build=cuda*]`).
///
/// Parsing accepts the repodata v3 syntax as well: extras
/// (`python[extras=[foo]]`), conditionals (`python[when="numpy"]`) and flags
/// (`python[flags=[cuda]]`). The package name must be exact: a glob or regex
/// name (`py*`) is rejected, as a gateway query could not look it up.
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

/// The options the bindings parse match specs with: the given strictness,
/// exact package names only, and the full repodata v3 syntax. This is also
/// what the gateway parses the specs of a query with, so a spec that parses
/// here is accepted there.
pub(crate) fn parse_options(strictness: ParseStrictness) -> ParseMatchSpecOptions {
    ParseMatchSpecOptions::new(strictness).with_repodata_revision(RepodataRevision::V3)
}

#[wasm_bindgen(js_class = "MatchSpec")]
impl JsMatchSpec {
    /// Constructs a new `MatchSpec` object from a string representation.
    ///
    /// Throws an error with code `PARSE_MATCH_SPEC` when the string is not a
    /// valid match spec.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(param_description = "The string representation of the match spec.")]
        spec: &str,
        #[wasm_bindgen(param_description = "The strictness of the parser. Defaults to `lenient`.")]
        parse_strictness: Option<JsParseStrictness>,
    ) -> JsResult<Self> {
        let parse_strictness = parse_strictness
            .map(TryFrom::try_from)
            .transpose()?
            .unwrap_or(ParseStrictness::Lenient);

        let spec = MatchSpec::from_str(spec, parse_options(parse_strictness))?;
        Ok(spec.into())
    }

    /// Returns the string representation of the match spec.
    ///
    /// An attempt is made to return the spec in the same format as the input
    /// string, but this is not guaranteed.
    #[wasm_bindgen(js_name = "toString")]
    pub fn as_str(&self) -> String {
        format!("{}", self.as_ref())
    }

    /// The name of the package this spec selects, normalized to lower case.
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> JsPackageName {
        let name = match self.as_ref().name.as_exact() {
            Some(name) => name.as_normalized().to_owned(),
            // The parser only produces exact names; see `parse_options`.
            None => self.as_ref().name.to_string(),
        };
        JsValue::from(name).into()
    }

    /// The version spec of the package (e.g. `>=1.2.3`), if any.
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> Option<JsVersionSpec> {
        self.as_ref().version.clone().map(Into::into)
    }

    /// The build string matcher of the package (e.g. `py37_0`, `py*`), if any.
    #[wasm_bindgen(getter)]
    pub fn build(&self) -> Option<String> {
        self.as_ref().build.as_ref().map(ToString::to_string)
    }

    /// The build number spec of the package (e.g. `>=3`), if any.
    #[wasm_bindgen(getter, js_name = "buildNumber")]
    pub fn build_number(&self) -> Option<String> {
        self.as_ref().build_number.as_ref().map(ToString::to_string)
    }

    /// The exact filename of the package archive, if any.
    #[wasm_bindgen(getter, js_name = "fileName")]
    pub fn file_name(&self) -> Option<String> {
        self.as_ref().file_name.clone()
    }

    /// The extras selected on the package (`python[extras=[foo]]`), if any.
    #[wasm_bindgen(getter)]
    pub fn extras(&self) -> Option<Vec<String>> {
        self.as_ref().extras.clone()
    }

    /// The flag matchers of the package (`python[flags=[cuda]]`), if any.
    #[wasm_bindgen(getter)]
    pub fn flags(&self) -> Option<Vec<String>> {
        self.as_ref()
            .flags
            .as_ref()
            .map(|flags| flags.iter().map(ToString::to_string).collect())
    }

    /// The base URL of the channel the package must come from, if any.
    #[wasm_bindgen(getter)]
    pub fn channel(&self) -> Option<String> {
        self.as_ref()
            .channel
            .as_ref()
            .map(|channel| channel.base_url.to_string())
    }

    /// The subdir of the channel the package must come from, if any.
    #[wasm_bindgen(getter)]
    pub fn subdir(&self) -> Option<String> {
        self.as_ref().subdir.clone()
    }

    /// The namespace of the package, if any. Namespaces are not used yet.
    #[wasm_bindgen(getter)]
    pub fn namespace(&self) -> Option<String> {
        self.as_ref().namespace.clone()
    }

    /// The hex encoded MD5 hash the package must have, if any.
    #[wasm_bindgen(getter)]
    pub fn md5(&self) -> Option<String> {
        self.as_ref().md5.as_ref().map(hex::encode)
    }

    /// The hex encoded SHA256 hash the package must have, if any.
    #[wasm_bindgen(getter)]
    pub fn sha256(&self) -> Option<String> {
        self.as_ref().sha256.as_ref().map(hex::encode)
    }

    /// The URL the package must be downloaded from, if any.
    #[wasm_bindgen(getter)]
    pub fn url(&self) -> Option<String> {
        self.as_ref().url.as_ref().map(ToString::to_string)
    }

    /// The license the package must have, if any.
    #[wasm_bindgen(getter)]
    pub fn license(&self) -> Option<String> {
        self.as_ref().license.clone()
    }

    /// The license family the package must have (e.g. `MIT`, `BSD`), if any.
    #[wasm_bindgen(getter, js_name = "licenseFamily")]
    pub fn license_family(&self) -> Option<String> {
        self.as_ref().license_family.clone()
    }

    /// The condition under which the spec applies
    /// (`python[when="numpy >=2"]`), if any.
    #[wasm_bindgen(getter)]
    pub fn condition(&self) -> Option<String> {
        self.as_ref().condition.as_ref().map(ToString::to_string)
    }

    /// The track features the package must have, if any.
    #[wasm_bindgen(getter, js_name = "trackFeatures")]
    pub fn track_features(&self) -> Option<Vec<String>> {
        self.as_ref().track_features.clone()
    }

    /// Returns `true` if the record satisfies this spec.
    ///
    /// A `PackageRecord` carries no `url`, so a spec selecting a package by
    /// its URL cannot be satisfied here; pass the plain record a gateway
    /// query returned to {@link MatchSpec.matchesJson} for that.
    pub fn matches(
        &self,
        #[wasm_bindgen(param_description = "The record to match")] record: &JsPackageRecord,
    ) -> bool {
        self.as_ref().matches(record.as_ref())
    }

    /// Returns `true` if the record, given as the plain JSON object a gateway
    /// query returns or as it appears in `repodata.json`, satisfies this spec.
    ///
    /// A record carrying `url` and `fn` is matched as a repodata record, so a
    /// spec selecting a package by URL is honored. Throws an error with code
    /// `SERDE` when the object is not a valid record.
    #[wasm_bindgen(js_name = "matchesJson")]
    pub fn matches_json(
        &self,
        #[wasm_bindgen(
            param_description = "The record to match",
            unchecked_param_type = "PackageRecordJson & { fn?: string; url?: string; channel?: string | null }"
        )]
        record: JsValue,
    ) -> JsResult<bool> {
        let has_url = js_sys::Reflect::has(&record, &JsValue::from_str("url")).unwrap_or(false);
        if has_url {
            let record: RepoDataRecord = serde_wasm_bindgen::from_value(record)?;
            Ok(self.as_ref().matches(&record))
        } else {
            let record: PackageRecord = serde_wasm_bindgen::from_value(record)?;
            Ok(self.as_ref().matches(&record))
        }
    }
}
