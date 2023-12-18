use pyo3::pyclass;
use rattler_conda_types::PatchInstructions;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPatchInstructions {
    pub(crate) inner: PatchInstructions,
}
