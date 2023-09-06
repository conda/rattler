use pyo3::exceptions::PyException;
use pyo3::{create_exception, PyErr};
use rattler_conda_types::{
    InvalidPackageNameError, ParseChannelError, ParseMatchSpecError, ParseVersionError,
};
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
        }
    }
}

create_exception!(exceptions, InvalidVersionException, PyException);
create_exception!(exceptions, InvalidMatchSpecException, PyException);
create_exception!(exceptions, InvalidPackageNameException, PyException);
create_exception!(exceptions, InvalidUrlException, PyException);
create_exception!(exceptions, InvalidChannelException, PyException);
