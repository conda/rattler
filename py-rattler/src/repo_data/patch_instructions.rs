use pyo3::pyclass;
use rattler_conda_types::PatchInstructions;

#[pyclass(from_py_object)]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPatchInstructions {
    pub(crate) inner: PatchInstructions,
}
