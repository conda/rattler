use serde::{Deserialize, Serialize};

/// Type of runtime export.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum RunExportKind {
    Weak,
    Strong,
    Noarch,
    WeakConstrain,
    StrongConstrain,
}
