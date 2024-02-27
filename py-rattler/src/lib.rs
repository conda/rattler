use pyo3::prelude::*;
use pyo3_rattler::{
    get_rattler_version, py_fetch_repo_data, py_index, py_link, py_solve, ActivationException,
    CacheDirException, ConvertSubdirException, DetectVirtualPackageException,
    EnvironmentCreationException, FetchRepoDataException, InvalidChannelException,
    InvalidMatchSpecException, InvalidPackageNameException, InvalidUrlException,
    InvalidVersionException, IoException, LinkException, ParseArchException,
    ParsePlatformException, PyAboutJson, PyActivationResult, PyActivationVariables, PyActivator,
    PyArch, PyAuthenticatedClient, PyChannel, PyChannelConfig, PyEnvironment,
    PyGenericVirtualPackage, PyLockChannel, PyLockFile, PyLockedPackage, PyMatchSpec, PyModule,
    PyNamelessMatchSpec, PyPackageHashes, PyPackageName, PyPatchInstructions, PyPlatform,
    PyPrefixPaths, PyPypiPackageData, PyPypiPackageEnvironmentData, PyRecord, PyRepoData,
    PyShellEnum, PySparseRepoData, PyVersion, PyVirtualPackage, SolverException,
    TransactionException, VersionBumpException,
};

#[pymodule]
fn rattler(py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_class::<PyVersion>().unwrap();

    m.add_class::<PyMatchSpec>().unwrap();
    m.add_class::<PyNamelessMatchSpec>().unwrap();

    m.add_class::<PyPackageName>().unwrap();

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

    m.add_class::<PySparseRepoData>().unwrap();
    m.add_class::<PyRepoData>().unwrap();
    m.add_class::<PyPatchInstructions>().unwrap();

    m.add_class::<PyRecord>().unwrap();

    m.add_function(wrap_pyfunction!(py_fetch_repo_data, m).unwrap())
        .unwrap();
    m.add_class::<PyGenericVirtualPackage>().unwrap();
    m.add_class::<PyVirtualPackage>().unwrap();
    m.add_class::<PyPrefixPaths>().unwrap();

    m.add_class::<PyLockFile>().unwrap();
    m.add_class::<PyEnvironment>().unwrap();
    m.add_class::<PyLockChannel>().unwrap();
    m.add_class::<PyLockedPackage>().unwrap();
    m.add_class::<PyPypiPackageData>().unwrap();
    m.add_class::<PyPypiPackageEnvironmentData>().unwrap();
    m.add_class::<PyPackageHashes>().unwrap();

    m.add_class::<PyAboutJson>().unwrap();

    m.add_function(wrap_pyfunction!(py_solve, m).unwrap())
        .unwrap();
    m.add_function(wrap_pyfunction!(get_rattler_version, m).unwrap())
        .unwrap();
    m.add_function(wrap_pyfunction!(py_link, m).unwrap())
        .unwrap();
    m.add_function(wrap_pyfunction!(py_index, m).unwrap())
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
    m.add("SolverError", py.get_type::<SolverException>())
        .unwrap();
    m.add("TransactionError", py.get_type::<TransactionException>())
        .unwrap();
    m.add("LinkError", py.get_type::<LinkException>()).unwrap();
    m.add("IoError", py.get_type::<IoException>()).unwrap();
    m.add(
        "DetectVirtualPackageError",
        py.get_type::<DetectVirtualPackageException>(),
    )
    .unwrap();
    m.add("CacheDirError", py.get_type::<CacheDirException>())
        .unwrap();
    m.add(
        "FetchRepoDataError",
        py.get_type::<FetchRepoDataException>(),
    )
    .unwrap();
    m.add(
        "ConvertSubdirError",
        py.get_type::<ConvertSubdirException>(),
    )
    .unwrap();
    m.add("VersionBumpError", py.get_type::<VersionBumpException>())
        .unwrap();

    m.add(
        "EnvironmentCreationError",
        py.get_type::<EnvironmentCreationException>(),
    )
    .unwrap();

    Ok(())
}
