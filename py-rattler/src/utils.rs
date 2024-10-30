use pyo3::{exceptions::PyValueError, types::PyBytes, PyErr};
use rattler_digest::{Md5Hash, Sha256Hash};

pub fn sha256_from_pybytes(bytes: &PyBytes) -> Result<Sha256Hash, PyErr> {
    if bytes.as_bytes().len() != 32 {
        return Err(PyValueError::new_err("Expected a 32 byte SHA256 digest"));
    }
    let digest = Sha256Hash::from_slice(bytes.as_bytes());
    Ok(digest.clone())
}

pub fn md5_from_pybytes(bytes: &PyBytes) -> Result<Md5Hash, PyErr> {
    if bytes.as_bytes().len() != 16 {
        return Err(PyValueError::new_err("Expected a 16 byte MD5 digest"));
    }
    let digest = Md5Hash::from_slice(bytes.as_bytes());
    Ok(digest.clone())
}
