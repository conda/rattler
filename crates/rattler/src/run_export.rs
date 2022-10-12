use serde::{Deserialize, Serialize};

/// Type of runtime export
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum RunExportKind {
    Weak,
    Strong,
}

/// Set of run exports of a package
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq, Hash, Default)]
pub struct RunExports {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    weak: Vec<String>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    strong: Vec<String>,
}
