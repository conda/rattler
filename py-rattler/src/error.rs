use std::{error::Error, io};

use pyo3::{create_exception, exceptions::PyException, PyErr};
use rattler::install::TransactionError;
use rattler_conda_types::{
    ConvertSubdirError, InvalidPackageNameError, ParseArchError, ParseChannelError,
    ParseMatchSpecError, ParsePlatformError, ParseVersionError, ValidatePackageRecordsError,
    VersionBumpError, VersionExtendError,
};
use rattler_lock::{ConversionError, ParseCondaLockError};
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
    InvalidMatchSpec(#[from] ParseMatchSpecError),
    #[error(transparent)]
    InvalidPackageName(#[from] InvalidPackageNameError),
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
    ActivationScriptFormatError(std::fmt::Error),
    #[error(transparent)]
    GatewayError(#[from] GatewayError),
    #[error(transparent)]
    InstallerError(#[from] rattler::install::InstallerError),
    #[error(transparent)]
    ParseExplicitEnvironmentSpecError(
        #[from] rattler_conda_types::ParseExplicitEnvironmentSpecError,
    ),
    #[error(transparent)]
    ValidatePackageRecordsError(#[from] ValidatePackageRecordsError),
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
                InvalidVersionException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidMatchSpec(err) => {
                InvalidMatchSpecException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidPackageName(err) => {
                InvalidPackageNameException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidUrl(err) => {
                InvalidUrlException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InvalidChannel(err) => {
                InvalidChannelException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ActivationError(err) => {
                ActivationException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParsePlatformError(err) => {
                ParsePlatformException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParseArchError(err) => {
                ParseArchException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::FetchRepoDataError(err) => {
                FetchRepoDataException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::CacheDirError(err) => {
                CacheDirException::new_err(pretty_print_error(err.as_ref()))
            }
            PyRattlerError::DetectVirtualPackageError(err) => {
                DetectVirtualPackageException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::IoError(err) => IoException::new_err(pretty_print_error(&err)),
            PyRattlerError::SolverError(err) => SolverException::new_err(pretty_print_error(&err)),
            PyRattlerError::TransactionError(err) => {
                TransactionException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::LinkError(err) => LinkException::new_err(err),
            PyRattlerError::ConvertSubdirError(err) => {
                ConvertSubdirException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::VersionBumpError(err) => {
                VersionBumpException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::VersionExtendError(err) => {
                VersionExtendException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParseCondaLockError(err) => {
                ParseCondaLockException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ConversionError(err) => {
                ConversionException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::RequirementError(err) => RequirementException::new_err(err),
            PyRattlerError::EnvironmentCreationError(err) => {
                EnvironmentCreationException::new_err(err)
            }
            PyRattlerError::ExtractError(err) => {
                ExtractException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ActivationScriptFormatError(err) => {
                ActivationScriptFormatException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::GatewayError(err) => {
                GatewayException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::InstallerError(err) => {
                InstallerException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParseExplicitEnvironmentSpecError(err) => {
                ParseExplicitEnvironmentSpecException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ValidatePackageRecordsError(err) => {
                ValidatePackageRecordsException::new_err(pretty_print_error(&err))
            }
        }
    }
}

create_exception!(exceptions, InvalidVersionException, PyException);
create_exception!(exceptions, InvalidMatchSpecException, PyException);
create_exception!(exceptions, InvalidPackageNameException, PyException);
create_exception!(exceptions, InvalidUrlException, PyException);
create_exception!(exceptions, InvalidChannelException, PyException);
create_exception!(exceptions, ActivationException, PyException);
create_exception!(exceptions, ParsePlatformException, PyException);
create_exception!(exceptions, ParseArchException, PyException);
create_exception!(exceptions, FetchRepoDataException, PyException);
create_exception!(exceptions, CacheDirException, PyException);
create_exception!(exceptions, DetectVirtualPackageException, PyException);
create_exception!(exceptions, IoException, PyException);
create_exception!(exceptions, SolverException, PyException);
create_exception!(exceptions, TransactionException, PyException);
create_exception!(exceptions, LinkException, PyException);
create_exception!(exceptions, ConvertSubdirException, PyException);
create_exception!(exceptions, VersionBumpException, PyException);
create_exception!(exceptions, VersionExtendException, PyException);
create_exception!(exceptions, ParseCondaLockException, PyException);
create_exception!(exceptions, ConversionException, PyException);
create_exception!(exceptions, RequirementException, PyException);
create_exception!(exceptions, EnvironmentCreationException, PyException);
create_exception!(exceptions, ExtractException, PyException);
create_exception!(exceptions, ActivationScriptFormatException, PyException);
create_exception!(exceptions, GatewayException, PyException);
create_exception!(exceptions, InstallerException, PyException);
create_exception!(
    exceptions,
    ParseExplicitEnvironmentSpecException,
    PyException
);
create_exception!(exceptions, ValidatePackageRecordsException, PyException);
