use pyo3::prelude::PyBytesMethods;
use pyo3::{Bound, PyErr, exceptions::PyValueError, types::PyBytes};
use rattler_digest::{Md5Hash, Sha256Hash};

pub fn sha256_from_pybytes(bytes: Bound<'_, PyBytes>) -> Result<Sha256Hash, PyErr> {
    Sha256Hash::try_from(bytes.as_bytes())
        .map_err(|_err| PyValueError::new_err("Expected a 32 byte SHA256 digest"))
}

pub fn md5_from_pybytes(bytes: Bound<'_, PyBytes>) -> Result<Md5Hash, PyErr> {
    Md5Hash::try_from(bytes.as_bytes())
        .map_err(|_err| PyValueError::new_err("Expected a 16 byte MD5 digest"))
}
