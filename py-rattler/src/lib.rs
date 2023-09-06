mod channel;
mod error;
mod match_spec;
mod nameless_match_spec;
mod repo_data;
mod version;

use channel::{PyChannel, PyChannelConfig};
use error::{
    InvalidChannelException, InvalidMatchSpecException, InvalidPackageNameException,
    InvalidUrlException, InvalidVersionException, PyRattlerError,
};
use match_spec::PyMatchSpec;
use nameless_match_spec::PyNamelessMatchSpec;
use repo_data::package_record::PyPackageRecord;
use version::PyVersion;

use pyo3::prelude::*;

#[pymodule]
fn rattler(py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<PyVersion>().unwrap();

    m.add_class::<PyMatchSpec>().unwrap();
    m.add_class::<PyNamelessMatchSpec>().unwrap();

    m.add_class::<PyPackageRecord>().unwrap();

    m.add_class::<PyChannel>().unwrap();
    m.add_class::<PyChannelConfig>().unwrap();

    // Exceptions
    m.add(
        "InvalidVersionError",
        py.get_type::<InvalidVersionException>(),
    )
    .unwrap();
    m.add(
        "InvalidMatchSpecError",
        py.get_type::<InvalidMatchSpecException>(),
    )
    .unwrap();
    m.add(
        "InvalidPackageNameError",
        py.get_type::<InvalidPackageNameException>(),
    )
    .unwrap();
    m.add("InvalidUrlError", py.get_type::<InvalidUrlException>())
        .unwrap();
    m.add(
        "InvalidChannelError",
        py.get_type::<InvalidChannelException>(),
    )
    .unwrap();

    Ok(())
}
