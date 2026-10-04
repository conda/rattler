use rattler_conda_types::version_spec::ParseVersionSpecError;
use rattler_conda_types::{
    InvalidPackageNameError, ParseChannelError, ParseMatchSpecError, ParseSubdirError,
    ParseVersionError, VersionBumpError, VersionExtendError,
};
use rattler_package_streaming::ExtractError;
use rattler_repodata_gateway::{GatewayError, fetch::FetchRepoDataError};
use rattler_solve::SolveError;
use thiserror::Error;
use wasm_bindgen::{JsCast, JsValue};

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
    ParsePlatform(#[from] ParseSubdirError),
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
    #[error("{0} is not a valid hex encoded MD5 hash")]
    InvalidHexMd5(String),
    #[error("{0} is not a valid hex encoded SHA256 hash")]
    InvalidHexSha256(String),
    #[error(transparent)]
    ParseUrl(#[from] url::ParseError),
    #[error(transparent)]
    Extract(#[from] ExtractError),
    #[error("{0}")]
    Fetch(String),
    #[error("{0:?} is not an archive section: expected \"info\" or \"pkg\"")]
    InvalidSection(String),
    #[error("{0} is not a valid package archive file name")]
    InvalidFileName(String),
    #[error("{0} is not a valid url")]
    InvalidUrl(String),
    #[error(transparent)]
    ParseJson(#[from] serde_json::Error),
}

pub type JsResult<T> = Result<T, JsError>;

impl JsError {
    /// The stable error code exposed to JavaScript as the `code` property
    /// of the thrown error. Callers use it to classify failures without
    /// matching on message strings.
    fn code(&self) -> &'static str {
        match self {
            JsError::InvalidVersion(_) => "PARSE_VERSION",
            JsError::VersionExtendError(_) => "VERSION_EXTEND",
            JsError::VersionBumpError(_) => "VERSION_BUMP",
            JsError::ParseVersionSpecError(_) => "PARSE_VERSION_SPEC",
            JsError::ParseChannel(_) => "PARSE_CHANNEL",
            JsError::ParsePlatform(_) => "PARSE_PLATFORM",
            JsError::ParseMatchSpec(_) => "PARSE_MATCH_SPEC",
            JsError::GatewayError(error) => match error {
                GatewayError::SubdirNotFoundError(_) => "SUBDIR_NOT_FOUND",
                GatewayError::JsFetchError(_)
                | GatewayError::FetchRepoDataError(FetchRepoDataError::JsFetchError(_)) => "FETCH",
                _ => "GATEWAY",
            },
            JsError::SolveError(_) => "SOLVE",
            JsError::Serde(_) => "SERDE",
            JsError::PackageNameError(_) => "PARSE_PACKAGE_NAME",
            JsError::InvalidHexMd5(_) => "PARSE_MD5",
            JsError::InvalidHexSha256(_) => "PARSE_SHA256",
            JsError::ParseUrl(_) | JsError::InvalidUrl(_) => "PARSE_URL",
            JsError::Extract(ExtractError::UnsupportedArchiveType) => "UNSUPPORTED_ARCHIVE_TYPE",
            JsError::Extract(_) => "ARCHIVE",
            JsError::Fetch(_) => "FETCH",
            JsError::InvalidSection(_) => "INVALID_SECTION",
            JsError::InvalidFileName(_) => "PARSE_FILE_NAME",
            JsError::ParseJson(_) => "PARSE_JSON",
        }
    }
}

impl From<JsError> for JsValue {
    fn from(error: JsError) -> Self {
        let code = error.code();
        let js_error: js_sys::Error = match error {
            // The serde error already carries a useful JavaScript error
            // object, only the code needs to be attached.
            JsError::Serde(error) => JsValue::from(error).unchecked_into(),
            error => js_sys::Error::new(&error.to_string()),
        };
        let _ = js_sys::Reflect::set(
            &js_error,
            &JsValue::from_str("code"),
            &JsValue::from_str(code),
        );
        js_error.into()
    }
}
