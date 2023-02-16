use serde::{Deserialize, Serialize};

/// Type of runtime export.
/// See more info in [the conda docs](https://docs.conda.io/projects/conda-build/en/latest/resources/define-metadata.html#export-runtime-requirements)
/// [`package::run_exports::RunExports`]
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum RunExportKind {
    Weak,
    Strong,
    Noarch,
    WeakConstrain,
    StrongConstrain,
}
