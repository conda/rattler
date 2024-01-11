use pyo3::{IntoPy, PyObject, Python};
use rattler_conda_types::Component;

pub enum PyComponent {
    String(String),
    Number(u64),
}

impl IntoPy<PyObject> for PyComponent {
    fn into_py(self, py: Python<'_>) -> PyObject {
        match self {
            Self::Number(val) => val.into_py(py),
            Self::String(val) => val.into_py(py),
        }
    }
}

impl From<Component> for PyComponent {
    fn from(value: Component) -> Self {
        match value {
            Component::Iden(v) => Self::String(v.to_string()),
            Component::Numeral(n) => Self::Number(n),
            Component::Dev => Self::String("dev".to_string()),
            Component::Post => Self::String("post".to_string()),
            Component::UnderscoreOrDash { .. } => Self::String("_".to_string()),
        }
    }
}
