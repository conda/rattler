use std::{error::Error, io};

use pyo3::exceptions::PyValueError;
use pyo3::PyErr;
use rattler::install::TransactionError;
use rattler_conda_types::{
    version_spec::ParseVersionSpecError, ConvertSubdirError, InvalidPackageNameError,
    PackageNameMatcherParseError, ParseArchError, ParseChannelError, ParseMatchSpecError,
    ParsePlatformError, ParseVersionError, ValidatePackageRecordsError, VersionBumpError,
    VersionExtendError,
};
use rattler_lock::{ConversionError, ParseCondaLockError};
use rattler_networking::authentication_storage::AuthenticationStorageError;
use rattler_package_streaming::ExtractError;
use rattler_repodata_gateway::{fetch::FetchRepoDataError, GatewayError};
use rattler_shell::activation::ActivationError;
use rattler_solve::SolveError;
use rattler_virtual_packages::DetectVirtualPackageError;
use thiserror::Error;

#[derive(Error, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum PyRattlerError {
    #[error(transparent)]
    InvalidVersion(#[from] ParseVersionError),
    #[error(transparent)]
    InvalidVersionSpec(#[from] ParseVersionSpecError),
    #[error(transparent)]
    InvalidMatchSpec(#[from] ParseMatchSpecError),
    #[error(transparent)]
    InvalidPackageName(#[from] InvalidPackageNameError),
    #[error(transparent)]
    PackageNameMatcherParseError(#[from] PackageNameMatcherParseError),
    #[error(transparent)]
    InvalidUrl(#[from] url::ParseError),
    #[error(transparent)]
    InvalidChannel(#[from] ParseChannelError),
    #[error(transparent)]
    ActivationError(#[from] ActivationError),
    #[error(transparent)]
    ParsePlatformError(#[from] ParsePlatformError),
    #[error(transparent)]
    ParseArchError(#[from] ParseArchError),
    #[error(transparent)]
    FetchRepoDataError(#[from] FetchRepoDataError),
    #[error(transparent)]
    CacheDirError(#[from] anyhow::Error),
    #[error(transparent)]
    DetectVirtualPackageError(#[from] DetectVirtualPackageError),
    #[error(transparent)]
    IoError(#[from] io::Error),
    #[error(transparent)]
    SolverError(#[from] SolveError),
    #[error(transparent)]
    TransactionError(#[from] TransactionError),
    #[error("{0}")]
    LinkError(String),
    #[error(transparent)]
    ConvertSubdirError(#[from] ConvertSubdirError),
    #[error(transparent)]
    VersionBumpError(#[from] VersionBumpError),
    #[error(transparent)]
    VersionExtendError(#[from] VersionExtendError),
    #[error(transparent)]
    ParseCondaLockError(#[from] ParseCondaLockError),
    #[error(transparent)]
    ConversionError(#[from] ConversionError),
    #[error("{0}")]
    RequirementError(String),
    #[error("{0}")]
    EnvironmentCreationError(String),
    #[error(transparent)]
    ExtractError(#[from] ExtractError),
    #[error(transparent)]
    ShellError(#[from] rattler_shell::shell::ShellError),
    #[error(transparent)]
    GatewayError(#[from] GatewayError),
    #[error(transparent)]
    InstallerError(#[from] rattler::install::InstallerError),
    #[error(transparent)]
    ParseExplicitEnvironmentSpecError(
        #[from] rattler_conda_types::ParseExplicitEnvironmentSpecError,
    ),
    #[error(transparent)]
    ValidatePackageRecordsError(#[from] Box<ValidatePackageRecordsError>),
    #[error(transparent)]
    AuthenticationStorageError(#[from] AuthenticationStorageError),
    #[error(transparent)]
    MatchSpecUrlError(#[from] rattler_conda_types::MatchSpecUrlError),
    #[error(transparent)]
    InvalidHeaderNameError(#[from] reqwest::header::InvalidHeaderName),
    #[error(transparent)]
    InvalidHeaderValueError(#[from] reqwest::header::InvalidHeaderValue),
    #[error(transparent)]
    FromSdkError(#[from] rattler_s3::FromSDKError),
}

fn pretty_print_error(mut err: &dyn Error) -> String {
    let mut result = err.to_string();
    while let Some(source) = err.source() {
        result.push_str(&format!("\nCaused by: {source}"));
        err = source;
    }
    result
}

impl From<PyRattlerError> for PyErr {
    fn from(value: PyRattlerError) -> Self {
        match value {
            PyRattlerError::InvalidVersion(err) => {
                crate::exceptions::InvalidVersionError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidVersionSpec(err) => {
                crate::exceptions::InvalidVersionSpecError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidMatchSpec(err) => {
                crate::exceptions::InvalidMatchSpecError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidPackageName(err) => {
                crate::exceptions::InvalidPackageNameError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::PackageNameMatcherParseError(err) => {
                crate::exceptions::PackageNameMatcherParseError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidUrl(err) => {
                crate::exceptions::InvalidUrlError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidChannel(err) => {
                crate::exceptions::InvalidChannelError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ActivationError(err) => {
                crate::exceptions::ActivationError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParsePlatformError(err) => {
                crate::exceptions::ParsePlatformError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParseArchError(err) => {
                crate::exceptions::ParseArchError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::FetchRepoDataError(err) => {
                crate::exceptions::FetchRepoDataError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::CacheDirError(err) => {
                crate::exceptions::CacheDirError::new_err(pretty_print_error(err.as_ref()))
            }
            PyRattlerError::DetectVirtualPackageError(err) => {
                crate::exceptions::DetectVirtualPackageError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::IoError(err) => {
                crate::exceptions::IoError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::SolverError(err) => {
                crate::exceptions::SolverError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::TransactionError(err) => {
                crate::exceptions::TransactionError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::LinkError(err) => crate::exceptions::LinkError::new_err(err),
            PyRattlerError::ConvertSubdirError(err) => {
                crate::exceptions::ConvertSubdirError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::VersionBumpError(err) => {
                crate::exceptions::VersionBumpError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::VersionExtendError(err) => {
                crate::exceptions::VersionExtendError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParseCondaLockError(err) => {
                crate::exceptions::ParseCondaLockError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ConversionError(err) => {
                crate::exceptions::ConversionError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::RequirementError(err) => {
                crate::exceptions::RequirementError::new_err(err)
            }
            PyRattlerError::EnvironmentCreationError(err) => {
                crate::exceptions::EnvironmentCreationError::new_err(err)
            }
            PyRattlerError::ExtractError(err) => {
                crate::exceptions::ExtractError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::GatewayError(err) => {
                crate::exceptions::GatewayError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InstallerError(err) => {
                crate::exceptions::InstallerError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParseExplicitEnvironmentSpecError(err) => {
                crate::exceptions::ParseExplicitEnvironmentSpecError::new_err(pretty_print_error(
                    &err,
                ))
            }
            PyRattlerError::ValidatePackageRecordsError(err) => {
                crate::exceptions::ValidatePackageRecordsError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::AuthenticationStorageError(err) => {
                crate::exceptions::AuthenticationStorageError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ShellError(err) => {
                crate::exceptions::ShellError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::MatchSpecUrlError(err) => {
                crate::exceptions::InvalidMatchSpecError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidHeaderNameError(err) => {
                crate::exceptions::InvalidHeaderNameError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidHeaderValueError(err) => {
                crate::exceptions::InvalidHeaderValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::FromSdkError(err) => PyValueError::new_err(pretty_print_error(&err)),
        }
    }
}

// No exception definitions here, they are in exceptions.rs
