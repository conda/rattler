use serde::{Deserialize, Deserializer};

/// Noarch packages are packages that are not architecture specific and therefore only have to be
/// built once. Noarch packages are either generic or Python.
///
/// This type describes the exact form in which the `noarch` was specified in a package record. Use
/// the [`NoArchType`] and [`NoArchKind`] for a higher level API.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum RawNoArchType {
    /// A generic noarch package. This differs from [`GenericV2`] by how it is stored in the
    /// repodata (old-format vs new-format)
    GenericV1,

    /// A generic noarch package. This differs from [`GenericV1`] by how it is stored in the
    /// repodata (old-format vs new-format)
    GenericV2,

    /// A noarch python package.
    Python,
}

/// Noarch packages are packages that are not architecture specific and therefore only have to be
/// built once. A `NoArchType` is either specific to an architecture or not. See [`NoArchKind`] for
/// more information on the different types of `noarch`.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Default)]
pub struct NoArchType(Option<RawNoArchType>);

impl From<NoArchType> for Option<RawNoArchType> {
    fn from(value: NoArchType) -> Self {
        value.0
    }
}

impl NoArchType {
    /// Returns the kind of this instance or `None` if this is not a noarch instance at all.
    pub fn kind(&self) -> Option<NoArchKind> {
        match self.0.as_ref() {
            None => None,
            Some(RawNoArchType::GenericV1) | Some(RawNoArchType::GenericV2) => {
                Some(NoArchKind::Generic)
            }
            Some(RawNoArchType::Python) => Some(NoArchKind::Python),
        }
    }
}

/// Defines the type of noarch that a package could be.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum NoArchKind {
    /// A noarch python package is a python package without any precompiled python files (`.pyc` or
    /// `__pycache__`). Normally these files are bundled with the package. However, these files are
    /// tied to a specific version of Python and must therefor be generated for every target
    /// platform and architecture. This complicates the build process.
    ///
    /// For noarch python packages these files are generated when installing the package by invoking
    /// the compilation process through the python binary that is installed in the same environment.
    ///
    /// This introductory blog post highlights some of specific of noarch python packages:
    /// https://www.anaconda.com/blog/condas-new-noarch-packages
    ///
    /// Or read the docs for more information:
    /// https://docs.conda.io/projects/conda/en/latest/user-guide/concepts/packages.html#noarch-python
    Python,

    /// Noarch generic packages allow users to distribute docs, datasets, and source code in conda
    /// packages.
    Generic,
}

/// Deserializer the parse the `noarch` field in conda package data.
impl<'de> Deserialize<'de> for NoArchType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Clone, Debug, Deserialize)]
        #[serde(untagged)]
        enum NoArchSerde {
            OldFormat(bool),
            NewFormat(NoArchTypeSerde),
        }

        #[derive(Clone, Debug, Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum NoArchTypeSerde {
            Python,
            Generic,
        }

        let value = Option::<NoArchSerde>::deserialize(deserializer)?;
        Ok(NoArchType(value.and_then(|value| match value {
            NoArchSerde::OldFormat(true) => Some(RawNoArchType::GenericV1),
            NoArchSerde::OldFormat(false) => None,
            NoArchSerde::NewFormat(NoArchTypeSerde::Python) => Some(RawNoArchType::Python),
            NoArchSerde::NewFormat(NoArchTypeSerde::Generic) => Some(RawNoArchType::GenericV2),
        })))
    }
}
