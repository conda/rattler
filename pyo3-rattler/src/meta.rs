use pyo3::pyfunction;

const VERSION: &str = env!("CARGO_PKG_VERSION");
#[pyfunction]
pub fn get_rattler_version() -> &'static str {
    VERSION
}
