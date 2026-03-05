use crate::error::JsResult;
use crate::package_name::JsPackageName;
use crate::package_record::JsPackageRecord;
use crate::parse_strictness::JsParseStrictness;
use crate::version_spec::JsVersionSpec;
use rattler_conda_types::{MatchSpec, ParseStrictness};
use std::str::FromStr;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const MATCH_SPEC_D_TS: &'static str = r#"
export interface MatchSpecOptions {
    name?: string;
    version?: string;
    build?: string;
    buildNumber?: string;
    fileName?: string;
    channel?: string;
    subdir?: string;
    namespace?: string;
    md5?: string;
    sha256?: string;
    url?: string;
    license?: string;
    trackFeatures?: string[];
}
"#;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "MatchSpecOptions")]
    pub type JsMatchSpecOptions;
}

/// Represents a build number specification in a conda match spec.
///
/// @public
#[wasm_bindgen(js_name = "BuildNumberSpec")]
pub struct JsBuildNumberSpec {
    inner: rattler_conda_types::build_spec::BuildNumberSpec,
}

impl From<rattler_conda_types::build_spec::BuildNumberSpec> for JsBuildNumberSpec {
    fn from(inner: rattler_conda_types::build_spec::BuildNumberSpec) -> Self {
        Self { inner }
    }
}

impl From<JsBuildNumberSpec> for rattler_conda_types::build_spec::BuildNumberSpec {
    fn from(js: JsBuildNumberSpec) -> Self {
        js.inner
    }
}

#[wasm_bindgen(js_class = "BuildNumberSpec")]
impl JsBuildNumberSpec {
    /// Parses a BuildNumberSpec from a string.
    #[wasm_bindgen(constructor)]
    pub fn new(spec: &str) -> JsResult<JsBuildNumberSpec> {
        Ok(rattler_conda_types::build_spec::BuildNumberSpec::from_str(spec)?.into())
    }

    /// Returns the string representation of the build number spec.
    #[wasm_bindgen(js_name = "toString")]
    pub fn to_string(&self) -> String {
        self.inner.to_string()
    }
}

/// Represents a conda match specification.
///
/// @public
#[wasm_bindgen(js_name = "MatchSpec")]
pub struct JsMatchSpec {
    inner: MatchSpec,
}

impl From<MatchSpec> for JsMatchSpec {
    fn from(inner: MatchSpec) -> Self {
        Self { inner }
    }
}

impl From<JsMatchSpec> for MatchSpec {
    fn from(js_match_spec: JsMatchSpec) -> Self {
        js_match_spec.inner
    }
}

#[wasm_bindgen(js_class = "MatchSpec")]
impl JsMatchSpec {
    /// Parses a MatchSpec from a string.
    #[wasm_bindgen(constructor)]
    pub fn new(spec: &str, strictness: Option<JsParseStrictness>) -> JsResult<JsMatchSpec> {
        let strictness = strictness
            .map(ParseStrictness::try_from)
            .transpose()?
            .unwrap_or(ParseStrictness::Lenient);
        Ok(MatchSpec::from_str(spec, strictness)?.into())
    }

    /// Constructs a MatchSpec from an options object.
    #[wasm_bindgen(js_name = "fromOptions")]
    pub fn from_options(options: JsMatchSpecOptions) -> JsResult<JsMatchSpec> {
        let options: serde_json::Value = serde_wasm_bindgen::from_value(options.into())?;
        let match_spec: MatchSpec = serde_json::from_value(options).map_err(crate::error::JsError::from)?;
        Ok(match_spec.into())
    }

    /// Returns the package name of the match specification.
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> Option<JsPackageName> {
        self.inner.name.as_exact().map(|n| n.as_source().into())
    }

    #[wasm_bindgen(setter)]
    pub fn set_name(&mut self, name: Option<String>) -> JsResult<()> {
        self.inner.name = name
            .map(|n| rattler_conda_types::match_spec::package_name_matcher::PackageNameMatcher::from_str(&n))
            .transpose()?
            .unwrap_or(rattler_conda_types::match_spec::package_name_matcher::PackageNameMatcher::from_str("*").unwrap());
        Ok(())
    }

    /// Returns the version specification of the match specification.
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> Option<JsVersionSpec> {
        self.inner.version.clone().map(Into::into)
    }

    /// Returns the build string of the match specification.
    #[wasm_bindgen(getter)]
    pub fn build(&self) -> Option<String> {
        self.inner.build.as_ref().map(|b| b.to_string())
    }

    #[wasm_bindgen(setter)]
    pub fn set_build(&mut self, build: Option<String>) -> JsResult<()> {
        self.inner.build = build
            .map(|b| rattler_conda_types::match_spec::matcher::StringMatcher::from_str(&b))
            .transpose()?;
        Ok(())
    }

    /// Returns the build number of the match specification.
    #[wasm_bindgen(getter, js_name = "buildNumber")]
    pub fn build_number(&self) -> Option<JsBuildNumberSpec> {
        self.inner.build_number.clone().map(Into::into)
    }

    #[wasm_bindgen(setter, js_name = "buildNumber")]
    pub fn set_build_number(&mut self, build_number: Option<JsBuildNumberSpec>) {
        self.inner.build_number = build_number.map(Into::into);
    }

    /// Returns the file name of the match specification.
    #[wasm_bindgen(getter, js_name = "fileName")]
    pub fn file_name(&self) -> Option<String> {
        self.inner.file_name.clone()
    }

    #[wasm_bindgen(setter, js_name = "fileName")]
    pub fn set_file_name(&mut self, file_name: Option<String>) {
        self.inner.file_name = file_name;
    }

    /// Returns the channel of the match specification.

    /// Returns the channel of the match specification.
    #[wasm_bindgen(getter)]
    pub fn channel(&self) -> Option<String> {
        self.inner.channel.as_ref().map(|c| c.name())
    }

    #[wasm_bindgen(setter)]
    pub fn set_channel(&mut self, channel: Option<String>) -> JsResult<()> {
        self.inner.channel = channel
            .map(|c| {
                let config = rattler_conda_types::ChannelConfig::default_with_root_dir(
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")),
                );
                rattler_conda_types::Channel::from_str(&c, &config).map(std::sync::Arc::new)
            })
            .transpose()?;
        Ok(())
    }

    /// Returns the subdir of the match specification.
    #[wasm_bindgen(getter)]
    pub fn subdir(&self) -> Option<String> {
        self.inner.subdir.clone()
    }

    #[wasm_bindgen(setter)]
    pub fn set_subdir(&mut self, subdir: Option<String>) {
        self.inner.subdir = subdir;
    }

    /// Returns the namespace of the match specification.
    #[wasm_bindgen(getter)]
    pub fn namespace(&self) -> Option<String> {
        self.inner.namespace.clone()
    }

    #[wasm_bindgen(setter)]
    pub fn set_namespace(&mut self, namespace: Option<String>) {
        self.inner.namespace = namespace;
    }

    /// Returns the md5 hash of the match specification.
    #[wasm_bindgen(getter)]
    pub fn md5(&self) -> Option<String> {
        self.inner.md5.as_ref().map(|h| format!("{:x}", h))
    }

    #[wasm_bindgen(setter)]
    pub fn set_md5(&mut self, md5: Option<String>) -> JsResult<()> {
        self.inner.md5 = md5
            .map(|h| {
                rattler_digest::parse_digest_from_hex::<rattler_digest::Md5>(&h)
                    .ok_or_else(|| crate::error::JsError::InvalidHexMd5(h))
            })
            .transpose()?;
        Ok(())
    }

    /// Returns the sha256 hash of the match specification.
    #[wasm_bindgen(getter)]
    pub fn sha256(&self) -> Option<String> {
        self.inner.sha256.as_ref().map(|h| format!("{:x}", h))
    }

    #[wasm_bindgen(setter)]
    pub fn set_sha256(&mut self, sha256: Option<String>) -> JsResult<()> {
        self.inner.sha256 = sha256
            .map(|h| {
                rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(&h)
                    .ok_or_else(|| crate::error::JsError::InvalidHexSha256(h))
            })
            .transpose()?;
        Ok(())
    }

    /// Returns the URL of the match specification.
    #[wasm_bindgen(getter)]
    pub fn url(&self) -> Option<String> {
        self.inner.url.as_ref().map(|u| u.to_string())
    }

    #[wasm_bindgen(setter)]
    pub fn set_url(&mut self, url: Option<String>) -> JsResult<()> {
        self.inner.url = url.map(|u| url::Url::parse(&u)).transpose()?;
        Ok(())
    }

    /// Returns the license of the match specification.
    #[wasm_bindgen(getter)]
    pub fn license(&self) -> Option<String> {
        self.inner.license.clone()
    }

    #[wasm_bindgen(setter)]
    pub fn set_license(&mut self, license: Option<String>) {
        self.inner.license = license;
    }

    /// Returns the track features of the match specification.
    #[wasm_bindgen(getter, js_name = "trackFeatures")]
    pub fn track_features(&self) -> Vec<String> {
        self.inner.track_features.clone().unwrap_or_default()
    }

    #[wasm_bindgen(setter, js_name = "trackFeatures")]
    pub fn set_track_features(&mut self, track_features: Vec<String>) {
        self.inner.track_features = if track_features.is_empty() {
            None
        } else {
            Some(track_features)
        };
    }

    /// Returns true if the given package record matches this specification.
    pub fn matches(&self, record: &JsPackageRecord) -> bool {
        self.inner.matches(record.as_ref())
    }

    /// Returns the string representation of the match specification.
    #[wasm_bindgen(js_name = "toString")]
    pub fn to_string(&self) -> String {
        self.inner.to_string()
    }
}
