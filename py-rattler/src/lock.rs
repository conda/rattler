use std::{collections::BTreeMap, path::Path};

use pyo3::{
    pyclass, pymethods,
    types::{IntoPyDict, PyDict},
    PyErr, PyResult, Python,
};
use rattler_conda_types::{MatchSpec, Platform};
use rattler_lock::{
    builder::{CondaLockedDependencyBuilder, LockFileBuilder, LockedDependencyBuilder, LockedPackagesBuilder},
    Channel, CondaLock, GitMeta, LockMeta, LockedDependency, PackageHashes, TimeMeta,
};

use crate::{
    channel::PyChannel,
    error::PyRattlerError,
    match_spec::PyMatchSpec,
    platform::{self, PyPlatform},
    record::PyRecord,
};

#[pyclass]
#[derive(Clone)]
pub struct PyCondaLock {
    pub(crate) inner: CondaLock,
}

impl From<PyCondaLock> for CondaLock {
    fn from(value: PyCondaLock) -> Self {
        value.inner
    }
}

impl From<CondaLock> for PyCondaLock {
    fn from(value: CondaLock) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyCondaLock {
    #[new]
    pub fn new(
        channels: Vec<PyChannel>,
        platforms: Vec<PyPlatform>,
        input_spec: Vec<PyMatchSpec>,
        records: Vec<PyRecord>,
    ) -> PyResult<Self> {
        let builder = LockFileBuilder::new(
            channels
                .into_iter()
                .map(Into::<rattler_lock::Channel>::into),
            platforms.into_iter().map(Into::<Platform>::into),
            input_spec.into_iter().map(Into::<MatchSpec>::into),
        );
        let records = records
            .into_iter()
            .map(|r| {
                Ok(
                    TryInto::<CondaLockedDependencyBuilder>::try_into(r.try_as_repodata_record()?)
                        .map_err(PyRattlerError::from)?,
                )
            })
            .collect::<Result<Vec<_>, PyErr>>()?;

        for p in platforms {
            let locked_package_builder = LockedPackagesBuilder::new(p.inner);
            for r in records {
                locked_package_builder.add_locked_package(r);
            }
        }
        todo!()
    }

    pub fn to_path(&self, path: &str) -> PyResult<()> {
        Ok(self
            .inner
            .to_path(Path::new(path))
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    #[staticmethod]
    pub fn from_path(path: &str) -> PyResult<Self> {
        Ok(CondaLock::from_path(Path::new(path))
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    pub fn packages_for_platform(&self, platform: PyPlatform) -> Vec<PyLockedDependency> {
        self.inner
            .packages_for_platform(platform.inner)
            .map(Into::into)
            .collect::<Vec<_>>()
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyLockMeta {
    pub(crate) inner: LockMeta,
}

impl From<PyLockMeta> for LockMeta {
    fn from(value: PyLockMeta) -> Self {
        value.inner
    }
}

impl From<LockMeta> for PyLockMeta {
    fn from(value: LockMeta) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyLockMeta {
    #[getter]
    pub fn content_hash(&self) -> BTreeMap<PyPlatform, String> {
        self.inner
            .content_hash
            .clone()
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect::<BTreeMap<_, _>>()
    }

    // #[getter]
    // pub fn channels(&self) ->  {
    //     self.inner.channels.clone()
    // }

    #[getter]
    pub fn platforms(&self) -> Vec<PyPlatform> {
        self.inner
            .platforms
            .clone()
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>()
    }

    #[getter]
    pub fn sources(&self) -> Vec<String> {
        self.inner.sources.clone()
    }

    #[getter]
    pub fn time_metadata(&self) -> Option<PyTimeMeta> {
        self.inner
            .time_metadata
            .clone()
            .map_or(None, |v| Some(v.into()))
    }

    #[getter]
    pub fn git_metadata(&self) -> Option<PyGitMeta> {
        self.inner
            .git_metadata
            .clone()
            .map_or(None, |v| Some(v.into()))
    }

    // #[getter]
    // pub fn inputs_metadata<'a>(&self, py: Python<'a>) -> Option<&'a PyDict> {
    //     if let Some(metadata) = self.inner.inputs_metadata.clone() {
    //         Some(metadata.into_iter().map(|(k, v)| (k, v.into())).collect::<IndexMap<_, PyPackageHashes>>().into_py_dict(py))
    //     } else {
    //         None
    //     }
    // }

    #[getter]
    pub fn customs_metadata<'a>(&self, py: Python<'a>) -> Option<&'a PyDict> {
        self.inner
            .custom_metadata
            .clone()
            .map(|v| v.into_py_dict(py))
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyLockedDependency {
    pub(crate) inner: LockedDependency,
}

impl From<PyLockedDependency> for LockedDependency {
    fn from(value: PyLockedDependency) -> Self {
        value.inner
    }
}

impl From<LockedDependency> for PyLockedDependency {
    fn from(value: LockedDependency) -> Self {
        Self { inner: value }
    }
}

impl From<&LockedDependency> for PyLockedDependency {
    fn from(value: &LockedDependency) -> Self {
        value.clone().into()
    }
}

// #[pymethods]
// impl PyLockedDependency {
//     #[new]
//     pub fn new(platform: PyPlatform, ) -> Self {

//     }
// }

// impl TryFrom<PyRecord> for Vec<PyLockedDependency> {
//     type Error = PyErr;

//     fn try_from(value: PyRecord) -> Result<Self, Self::Error> {
//     //    let a: Result<CondaLockedDependencyBuilder, _> = value.try_as_repodata_record()?.try_into();
//     //     match a {
//     //         Ok(v) => Ok(Into::<LockedDependency>::into(v).into()),
//     //         Err(e) => todo!(),
//     //     }
//         // .map_err(|_e| PyRattlerError::LinkError("xxxx".into())?)
//         let a: LockedDependencyBuilder = TryInto::<CondaLockedDependencyBuilder>::try_into(value.try_as_repodata_record()?).unwrap().into();
//         Ok(a.build())
//         // todo!()
//     }
// }

#[pyclass]
#[derive(Clone)]
pub struct PyTimeMeta {
    pub(crate) inner: TimeMeta,
}

impl From<PyTimeMeta> for TimeMeta {
    fn from(value: PyTimeMeta) -> Self {
        value.inner
    }
}

impl From<TimeMeta> for PyTimeMeta {
    fn from(value: TimeMeta) -> Self {
        Self { inner: value }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyGitMeta {
    pub(crate) inner: GitMeta,
}

impl From<PyGitMeta> for GitMeta {
    fn from(value: PyGitMeta) -> Self {
        value.inner
    }
}

impl From<GitMeta> for PyGitMeta {
    fn from(value: GitMeta) -> Self {
        Self { inner: value }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyPackageHashes {
    pub(crate) inner: PackageHashes,
}

impl From<PyPackageHashes> for PackageHashes {
    fn from(value: PyPackageHashes) -> Self {
        value.inner
    }
}

impl From<PackageHashes> for PyPackageHashes {
    fn from(value: PackageHashes) -> Self {
        Self { inner: value }
    }
}
