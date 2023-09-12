mod channel;
mod error;
mod match_spec;
mod nameless_match_spec;
mod networking;
mod platform;
mod repo_data;
mod shell;
mod version;

use channel::{PyChannel, PyChannelConfig};
use error::{
    ActivationException, InvalidChannelException, InvalidMatchSpecException,
    InvalidPackageNameException, InvalidUrlException, InvalidVersionException, ParseArchException,
    ParsePlatformException, PyRattlerError,
};
use match_spec::PyMatchSpec;
use nameless_match_spec::PyNamelessMatchSpec;
use networking::{authenticated_client::PyAuthenticatedClient, py_fetch_repo_data};
use repo_data::{
    package_record::PyPackageRecord, patch_instructions::PyPatchInstructions,
    repo_data_record::PyRepoDataRecord, PyRepoData,
};
use version::PyVersion;

use pyo3::prelude::*;

use platform::{PyArch, PyPlatform};
use shell::{PyActivationResult, PyActivationVariables, PyActivator, PyShellEnum};

#[pymodule]
fn rattler(py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<PyVersion>().unwrap();

    m.add_class::<PyMatchSpec>().unwrap();
    m.add_class::<PyNamelessMatchSpec>().unwrap();

    m.add_class::<PyPackageRecord>().unwrap();

    m.add_class::<PyChannel>().unwrap();
    m.add_class::<PyChannelConfig>().unwrap();
    m.add_class::<PyPlatform>().unwrap();
    m.add_class::<PyArch>().unwrap();

    m.add_class::<PyAuthenticatedClient>().unwrap();

    // Shell activation things
    m.add_class::<PyActivationVariables>().unwrap();
    m.add_class::<PyActivationResult>().unwrap();
    m.add_class::<PyShellEnum>().unwrap();
    m.add_class::<PyActivator>().unwrap();

    m.add_class::<PyRepoData>().unwrap();
    m.add_class::<PyRepoDataRecord>().unwrap();
    m.add_class::<PyPatchInstructions>().unwrap();

    m.add_function(wrap_pyfunction!(py_fetch_repo_data, m).unwrap())
        .unwrap();

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
    m.add("ActivationError", py.get_type::<ActivationException>())
        .unwrap();
    m.add(
        "ParsePlatformError",
        py.get_type::<ParsePlatformException>(),
    )
    .unwrap();
    m.add("ParseArchError", py.get_type::<ParseArchException>())
        .unwrap();

    Ok(())
}
