use std::{error::Error, io};

use pyo3::exceptions::PyValueError;
use pyo3::{create_exception, exceptions::PyException, PyErr};
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
                InvalidVersionError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidVersionSpec(err) => {
                InvalidVersionSpecError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidMatchSpec(err) => {
                InvalidMatchSpecError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidPackageName(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::PackageNameMatcherParseError(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidUrl(err) => InvalidUrlError::new_err(pretty_print_error(&err)),
            PyRattlerError::InvalidChannel(err) => {
                InvalidChannelError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ActivationError(err) => PyValueError::new_err(pretty_print_error(&err)),
            PyRattlerError::ParsePlatformError(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParseArchError(err) => PyValueError::new_err(pretty_print_error(&err)),
            PyRattlerError::FetchRepoDataError(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::CacheDirError(err) => {
                CacheDirError::new_err(pretty_print_error(err.as_ref()))
            }
            PyRattlerError::DetectVirtualPackageError(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::IoError(err) => IoError::new_err(pretty_print_error(&err)),
            PyRattlerError::SolverError(err) => SolverError::new_err(pretty_print_error(&err)),
            PyRattlerError::TransactionError(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::LinkError(err) => LinkError::new_err(err),
            PyRattlerError::ConvertSubdirError(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::VersionBumpError(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::VersionExtendError(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParseCondaLockError(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ConversionError(err) => PyValueError::new_err(pretty_print_error(&err)),
            PyRattlerError::RequirementError(err) => RequirementError::new_err(err),
            PyRattlerError::EnvironmentCreationError(err) => EnvironmentCreationError::new_err(err),
            PyRattlerError::ExtractError(err) => PyValueError::new_err(pretty_print_error(&err)),
            PyRattlerError::GatewayError(err) => PyValueError::new_err(pretty_print_error(&err)),
            PyRattlerError::InstallerError(err) => {
                InstallerError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParseExplicitEnvironmentSpecError(err) => {
                ParseExplicitEnvironmentSpecError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ValidatePackageRecordsError(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::AuthenticationStorageError(err) => {
                PyValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ShellError(err) => ShellError::new_err(pretty_print_error(&err)),
            PyRattlerError::MatchSpecUrlError(err) => {
                InvalidMatchSpecError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidHeaderNameError(err) => {
                InvalidHeaderNameError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidHeaderValueError(err) => {
                InvalidHeaderValueError::new_err(pretty_print_error(&err))
            }
            PyRattlerError::FromSdkError(err) => PyValueError::new_err(pretty_print_error(&err)),
        }
    }
}

create_exception!(exceptions, InvalidVersionError, PyException);
create_exception!(exceptions, InvalidVersionSpecError, PyException);
create_exception!(exceptions, InvalidMatchSpecError, PyException);
create_exception!(exceptions, InvalidUrlError, PyException);
create_exception!(exceptions, InvalidChannelError, PyException);
create_exception!(exceptions, CacheDirError, PyException);
create_exception!(exceptions, IoError, PyException);
create_exception!(exceptions, SolverError, PyException);
create_exception!(exceptions, LinkError, PyException);
create_exception!(exceptions, RequirementError, PyException);
create_exception!(exceptions, EnvironmentCreationError, PyException);
create_exception!(exceptions, ActivationScriptFormatError, PyException);
create_exception!(exceptions, ParseExplicitEnvironmentSpecError, PyException);
create_exception!(exceptions, InstallerError, PyException);
create_exception!(exceptions, ShellError, PyException);
create_exception!(exceptions, InvalidHeaderNameError, PyException);
create_exception!(exceptions, InvalidHeaderValueError, PyException);
