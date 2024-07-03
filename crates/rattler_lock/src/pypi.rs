use crate::{PackageHashes, UrlOrPath};
use pep440_rs::VersionSpecifiers;
use pep508_rs::{ExtraName, PackageName, Requirement};
use rattler_digest::{digest::Digest, Sha256};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// A pinned Pypi package
#[serde_as]
#[skip_serializing_none]
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, Hash)]
pub struct PypiPackageData {
    /// The name of the package.
    pub name: PackageName,

    /// The version of the package.
    pub version: pep440_rs::Version,

    /// The URL that points to where the artifact can be downloaded from.
    #[serde(with = "crate::utils::serde::url_or_path", flatten)]
    pub url_or_path: UrlOrPath,

    /// Hashes of the file pointed to by `url`.
    #[serde(flatten)]
    pub hash: Option<PackageHashes>,

    /// A list of dependencies on other packages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_dist: Vec<Requirement>,

    /// The python version that this package requires.
    pub requires_python: Option<VersionSpecifiers>,

    /// Whether the projects should be installed in editable mode or not.
    #[serde(default, skip_serializing_if = "should_skip_serializing_editable")]
    pub editable: bool,
}

/// Additional runtime configuration of a package. Multiple environments/platforms might refer to
/// the same pypi package but with different extras enabled.
#[derive(Clone, Debug, Default)]
pub struct PypiPackageEnvironmentData {
    /// The extras enabled for the package. Note that the order doesn't matter here but it does matter for serialization.
    pub extras: BTreeSet<ExtraName>,
}

impl PartialOrd for PypiPackageData {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PypiPackageData {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name
            .cmp(&other.name)
            .then_with(|| self.version.cmp(&other.version))
            .then_with(|| self.url_or_path.cmp(&other.url_or_path))
            .then_with(|| self.hash.cmp(&other.hash))
    }
}

impl PypiPackageData {
    /// Returns true if this package satisfies the given `spec`.
    pub fn satisfies(&self, spec: &Requirement) -> bool {
        // Check if the name matches
        if spec.name != self.name {
            return false;
        }

        // Check if the version of the requirement matches
        match &spec.version_or_url {
            None => {}
            Some(pep508_rs::VersionOrUrl::Url(_)) => return false,
            Some(pep508_rs::VersionOrUrl::VersionSpecifier(spec)) => {
                if !spec.contains(&self.version) {
                    return false;
                }
            }
        }

        true
    }
}

/// Used in `skip_serializing_if` to skip serializing the `editable` field if it is `false`.
fn should_skip_serializing_editable(editable: &bool) -> bool {
    !*editable
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
