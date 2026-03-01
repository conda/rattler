use rattler_conda_types::version_spec::ParseVersionSpecError;
use rattler_conda_types::{
    InvalidPackageNameError, PackageNameMatcherParseError, ParseBuildNumberSpecError,
    ParseChannelError, ParseMatchSpecError, ParsePlatformError, ParseVersionError,
    StringMatcherParseError, VersionBumpError, VersionExtendError,
};
use rattler_repodata_gateway::GatewayError;
use rattler_solve::SolveError;
use thiserror::Error;
use url::ParseError as UrlParseError;
use wasm_bindgen::JsValue;

#[derive(Debug, Error)]
pub enum JsError {
    #[error(transparent)]
    InvalidVersion(#[from] ParseVersionError),
    #[error(transparent)]
    VersionExtendError(#[from] VersionExtendError),
    #[error(transparent)]
    VersionBumpError(#[from] VersionBumpError),
    #[error(transparent)]
    ParseVersionSpecError(#[from] ParseVersionSpecError),
    #[error(transparent)]
    ParseChannel(#[from] ParseChannelError),
    #[error(transparent)]
    ParsePlatform(#[from] ParsePlatformError),
    #[error(transparent)]
    ParseMatchSpec(#[from] ParseMatchSpecError),
    #[error(transparent)]
    GatewayError(#[from] GatewayError),
    #[error(transparent)]
    SolveError(#[from] SolveError),
    #[error(transparent)]
    Serde(#[from] serde_wasm_bindgen::Error),
    #[error(transparent)]
    PackageNameError(#[from] InvalidPackageNameError),
    #[error(transparent)]
    ParseBuildNumberSpecError(#[from] ParseBuildNumberSpecError),
    #[error(transparent)]
    PackageNameMatcherParseError(#[from] PackageNameMatcherParseError),
    #[error(transparent)]
    StringMatcherParseError(#[from] StringMatcherParseError),
    #[error(transparent)]
    UrlParseError(#[from] UrlParseError),
    #[error("{0} is not a valid hex encoded MD5 hash")]
    InvalidHexMd5(String),
    #[error("{0} is not a valid hex encoded SHA256 hash")]
    InvalidHexSha256(String),
}

pub type JsResult<T> = Result<T, JsError>;

impl From<JsError> for JsValue {
    fn from(error: JsError) -> Self {
        match error {
            JsError::Serde(error) => error.into(),
            error => JsValue::from_str(&error.to_string()),
        }
    }
}
