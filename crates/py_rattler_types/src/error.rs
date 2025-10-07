use std::error::Error;

use pyo3::{create_exception, exceptions::PyException, PyErr};
use rattler_conda_types::{ParseArchError, ParsePlatformError};
use thiserror::Error;

#[derive(Error, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum PyRattlerError {
    #[error(transparent)]
    ParsePlatformError(#[from] ParsePlatformError),
    #[error(transparent)]
    ParseArchError(#[from] ParseArchError),
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
            PyRattlerError::ParsePlatformError(err) => {
                ParsePlatformException::new_err(pretty_print_error(&err))
            }
            PyRattlerError::ParseArchError(err) => {
                ParseArchException::new_err(pretty_print_error(&err))
            }
        }
    }
}

create_exception!(exceptions, ParsePlatformException, PyException);
create_exception!(exceptions, ParseArchException, PyException);
