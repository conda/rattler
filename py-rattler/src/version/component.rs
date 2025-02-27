use pyo3::{Bound, IntoPyObject, PyAny, Python};
use rattler_conda_types::Component;

pub enum PyComponent {
    String(String),
    Number(u64),
}

impl<'py> IntoPyObject<'py> for PyComponent {
    type Target = PyAny;
    type Output = Bound<'py, Self::Target>;
    type Error = std::convert::Infallible;

    fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
        match self {
            Self::Number(val) => Ok(val.into_pyobject(py)?.into_any()),
            Self::String(val) => Ok(val.into_pyobject(py)?.into_any()),
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
