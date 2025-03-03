use rattler_conda_types::version_spec::ParseVersionSpecError;
use rattler_conda_types::{
    InvalidPackageNameError, ParseChannelError, ParseMatchSpecError, ParsePlatformError,
    ParseVersionError, VersionBumpError, VersionExtendError,
};
use rattler_repodata_gateway::GatewayError;
use rattler_solve::SolveError;
use thiserror::Error;
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
}

pub type JsResult<T> = Result<T, JsError>;

impl From<JsError> for JsValue {
    fn from(error: JsError) -> Self {
        match error {
            JsError::InvalidVersion(error) => JsValue::from_str(&error.to_string()),
            JsError::VersionExtendError(error) => JsValue::from_str(&error.to_string()),
            JsError::VersionBumpError(error) => JsValue::from_str(&error.to_string()),
            JsError::ParseVersionSpecError(error) => JsValue::from_str(&error.to_string()),
            JsError::ParseChannel(error) => JsValue::from_str(&error.to_string()),
            JsError::ParsePlatform(error) => JsValue::from_str(&error.to_string()),
            JsError::ParseMatchSpec(error) => JsValue::from_str(&error.to_string()),
            JsError::GatewayError(error) => JsValue::from_str(&error.to_string()),
            JsError::SolveError(error) => JsValue::from_str(&error.to_string()),
            JsError::PackageNameError(error) => JsValue::from_str(&error.to_string()),
            JsError::Serde(error) => error.into(),
        }
    }
}
