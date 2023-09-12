use pyo3::{pyclass, pymethods};
use rattler_repodata_gateway::fetch::CachedRepoData;
use std::sync::Arc;

#[pyclass]
#[repr(transparent)]
#[derive(Clone, Debug)]
pub struct PyCachedRepoData {
    pub(crate) inner: Arc<CachedRepoData>,
}

impl From<PyCachedRepoData> for CachedRepoData {
    fn from(value: PyCachedRepoData) -> Self {
        Arc::<CachedRepoData>::into_inner(value.inner)
            .expect("CachedRepoData has multiple strong references!")
    }
}

impl From<CachedRepoData> for PyCachedRepoData {
    fn from(value: CachedRepoData) -> Self {
        Self {
            inner: Arc::new(value),
        }
    }
}

#[pymethods]
impl PyCachedRepoData {
    pub fn as_str(&self) -> String {
        format!("{self:?}")
    }
}
