use std::io;

use pyo3::exceptions::PyException;
use pyo3::{create_exception, PyErr};
use rattler::install::TransactionError;
use rattler_conda_types::{
    InvalidPackageNameError, ParseArchError, ParseChannelError, ParseMatchSpecError,
    ParsePlatformError, ParseVersionError,
};
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
