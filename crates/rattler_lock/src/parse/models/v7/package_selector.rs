use serde::{Deserialize, Serialize};

use crate::{SelectorId, SelectorKind};

/// A tagged reference to another package in the lockfile.
///
/// Serializes as a single-key YAML mapping whose key is the package kind
/// (`conda`, `conda_source`, or `pypi`) and whose value is the kind-specific
/// id. This matches how top-level environment package entries are emitted,
/// so `build_packages` and `host_packages` entries read the same way.
#[derive(Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd, Clone, Debug, Hash)]
#[serde(untagged, rename_all = "snake_case")]
pub(crate) enum PackageSelector {
    Conda { conda: String },
    CondaSource { conda_source: String },
    Pypi { pypi: String },
}

impl PackageSelector {
    pub(crate) fn kind(&self) -> SelectorKind {
        match self {
            Self::Conda { .. } => SelectorKind::CondaBinary,
            Self::CondaSource { .. } => SelectorKind::CondaSource,
            Self::Pypi { .. } => SelectorKind::Pypi,
        }
    }

    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Conda { conda } => conda,
            Self::CondaSource { conda_source } => conda_source,
            Self::Pypi { pypi } => pypi,
        }
    }

    pub(crate) fn to_selector_id(&self) -> SelectorId {
        SelectorId::from_parts(self.kind(), self.id())
    }

    pub(crate) fn from_selector_id(id: &SelectorId) -> Self {
        let raw = id.as_str().to_string();
        match id.kind() {
            SelectorKind::CondaBinary => Self::Conda { conda: raw },
            SelectorKind::CondaSource => Self::CondaSource { conda_source: raw },
            SelectorKind::Pypi => Self::Pypi { pypi: raw },
        }
    }
}
