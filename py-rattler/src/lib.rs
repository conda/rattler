mod about_json;
mod channel;
mod error;
mod generic_virtual_package;
mod index;
mod installer;
mod lock;
mod match_spec;
mod meta;
mod nameless_match_spec;
mod networking;
mod no_arch_type;
mod package_name;
mod paths_json;
mod platform;
mod prefix_paths;
mod record;
mod repo_data;
mod shell;
mod solver;
mod version;
mod virtual_package;

mod index_json;
mod run_exports_json;

use std::ops::Deref;

use about_json::PyAboutJson;
use channel::{PyChannel, PyChannelConfig, PyChannelPriority};
use error::{
    ActivationException, CacheDirException, ConvertSubdirException, DetectVirtualPackageException,
    EnvironmentCreationException, ExtractException, FetchRepoDataException,
    InvalidChannelException, InvalidMatchSpecException, InvalidPackageNameException,
    InvalidUrlException, InvalidVersionException, IoException, LinkException, ParseArchException,
    ParsePlatformException, PyRattlerError, SolverException, TransactionException,
    VersionBumpException,
};
use generic_virtual_package::PyGenericVirtualPackage;
use index::py_index;
use index_json::PyIndexJson;
use installer::py_install;
use lock::{
    PyEnvironment, PyLockChannel, PyLockFile, PyLockedPackage, PyPackageHashes, PyPypiPackageData,
    PyPypiPackageEnvironmentData,
};
use match_spec::PyMatchSpec;
use meta::get_rattler_version;
use nameless_match_spec::PyNamelessMatchSpec;
use networking::{authenticated_client::PyAuthenticatedClient, py_fetch_repo_data};
use no_arch_type::PyNoArchType;
use package_name::PyPackageName;
use paths_json::{PyFileMode, PyPathType, PyPathsEntry, PyPathsJson, PyPrefixPlaceholder};
use platform::{PyArch, PyPlatform};
use prefix_paths::{PyPrefixPathType, PyPrefixPaths, PyPrefixPathsEntry};
use pyo3::prelude::*;
use record::PyRecord;
use repo_data::{
    gateway::{PyGateway, PySourceConfig},
    patch_instructions::PyPatchInstructions,
    sparse::PySparseRepoData,
    PyRepoData,
};
use run_exports_json::PyRunExportsJson;
use shell::{PyActivationResult, PyActivationVariables, PyActivator, PyShellEnum};
use solver::{py_solve, py_solve_with_sparse_repodata};
use version::PyVersion;
use virtual_package::{PyOverride, PyVirtualPackage, PyVirtualPackageOverrides};

use crate::error::GatewayException;

/// A struct to make it easy to wrap a type as a python type.
#[repr(transparent)]
#[derive(Clone)]
pub struct Wrap<T>(pub T);

impl<T> Deref for Wrap<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[pymodule]
fn rattler(py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_class::<PyVersion>().unwrap();

    m.add_class::<PyMatchSpec>().unwrap();
    m.add_class::<PyNamelessMatchSpec>().unwrap();

    m.add_class::<PyPackageName>().unwrap();

    m.add_class::<PyChannel>().unwrap();
    m.add_class::<PyChannelConfig>().unwrap();
    m.add_class::<PyChannelPriority>().unwrap();
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
    m.add_class::<PyGateway>().unwrap();
    m.add_class::<PySourceConfig>().unwrap();

    m.add_class::<PyRecord>().unwrap();

    m.add_function(wrap_pyfunction!(py_fetch_repo_data, m).unwrap())
        .unwrap();
    m.add_class::<PyGenericVirtualPackage>().unwrap();
    m.add_class::<PyOverride>().unwrap();
    m.add_class::<PyVirtualPackageOverrides>().unwrap();
    m.add_class::<PyVirtualPackage>().unwrap();
    m.add_class::<PyPrefixPathsEntry>().unwrap();
    m.add_class::<PyPrefixPathType>().unwrap();
    m.add_class::<PyPrefixPaths>().unwrap();

    m.add_class::<PyNoArchType>().unwrap();

    m.add_class::<PyLockFile>().unwrap();
    m.add_class::<PyEnvironment>().unwrap();
    m.add_class::<PyLockChannel>().unwrap();
    m.add_class::<PyLockedPackage>().unwrap();
    m.add_class::<PyPypiPackageData>().unwrap();
    m.add_class::<PyPypiPackageEnvironmentData>().unwrap();
    m.add_class::<PyPackageHashes>().unwrap();

    m.add_class::<PyAboutJson>().unwrap();

    m.add_class::<PyRunExportsJson>().unwrap();
    m.add_class::<PyPathsJson>().unwrap();
    m.add_class::<PyPathsEntry>().unwrap();
    m.add_class::<PyPathType>().unwrap();
    m.add_class::<PyPrefixPlaceholder>().unwrap();
    m.add_class::<PyFileMode>().unwrap();
    m.add_class::<PyIndexJson>().unwrap();

    m.add_function(wrap_pyfunction!(py_solve, m).unwrap())
        .unwrap();
    m.add_function(wrap_pyfunction!(py_solve_with_sparse_repodata, m).unwrap())
        .unwrap();
    m.add_function(wrap_pyfunction!(get_rattler_version, m).unwrap())
        .unwrap();
    m.add_function(wrap_pyfunction!(py_install, m).unwrap())
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

    m.add("ExtractError", py.get_type::<ExtractException>())
        .unwrap();

    m.add("GatewayError", py.get_type::<GatewayException>())
        .unwrap();

    Ok(())
}
