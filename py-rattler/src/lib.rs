mod error;
mod version;

use error::{InvalidVersionException, PyRattlerError};

use pyo3::prelude::*;
use version::PyVersion;

#[pymodule]
fn rattler(py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<PyVersion>().unwrap();

    // Exceptions
    m.add(
        "InvalidVersionError",
        py.get_type::<InvalidVersionException>(),
    )
    .unwrap();
    Ok(())
}
