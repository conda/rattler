use std::io;

use pyo3::exceptions::PyException;
use pyo3::{create_exception, PyErr};
use rattler::install::TransactionError;
use rattler_conda_types::{
    ConvertSubdirError, InvalidPackageNameError, ParseArchError, ParseChannelError,
    ParseMatchSpecError, ParsePlatformError, ParseVersionError, VersionBumpError,
};
use rattler_lock::{ConversionError, ParseCondaLockError};
use rattler_package_streaming::ExtractError;
use rattler_repodata_gateway::fetch::FetchRepoDataError;
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
    ConverSubdirError(#[from] ConvertSubdirError),
    #[error(transparent)]
    VersionBumpError(#[from] VersionBumpError),
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
}

impl From<PyRattlerError> for PyErr {
    fn from(value: PyRattlerError) -> Self {
        match value {
            PyRattlerError::InvalidVersion(err) => {
                InvalidVersionException::new_err(err.to_string())
            }
            PyRattlerError::InvalidMatchSpec(err) => {
                InvalidMatchSpecException::new_err(err.to_string())
            }
            PyRattlerError::InvalidPackageName(err) => {
                InvalidPackageNameException::new_err(err.to_string())
            }
            PyRattlerError::InvalidUrl(err) => InvalidUrlException::new_err(err.to_string()),
            PyRattlerError::InvalidChannel(err) => {
                InvalidChannelException::new_err(err.to_string())
            }
            PyRattlerError::ActivationError(err) => ActivationException::new_err(err.to_string()),
            PyRattlerError::ParsePlatformError(err) => {
                ParsePlatformException::new_err(err.to_string())
            }
            PyRattlerError::ParseArchError(err) => ParseArchException::new_err(err.to_string()),
            PyRattlerError::FetchRepoDataError(err) => {
                FetchRepoDataException::new_err(err.to_string())
            }
            PyRattlerError::CacheDirError(err) => CacheDirException::new_err(err.to_string()),
            PyRattlerError::DetectVirtualPackageError(err) => {
                DetectVirtualPackageException::new_err(err.to_string())
            }
            PyRattlerError::IoError(err) => IoException::new_err(err.to_string()),
            PyRattlerError::SolverError(err) => SolverException::new_err(err.to_string()),
            PyRattlerError::TransactionError(err) => TransactionException::new_err(err.to_string()),
            PyRattlerError::LinkError(err) => LinkException::new_err(err),
            PyRattlerError::ConverSubdirError(err) => {
                ConvertSubdirException::new_err(err.to_string())
            }
            PyRattlerError::VersionBumpError(err) => VersionBumpException::new_err(err.to_string()),
            PyRattlerError::ParseCondaLockError(err) => {
                ParseCondaLockException::new_err(err.to_string())
            }
            PyRattlerError::ConversionError(err) => ConversionException::new_err(err.to_string()),
            PyRattlerError::RequirementError(err) => RequirementException::new_err(err),
            PyRattlerError::EnvironmentCreationError(err) => {
                EnvironmentCreationException::new_err(err)
            }
            PyRattlerError::ExtractError(err) => ExtractException::new_err(err.to_string()),
            PyRattlerError::ActivationScriptFormatError(err) => {
                ActivationScriptFormatException::new_err(err.to_string())
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
create_exception!(exceptions, ParseCondaLockException, PyException);
create_exception!(exceptions, ConversionException, PyException);
create_exception!(exceptions, RequirementException, PyException);
create_exception!(exceptions, EnvironmentCreationException, PyException);
create_exception!(exceptions, ExtractException, PyException);
create_exception!(exceptions, ActivationScriptFormatException, PyException);
