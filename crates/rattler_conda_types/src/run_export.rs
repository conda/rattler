use serde::{Deserialize, Serialize};

/// Type of runtime export.
/// See more info in [the conda docs](https://docs.conda.io/projects/conda-build/en/latest/resources/define-metadata.html#export-runtime-requirements)
/// [`crate::package::RunExportsJson`]
#[allow(missing_docs)]
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum RunExportKind {
    Weak,
    Strong,
    Noarch,
    WeakConstrain,
    StrongConstrain,
}
