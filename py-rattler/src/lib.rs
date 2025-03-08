mod about_json;
mod channel;
mod error;
mod explicit_environment_spec;
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
mod package_streaming;
mod paths_json;
mod platform;
mod prefix_paths;
mod record;
mod repo_data;
mod shell;
mod solver;
mod utils;
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
    ValidatePackageRecordsException, VersionBumpException,
};
use explicit_environment_spec::{PyExplicitEnvironmentEntry, PyExplicitEnvironmentSpec};
use generic_virtual_package::PyGenericVirtualPackage;
use index::{py_index_fs, py_index_s3};
use index_json::PyIndexJson;
use installer::py_install;
use lock::{
    PyEnvironment, PyLockChannel, PyLockFile, PyLockedPackage, PyPackageHashes, PyPypiPackageData,
    PyPypiPackageEnvironmentData,
};
use match_spec::PyMatchSpec;
use meta::get_rattler_version;
use nameless_match_spec::PyNamelessMatchSpec;
use networking::middleware::{
    PyAuthenticationMiddleware, PyGCSMiddleware, PyMirrorMiddleware, PyOciMiddleware, PyS3Config,
    PyS3Middleware,
};
use networking::{client::PyClientWithMiddleware, py_fetch_repo_data};
use no_arch_type::PyNoArchType;
use package_name::PyPackageName;
use paths_json::{PyFileMode, PyPathType, PyPathsEntry, PyPathsJson, PyPrefixPlaceholder};
use platform::{PyArch, PyPlatform};
use prefix_paths::{PyPrefixPathType, PyPrefixPaths, PyPrefixPathsEntry};
use pyo3::prelude::*;
use record::PyRecord;
use repo_data::{
    gateway::{PyFetchRepoDataOptions, PyGateway, PySourceConfig},
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
fn rattler<'py>(py: Python<'py>, m: Bound<'py, PyModule>) -> PyResult<()> {
    m.add_class::<PyVersion>()?;

    m.add_class::<PyMatchSpec>()?;
    m.add_class::<PyNamelessMatchSpec>()?;

    m.add_class::<PyPackageName>()?;

    m.add_class::<PyChannel>()?;
    m.add_class::<PyChannelConfig>()?;
    m.add_class::<PyChannelPriority>()?;
    m.add_class::<PyPlatform>()?;
    m.add_class::<PyArch>()?;

    m.add_class::<PyMirrorMiddleware>()?;
    m.add_class::<PyAuthenticationMiddleware>()?;
    m.add_class::<PyOciMiddleware>()?;
    m.add_class::<PyGCSMiddleware>()?;
    m.add_class::<PyS3Middleware>()?;
    m.add_class::<PyS3Config>()?;
    m.add_class::<PyClientWithMiddleware>()?;

    // Shell activation things
    m.add_class::<PyActivationVariables>()?;
    m.add_class::<PyActivationResult>()?;
    m.add_class::<PyShellEnum>()?;
    m.add_class::<PyActivator>()?;

    m.add_class::<PySparseRepoData>()?;
    m.add_class::<PyRepoData>()?;
    m.add_class::<PyPatchInstructions>()?;
    m.add_class::<PyGateway>()?;
    m.add_class::<PySourceConfig>()?;
    m.add_class::<PyFetchRepoDataOptions>()?;

    m.add_class::<PyRecord>()?;

    m.add_function(wrap_pyfunction!(py_fetch_repo_data, &m)?)?;
    m.add_class::<PyGenericVirtualPackage>()?;
    m.add_class::<PyOverride>()?;
    m.add_class::<PyVirtualPackageOverrides>()?;
    m.add_class::<PyVirtualPackage>()?;
    m.add_class::<PyPrefixPathsEntry>()?;
    m.add_class::<PyPrefixPathType>()?;
    m.add_class::<PyPrefixPaths>()?;

    m.add_class::<PyNoArchType>()?;

    m.add_class::<PyLockFile>()?;
    m.add_class::<PyEnvironment>()?;
    m.add_class::<PyLockChannel>()?;
    m.add_class::<PyLockedPackage>()?;
    m.add_class::<PyPypiPackageData>()?;
    m.add_class::<PyPypiPackageEnvironmentData>()?;
    m.add_class::<PyPackageHashes>()?;

    m.add_class::<PyAboutJson>()?;

    m.add_class::<PyRunExportsJson>()?;
    m.add_class::<PyPathsJson>()?;
    m.add_class::<PyPathsEntry>()?;
    m.add_class::<PyPathType>()?;
    m.add_class::<PyPrefixPlaceholder>()?;
    m.add_class::<PyFileMode>()?;
    m.add_class::<PyIndexJson>()?;

    m.add_function(wrap_pyfunction!(py_solve, &m).unwrap())?;
    m.add_function(wrap_pyfunction!(py_solve_with_sparse_repodata, &m).unwrap())?;
    m.add_function(wrap_pyfunction!(get_rattler_version, &m).unwrap())?;
    m.add_function(wrap_pyfunction!(py_install, &m).unwrap())?;
    m.add_function(wrap_pyfunction!(py_index_fs, &m).unwrap())?;
    m.add_function(wrap_pyfunction!(py_index_s3, &m).unwrap())?;

    m.add_function(wrap_pyfunction!(package_streaming::extract_tar_bz2, &m).unwrap())?;
    m.add_function(wrap_pyfunction!(package_streaming::extract, &m).unwrap())?;
    m.add_function(wrap_pyfunction!(package_streaming::download_and_extract, &m).unwrap())?;

    // Explicit environment specification
    m.add_class::<PyExplicitEnvironmentSpec>()?;
    m.add_class::<PyExplicitEnvironmentEntry>()?;

    // Exceptions
    m.add(
        "InvalidVersionError",
        py.get_type::<InvalidVersionException>(),
    )?;
    m.add(
        "InvalidMatchSpecError",
        py.get_type::<InvalidMatchSpecException>(),
    )?;
    m.add(
        "InvalidPackageNameError",
        py.get_type::<InvalidPackageNameException>(),
    )?;
    m.add("InvalidUrlError", py.get_type::<InvalidUrlException>())?;
    m.add(
        "InvalidChannelError",
        py.get_type::<InvalidChannelException>(),
    )?;
    m.add("ActivationError", py.get_type::<ActivationException>())?;
    m.add(
        "ParsePlatformError",
        py.get_type::<ParsePlatformException>(),
    )?;
    m.add("ParseArchError", py.get_type::<ParseArchException>())?;
    m.add("SolverError", py.get_type::<SolverException>())?;
    m.add("TransactionError", py.get_type::<TransactionException>())?;
    m.add("LinkError", py.get_type::<LinkException>())?;
    m.add("IoError", py.get_type::<IoException>())?;
    m.add(
        "DetectVirtualPackageError",
        py.get_type::<DetectVirtualPackageException>(),
    )?;
    m.add("CacheDirError", py.get_type::<CacheDirException>())?;
    m.add(
        "FetchRepoDataError",
        py.get_type::<FetchRepoDataException>(),
    )?;
    m.add(
        "ConvertSubdirError",
        py.get_type::<ConvertSubdirException>(),
    )?;
    m.add("VersionBumpError", py.get_type::<VersionBumpException>())?;

    m.add(
        "EnvironmentCreationError",
        py.get_type::<EnvironmentCreationException>(),
    )?;

    m.add("ExtractError", py.get_type::<ExtractException>())?;

    m.add("GatewayError", py.get_type::<GatewayException>())?;

    m.add(
        "ValidatePackageRecordsException",
        py.get_type::<ValidatePackageRecordsException>(),
    )?;

    Ok(())
}
