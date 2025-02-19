use rattler_conda_types::version_spec::ParseVersionSpecError;
use rattler_conda_types::{ParseVersionError, VersionBumpError, VersionExtendError};
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
}

pub type JsResult<T> = Result<T, JsError>;

impl From<JsError> for JsValue {
    fn from(error: JsError) -> Self {
        match error {
            JsError::InvalidVersion(error) => JsValue::from_str(&error.to_string()),
            JsError::VersionExtendError(error) => JsValue::from_str(&error.to_string()),
            JsError::VersionBumpError(error) => JsValue::from_str(&error.to_string()),
            JsError::ParseVersionSpecError(error) => JsValue::from_str(&error.to_string()),
        }
    }
}
