use std::{io, path::Path};

use rattler_conda_types::{MinimalPrefixRecord, PrefixRecord, RepoDataRecord};
use rattler_conda_types::{NoArchType, PackageName, PackageRecord, VersionWithSource};
use rattler_digest::{Md5Hash, Sha256Hash};

/// Special kind of record that either can be either minimal or full.
///
/// In case where nothing changed transaction will be noop and to
/// check this we have to read only few fields of `PrefixRecord` which
/// are entailed in `MinimalPrefixRecord`.
///
/// Without this structure we would have to inject
/// `MinimalPrefixRecord` to `PrefixRecord` which could lead to
/// unexpected results.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum InstallationResultRecord {
    /// Full record.
    Max(PrefixRecord),
    /// Minimal record.
    Min(MinimalPrefixRecord),
}

#[allow(deprecated)]
impl InstallationResultRecord {
    /// Either just returns stored `PrefixRecord` or parses `PrefixRecord` from the given prefix.
    pub fn into_prefix_record(self, prefix: impl AsRef<Path>) -> Result<PrefixRecord, io::Error> {
        match self {
            InstallationResultRecord::Max(prefix_record) => Ok(prefix_record),
            InstallationResultRecord::Min(minimal_prefix_record) => {
                let record_name = format!(
                    "{build}-{version}-{name}.json",
                    build = minimal_prefix_record.build,
                    version = minimal_prefix_record.version,
                    name = minimal_prefix_record.name.as_normalized()
                );
                let record_path = prefix.as_ref().join(record_name);
                PrefixRecord::from_path(record_path)
            }
        }
    }

    /// Return reference to the underlying `PackageName`.
    pub fn name(&self) -> &PackageName {
        match self {
            InstallationResultRecord::Max(prefix_record) => prefix_record.name(),
            InstallationResultRecord::Min(minimal_prefix_record) => minimal_prefix_record.name(),
        }
    }

    /// Return reference to the underlying `VersionWithSource`.
    pub fn version(&self) -> &rattler_conda_types::VersionWithSource {
        match self {
            InstallationResultRecord::Max(prefix_record) => prefix_record.version(),
            InstallationResultRecord::Min(minimal_prefix_record) => minimal_prefix_record.version(),
        }
    }

    /// Return reference to the underlying build string.
    pub fn build(&self) -> &str {
        match self {
            InstallationResultRecord::Max(prefix_record) => prefix_record.build(),
            InstallationResultRecord::Min(minimal_prefix_record) => minimal_prefix_record.build(),
        }
    }

    /// Return reference to the underlying `Sha256Hash`.
    pub fn sha256(&self) -> Option<&rattler_digest::Sha256Hash> {
        match self {
            InstallationResultRecord::Max(prefix_record) => prefix_record.sha256(),
            InstallationResultRecord::Min(minimal_prefix_record) => minimal_prefix_record.sha256(),
        }
    }

    /// Return reference to the underlying `Md5Hash`.
    pub fn md5(&self) -> Option<&rattler_digest::Md5Hash> {
        match self {
            InstallationResultRecord::Max(prefix_record) => prefix_record.md5(),
            InstallationResultRecord::Min(minimal_prefix_record) => minimal_prefix_record.md5(),
        }
    }

    /// Return reference to the underlying content size.
    pub fn size(&self) -> Option<u64> {
        match self {
            InstallationResultRecord::Max(prefix_record) => prefix_record.size(),
            InstallationResultRecord::Min(minimal_prefix_record) => minimal_prefix_record.size(),
        }
    }

    /// Return reference to the underlying `NoArchType`.
    pub fn noarch(&self) -> rattler_conda_types::NoArchType {
        match self {
            InstallationResultRecord::Max(prefix_record) => prefix_record.noarch(),
            InstallationResultRecord::Min(minimal_prefix_record) => minimal_prefix_record.noarch(),
        }
    }

    /// Return reference to the underlying `python_site_packages_path`.
    pub fn python_site_packages_path(&self) -> Option<&str> {
        match self {
            InstallationResultRecord::Max(prefix_record) => {
                prefix_record.python_site_packages_path()
            }
            InstallationResultRecord::Min(minimal_prefix_record) => {
                minimal_prefix_record.python_site_packages_path()
            }
        }
    }

    /// Return reference to the underlying `requested_spec`.
    pub fn requested_spec(&self) -> Option<&String> {
        match self {
            InstallationResultRecord::Max(prefix_record) => prefix_record.requested_spec.as_ref(),
            InstallationResultRecord::Min(minimal_prefix_record) => {
                minimal_prefix_record.requested_spec.as_ref()
            }
        }
    }

    /// Return reference to the underlying `requested_specs`.
    pub fn requested_specs(&self) -> &Vec<String> {
        match self {
            InstallationResultRecord::Max(prefix_record) => &prefix_record.requested_specs,
            InstallationResultRecord::Min(minimal_prefix_record) => {
                &minimal_prefix_record.requested_specs
            }
        }
    }

    /// Return reference to the underlying `requested_spec`.
    pub(crate) fn requested_spec_mut(&mut self) -> &mut Option<String> {
        match self {
            InstallationResultRecord::Max(prefix_record) => &mut prefix_record.requested_spec,
            InstallationResultRecord::Min(minimal_prefix_record) => {
                &mut minimal_prefix_record.requested_spec
            }
        }
    }

    /// Return reference to the underlying `requested_specs`.
    pub(crate) fn requested_specs_mut(&mut self) -> &mut Vec<String> {
        match self {
            InstallationResultRecord::Max(prefix_record) => &mut prefix_record.requested_specs,
            InstallationResultRecord::Min(minimal_prefix_record) => {
                &mut minimal_prefix_record.requested_specs
            }
        }
    }
}

/// Trait that defines the fields needed for package content comparison.
/// This ensures type safety - if `describe_same_content` needs a new field,
/// it must be added here and implemented for all types.
pub trait ContentComparable {
    fn name(&self) -> &PackageName;
    fn version(&self) -> &VersionWithSource;
    fn build(&self) -> &str;
    fn sha256(&self) -> Option<&Sha256Hash>;
    fn md5(&self) -> Option<&Md5Hash>;
    fn size(&self) -> Option<u64>;
    fn noarch(&self) -> NoArchType;
    fn python_site_packages_path(&self) -> Option<&str>;
}

impl ContentComparable for InstallationResultRecord {
    fn name(&self) -> &PackageName {
        self.name()
    }

    fn version(&self) -> &rattler_conda_types::VersionWithSource {
        self.version()
    }

    fn build(&self) -> &str {
        self.build()
    }

    fn sha256(&self) -> Option<&rattler_digest::Sha256Hash> {
        self.sha256()
    }

    fn md5(&self) -> Option<&rattler_digest::Md5Hash> {
        self.md5()
    }

    fn size(&self) -> Option<u64> {
        self.size()
    }

    fn noarch(&self) -> rattler_conda_types::NoArchType {
        self.noarch()
    }

    fn python_site_packages_path(&self) -> Option<&str> {
        self.python_site_packages_path()
    }
}

impl ContentComparable for PackageRecord {
    fn name(&self) -> &PackageName {
        &self.name
    }
    fn version(&self) -> &VersionWithSource {
        &self.version
    }
    fn build(&self) -> &str {
        &self.build
    }
    fn sha256(&self) -> Option<&Sha256Hash> {
        self.sha256.as_ref()
    }
    fn md5(&self) -> Option<&Md5Hash> {
        self.md5.as_ref()
    }
    fn size(&self) -> Option<u64> {
        self.size
    }
    fn noarch(&self) -> NoArchType {
        self.noarch
    }
    fn python_site_packages_path(&self) -> Option<&str> {
        self.python_site_packages_path.as_deref()
    }
}

impl ContentComparable for MinimalPrefixRecord {
    fn name(&self) -> &PackageName {
        &self.name
    }
    fn version(&self) -> &VersionWithSource {
        &self.version
    }
    fn build(&self) -> &str {
        &self.build
    }
    fn sha256(&self) -> Option<&Sha256Hash> {
        self.sha256.as_ref()
    }
    fn md5(&self) -> Option<&Md5Hash> {
        self.md5.as_ref()
    }
    fn size(&self) -> Option<u64> {
        self.size
    }
    fn noarch(&self) -> NoArchType {
        self.noarch
    }
    fn python_site_packages_path(&self) -> Option<&str> {
        self.python_site_packages_path.as_deref()
    }
}

impl ContentComparable for PrefixRecord {
    fn name(&self) -> &PackageName {
        &self.repodata_record.package_record.name
    }
    fn version(&self) -> &VersionWithSource {
        &self.repodata_record.package_record.version
    }
    fn build(&self) -> &str {
        &self.repodata_record.package_record.build
    }
    fn sha256(&self) -> Option<&Sha256Hash> {
        self.repodata_record.package_record.sha256.as_ref()
    }
    fn md5(&self) -> Option<&Md5Hash> {
        self.repodata_record.package_record.md5.as_ref()
    }
    fn size(&self) -> Option<u64> {
        self.repodata_record.package_record.size
    }
    fn noarch(&self) -> NoArchType {
        self.repodata_record.package_record.noarch
    }
    fn python_site_packages_path(&self) -> Option<&str> {
        self.repodata_record
            .package_record
            .python_site_packages_path
            .as_deref()
    }
}

impl ContentComparable for RepoDataRecord {
    fn name(&self) -> &PackageName {
        &self.package_record.name
    }
    fn version(&self) -> &VersionWithSource {
        &self.package_record.version
    }
    fn build(&self) -> &str {
        &self.package_record.build
    }
    fn sha256(&self) -> Option<&Sha256Hash> {
        self.package_record.sha256.as_ref()
    }
    fn md5(&self) -> Option<&Md5Hash> {
        self.package_record.md5.as_ref()
    }
    fn size(&self) -> Option<u64> {
        self.package_record.size
    }
    fn noarch(&self) -> NoArchType {
        self.package_record.noarch
    }
    fn python_site_packages_path(&self) -> Option<&str> {
        self.package_record.python_site_packages_path.as_deref()
    }
}

impl<T: ContentComparable> ContentComparable for &T {
    fn name(&self) -> &PackageName {
        (*self).name()
    }
    fn version(&self) -> &VersionWithSource {
        (*self).version()
    }
    fn build(&self) -> &str {
        (*self).build()
    }
    fn sha256(&self) -> Option<&Sha256Hash> {
        (*self).sha256()
    }
    fn md5(&self) -> Option<&Md5Hash> {
        (*self).md5()
    }
    fn size(&self) -> Option<u64> {
        (*self).size()
    }
    fn noarch(&self) -> NoArchType {
        T::noarch(self)
    }
    fn python_site_packages_path(&self) -> Option<&str> {
        T::python_site_packages_path(self)
    }
}
