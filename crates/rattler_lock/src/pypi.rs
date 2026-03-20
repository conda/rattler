use crate::{PackageHashes, UrlOrPath, Verbatim};
use pep440_rs::VersionSpecifiers;
use pep508_rs::{PackageName, Requirement};
use rattler_digest::{digest::Digest, Sha256};
use std::cmp::Ordering;
use std::fs;
use std::path::Path;

/// A pinned `PyPI` package, either a wheel (immutable artifact) or a source
/// directory (mutable local path).
#[derive(Eq, PartialEq, Clone, Debug, Hash)]
pub enum PypiPackageData {
    /// A wheel package — an immutable artifact with a known version.
    Wheel(Box<PypiWheelData>),

    /// A local source directory whose content can change at any time.
    Source(Box<PypiSourceData>),
}

/// Data for a wheel package (index-served or local `.whl` file).
#[derive(Eq, PartialEq, Clone, Debug, Hash)]
pub struct PypiWheelData {
    /// The name of the package.
    pub name: PackageName,

    /// The version of the package.
    pub version: pep440_rs::Version,

    /// The location of the package. This can be a URL or a path.
    pub location: Verbatim<UrlOrPath>,

    /// The index this came from. Is `None` for local wheel files.
    pub index_url: Option<url::Url>,

    /// Hashes of the file pointed to by the location.
    pub hash: Option<PackageHashes>,

    /// A list of dependencies on other packages.
    pub requires_dist: Vec<Requirement>,

    /// The python version that this package requires.
    pub requires_python: Option<VersionSpecifiers>,
}

/// Data for a local source directory package.
#[derive(Eq, PartialEq, Clone, Debug, Hash)]
pub struct PypiSourceData {
    /// The name of the package.
    pub name: PackageName,

    /// The location of the source directory.
    pub location: Verbatim<UrlOrPath>,

    /// A list of dependencies on other packages.
    pub requires_dist: Vec<Requirement>,

    /// The python version that this package requires.
    pub requires_python: Option<VersionSpecifiers>,
}

impl PartialOrd for PypiWheelData {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PypiWheelData {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name
            .cmp(&other.name)
            .then_with(|| self.version.cmp(&other.version))
            .then_with(|| self.location.cmp(&other.location))
            .then_with(|| self.hash.cmp(&other.hash))
    }
}

impl PartialOrd for PypiSourceData {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PypiSourceData {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name
            .cmp(&other.name)
            .then_with(|| self.location.cmp(&other.location))
    }
}

impl PartialOrd for PypiPackageData {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PypiPackageData {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Wheel(a), Self::Wheel(b)) => a.cmp(b),
            (Self::Source(a), Self::Source(b)) => a.cmp(b),
            (Self::Wheel(_), Self::Source(_)) => Ordering::Less,
            (Self::Source(_), Self::Wheel(_)) => Ordering::Greater,
        }
    }
}

impl PypiPackageData {
    /// Returns the name of the package.
    pub fn name(&self) -> &PackageName {
        match self {
            Self::Wheel(w) => &w.name,
            Self::Source(s) => &s.name,
        }
    }

    /// Returns the location of the package.
    pub fn location(&self) -> &Verbatim<UrlOrPath> {
        match self {
            Self::Wheel(w) => &w.location,
            Self::Source(s) => &s.location,
        }
    }

    /// Returns true if this package satisfies the given `spec`.
    pub fn satisfies(&self, spec: &Requirement) -> bool {
        if spec.name != *self.name() {
            return false;
        }

        match &spec.version_or_url {
            None => true,
            Some(pep508_rs::VersionOrUrl::Url(_)) => false,
            Some(pep508_rs::VersionOrUrl::VersionSpecifier(spec)) => match self {
                Self::Wheel(w) => spec.contains(&w.version),
                Self::Source(_) => true,
            },
        }
    }

    /// Returns a reference to the wheel data if this is a wheel.
    pub fn as_wheel(&self) -> Option<&PypiWheelData> {
        match self {
            Self::Wheel(w) => Some(w),
            Self::Source(_) => None,
        }
    }

    /// Returns a reference to the source data if this is a source directory.
    pub fn as_source(&self) -> Option<&PypiSourceData> {
        match self {
            Self::Wheel(_) => None,
            Self::Source(s) => Some(s),
        }
    }

    /// Consumes self and returns the wheel data if this is a wheel.
    pub fn into_wheel(self) -> Option<PypiWheelData> {
        match self {
            Self::Wheel(w) => Some(*w),
            Self::Source(_) => None,
        }
    }

    /// Consumes self and returns the source data if this is a source directory.
    pub fn into_source(self) -> Option<PypiSourceData> {
        match self {
            Self::Wheel(_) => None,
            Self::Source(s) => Some(*s),
        }
    }
}

impl From<PypiWheelData> for PypiPackageData {
    fn from(value: PypiWheelData) -> Self {
        Self::Wheel(Box::new(value))
    }
}

impl From<PypiSourceData> for PypiPackageData {
    fn from(value: PypiSourceData) -> Self {
        Self::Source(Box::new(value))
    }
}

/// A struct that wraps the hashable part of a source package.
///
/// This struct the relevant parts of a source package that are used to compute a [`PackageHashes`].
pub struct PypiSourceTreeHashable {
    /// The contents of an optional pyproject.toml file.
    pub pyproject_toml: Option<String>,

    /// The contents of an optional setup.py file.
    pub setup_py: Option<String>,

    /// The contents of an optional setup.cfg file.
    pub setup_cfg: Option<String>,
}

fn ignore_not_found<C>(result: std::io::Result<C>) -> std::io::Result<Option<C>> {
    match result {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Ensure that line endings are normalized to `\n` this ensures that if files are checked out on
/// windows through git they still have the same hash as on linux.
fn normalize_file_contents(contents: &str) -> String {
    contents.replace("\r\n", "\n")
}

impl PypiSourceTreeHashable {
    /// Creates a new [`PypiSourceTreeHashable`] from a directory containing a source package.
    pub fn from_directory(directory: impl AsRef<Path>) -> std::io::Result<Self> {
        let directory = directory.as_ref();

        let pyproject_toml =
            ignore_not_found(fs::read_to_string(directory.join("pyproject.toml")))?;
        let setup_py = ignore_not_found(fs::read_to_string(directory.join("setup.py")))?;
        let setup_cfg = ignore_not_found(fs::read_to_string(directory.join("setup.cfg")))?;

        Ok(Self {
            pyproject_toml: pyproject_toml.as_deref().map(normalize_file_contents),
            setup_py: setup_py.as_deref().map(normalize_file_contents),
            setup_cfg: setup_cfg.as_deref().map(normalize_file_contents),
        })
    }

    /// Determine the [`PackageHashes`] of this source package.
    pub fn hash(&self) -> PackageHashes {
        let mut hasher = Sha256::new();

        if let Some(pyproject_toml) = &self.pyproject_toml {
            hasher.update(pyproject_toml.as_bytes());
        }

        if let Some(setup_py) = &self.setup_py {
            hasher.update(setup_py.as_bytes());
        }

        if let Some(setup_cfg) = &self.setup_cfg {
            hasher.update(setup_cfg.as_bytes());
        }

        PackageHashes::Sha256(hasher.finalize())
    }
}
