use std::collections::{HashMap, HashSet};

use rattler_conda_types::{
    MinimalPrefixRecord, NoArchType, PackageName, PackageRecord, Platform, PrefixRecord,
    RepoDataRecord, VersionWithSource,
};
use rattler_digest::{Md5Hash, Sha256Hash};

use crate::install::{python::PythonInfoError, PythonInfo};

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

// Blanket implementation for references
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

/// Error that occurred during creation of a Transaction
#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    /// An error that happens if the python version could not be parsed.
    #[error(transparent)]
    PythonInfoError(#[from] PythonInfoError),

    /// The operation was cancelled
    #[error("the operation was cancelled")]
    Cancelled,
}

/// Describes an operation to perform
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionOperation<Old, New> {
    /// The given package record should be installed
    Install(New),

    /// Remove an old package and install another
    Change {
        /// The old record to remove
        old: Old,

        /// The new record to install
        new: New,
    },

    /// Reinstall a package. This can happen if the Python version changed in
    /// the environment, we need to relink all noarch python packages in
    /// that case.
    /// Includes old and new because certain fields like the channel/url may
    /// have changed between installations
    Reinstall {
        /// The old record to remove
        old: Old,
        /// The new record to install
        new: New,
    },

    /// Completely remove a package
    Remove(Old),
}

impl<Old: Clone, New: Clone> TransactionOperation<&Old, &New> {
    /// Own records.
    pub fn to_owned(self) -> TransactionOperation<Old, New> {
        match self {
            TransactionOperation::Install(new) => TransactionOperation::Install(new.clone()),
            TransactionOperation::Change { old, new } => TransactionOperation::Change {
                old: old.clone(),
                new: new.clone(),
            },
            TransactionOperation::Reinstall { old, new } => TransactionOperation::Reinstall {
                old: old.clone(),
                new: new.clone(),
            },
            TransactionOperation::Remove(old) => TransactionOperation::Remove(old.clone()),
        }
    }
}

impl<Old: AsRef<New>, New> TransactionOperation<Old, New> {
    /// Returns the record of the package to install for this operation. If this
    /// operation does not refer to an installable package, `None` is
    /// returned.
    pub fn record_to_install(&self) -> Option<&New> {
        match self {
            TransactionOperation::Install(record) => Some(record),
            TransactionOperation::Change { new, .. }
            | TransactionOperation::Reinstall { new, .. } => Some(new),
            TransactionOperation::Remove(_) => None,
        }
    }
}

impl<Old, New> TransactionOperation<Old, New> {
    /// Returns the record of the package to remove for this operation. If this
    /// operation does not refer to an removable package, `None` is
    /// returned.
    pub fn record_to_remove(&self) -> Option<&Old> {
        match self {
            TransactionOperation::Install(_) => None,
            TransactionOperation::Change { old, .. }
            | TransactionOperation::Reinstall { old, new: _ }
            | TransactionOperation::Remove(old) => Some(old),
        }
    }
}

/// Describes the operations to perform to bring an environment from one state
/// into another.
#[derive(Debug)]
pub struct Transaction<Old, New> {
    /// A list of operations to update an environment
    pub operations: Vec<TransactionOperation<Old, New>>,

    /// The python version of the target state, or None if python doesnt exist
    /// in the environment.
    pub python_info: Option<PythonInfo>,

    /// The python version of the current state, or None if python didnt exist
    /// in the previous environment.
    pub current_python_info: Option<PythonInfo>,

    /// The target platform of the transaction
    pub platform: Platform,

    /// The records that are not touched by the transaction.
    pub unchanged: Vec<Old>,
}

impl<Old: Clone, New: Clone> Transaction<&Old, &New> {
    /// Own records.
    pub fn to_owned(self) -> Transaction<Old, New> {
        Transaction {
            operations: self
                .operations
                .into_iter()
                .map(TransactionOperation::to_owned)
                .collect(),
            python_info: self.python_info,
            current_python_info: self.current_python_info,
            platform: self.platform,
            unchanged: self.unchanged.into_iter().cloned().collect(),
        }
    }
}

impl<Old, New> Transaction<Old, New> {
    /// Return an iterator over the prefix records of all packages that are
    /// going to be removed.
    pub fn removed_packages(&self) -> impl Iterator<Item = &Old> + '_ {
        self.operations
            .iter()
            .filter_map(TransactionOperation::record_to_remove)
    }

    /// Return an iterator over the records that are not touched by the
    /// transaction
    pub fn unchanged_packages(&self) -> &[Old] {
        &self.unchanged
    }

    /// Returns the number of records to install.
    pub fn packages_to_uninstall(&self) -> usize {
        self.removed_packages().count()
    }
}

impl<Old: AsRef<New>, New> Transaction<Old, New> {
    /// Return an iterator over the prefix records of all packages that are
    /// going to be installed.
    pub fn installed_packages(&self) -> impl Iterator<Item = &New> + '_ {
        self.operations
            .iter()
            .filter_map(TransactionOperation::record_to_install)
    }

    /// Returns the number of records to install.
    pub fn packages_to_install(&self) -> usize {
        self.installed_packages().count()
    }
}

impl<Old, New> Transaction<Old, New>
where
    Old: ContentComparable,
    New: ContentComparable,
{
    /// Constructs a [`Transaction`] by taking the current situation and diffing
    /// that against the desired situation. You can specify a set of package
    /// names that should be reinstalled even if their content has not
    /// changed. You can also specify a set of package names that should be
    /// ignored (left untouched).
    pub fn from_current_and_desired<
        CurIter: IntoIterator<Item = Old>,
        NewIter: IntoIterator<Item = New>,
    >(
        current: CurIter,
        desired: NewIter,
        reinstall: Option<&HashSet<PackageName>>,
        ignored: Option<&HashSet<PackageName>>,
        platform: Platform,
    ) -> Result<Self, TransactionError> {
        let current_packages = current.into_iter().collect::<Vec<_>>();
        let desired_packages = desired.into_iter().collect::<Vec<_>>();

        // Determine the python version used in the current situation.
        let current_python_info = find_python_info(&current_packages, platform)?;
        let desired_python_info = find_python_info(&desired_packages, platform)?;
        let needs_python_relink = match (&current_python_info, &desired_python_info) {
            (Some(current), Some(desired)) => desired.is_relink_required(current),
            _ => false,
        };

        let mut operations = Vec::new();

        let empty_hashset = HashSet::new();
        let reinstall = reinstall.unwrap_or(&empty_hashset);
        let ignored = ignored.unwrap_or(&empty_hashset);

        let desired_names = desired_packages
            .iter()
            .map(|r| r.name().clone())
            .collect::<HashSet<_>>();

        // Remove all current packages that are not in desired (but keep order of
        // current), except for ignored packages which should be left untouched
        let mut unchanged = Vec::new();
        let mut current_map = HashMap::new();
        for record in current_packages {
            let package_name = record.name();
            if desired_names.contains(package_name) {
                // The record is desired. Keep it in the map so we can compare it to the desired record later.
                current_map.insert(record.name().clone(), record);
            } else {
                // The record is not desired.
                if ignored.contains(package_name) {
                    // But we want to ignore it, so we keep it unchanged.
                    unchanged.push(record);
                } else {
                    // Otherwise we have to remove it.
                    operations.push(TransactionOperation::Remove(record));
                }
            }
        }

        // reverse all removals, last in first out
        operations.reverse();

        // Figure out the operations to perform, but keep the order of the original
        // "desired" iterator. Skip ignored packages entirely.
        for record in desired_packages {
            let name = record.name();
            let old_record = current_map.remove(name);

            // Skip ignored packages - they should be left in their current state
            if ignored.contains(name) {
                if let Some(old_record) = old_record {
                    unchanged.push(old_record);
                }
            } else if let Some(old_record) = old_record {
                if !describe_same_content(&record, &old_record) || reinstall.contains(record.name())
                {
                    // if the content changed, we need to reinstall (remove and install)
                    operations.push(TransactionOperation::Change {
                        old: old_record,
                        new: record,
                    });
                } else if needs_python_relink && old_record.noarch().is_python() {
                    // when the python version changed, we need to relink all noarch packages
                    // to recompile the bytecode
                    operations.push(TransactionOperation::Reinstall {
                        old: old_record,
                        new: record,
                    });
                } else {
                    // if the content is the same, we dont need to do anything
                    unchanged.push(old_record);
                }
            } else {
                operations.push(TransactionOperation::Install(record));
            }
        }

        Ok(Self {
            operations,
            python_info: desired_python_info,
            current_python_info,
            platform,
            unchanged,
        })
    }
}

/// Determine the version of Python used by a set of packages. Returns `None` if
/// none of the packages refers to a Python installation.
fn find_python_info(
    records: impl IntoIterator<Item = impl ContentComparable>,
    platform: Platform,
) -> Result<Option<PythonInfo>, PythonInfoError> {
    records
        .into_iter()
        .find(|r| r.name().as_normalized() == "python")
        .map(|record| {
            PythonInfo::from_version(
                record.version(),
                record.python_site_packages_path(),
                platform,
            )
        })
        .map_or(Ok(None), |info| info.map(Some))
}

/// Returns true if the `from` and `to` describe the same package content
fn describe_same_content<T: ContentComparable, U: ContentComparable>(from: &T, to: &U) -> bool {
    // If one hash is set and the other is not, the packages are different
    if from.sha256().is_some() != to.sha256().is_some() {
        return false;
    }

    // If the hashes of the packages match we consider them to be equal
    if let (Some(a), Some(b)) = (from.sha256(), to.sha256()) {
        return a == b;
    }

    if from.md5().is_some() != to.md5().is_some() {
        return false;
    }

    if let (Some(a), Some(b)) = (from.md5(), to.md5()) {
        return a == b;
    }

    // If the size doesn't match, the contents must be different
    if matches!((from.size(), to.size()), (Some(a), Some(b)) if a != b) {
        return false;
    }

    // Otherwise, just check that the name, version and build string match
    from.name() == to.name() && from.version() == to.version() && from.build() == to.build()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use assert_matches::assert_matches;
    use rattler_conda_types::{prefix::Prefix, Platform};

    use crate::install::{
        test_utils::download_and_get_prefix_record, Transaction, TransactionOperation,
    };

    #[tokio::test]
    async fn test_reinstall_package() {
        let environment_dir = tempfile::TempDir::new().unwrap();
        let prefix_record = download_and_get_prefix_record(
            &Prefix::create(environment_dir.path()).unwrap(),
            "https://conda.anaconda.org/conda-forge/win-64/ruff-0.0.171-py310h298983d_0.conda"
                .parse()
                .unwrap(),
            "25c755b97189ee066576b4ae3999d5e7ff4406d236b984742194e63941838dcd",
        )
        .await;
        let name = prefix_record.repodata_record.package_record.name.clone();

        let transaction = Transaction::from_current_and_desired(
            vec![prefix_record.clone()],
            vec![prefix_record.clone()],
            Some(&HashSet::from_iter(vec![name])),
            None, // ignored packages
            Platform::current(),
        )
        .unwrap();

        assert_matches!(
            transaction.operations[0],
            TransactionOperation::Change { .. }
        );
    }

    #[tokio::test]
    async fn test_ignored_packages() {
        let environment_dir = tempfile::TempDir::new().unwrap();
        let prefix_record = download_and_get_prefix_record(
            &Prefix::create(environment_dir.path()).unwrap(),
            "https://conda.anaconda.org/conda-forge/win-64/ruff-0.0.171-py310h298983d_0.conda"
                .parse()
                .unwrap(),
            "25c755b97189ee066576b4ae3999d5e7ff4406d236b984742194e63941838dcd",
        )
        .await;

        let name = prefix_record.repodata_record.package_record.name.clone();

        // Test case 1: Package is in both current and desired, but ignored - should
        // result in no operations
        let ignored_packages = Some(HashSet::from_iter(vec![name.clone()]));
        let transaction = Transaction::from_current_and_desired(
            vec![prefix_record.clone()],
            vec![prefix_record.repodata_record.clone()],
            None, // reinstall
            ignored_packages.as_ref(),
            Platform::current(),
        )
        .unwrap();

        // Should have no operations because the package is ignored
        assert!(transaction.operations.is_empty());

        // Test case 2: Package is in current but not desired, and ignored - should not
        // be removed
        let ignored_packages = Some(HashSet::from_iter(vec![name.clone()]));
        let transaction = Transaction::from_current_and_desired(
            vec![prefix_record.clone()],
            Vec::<rattler_conda_types::RepoDataRecord>::new(), // empty desired
            None,                                              // reinstall
            ignored_packages.as_ref(),
            Platform::current(),
        )
        .unwrap();

        // Should have no operations because the package is ignored (not removed)
        assert!(transaction.operations.is_empty());

        // Test case 3: Package is not in current but in desired, and ignored - should
        // not be installed
        let ignored_packages = Some(HashSet::from_iter(vec![name.clone()]));
        let transaction = Transaction::from_current_and_desired(
            Vec::<rattler_conda_types::PrefixRecord>::new(), // empty current
            vec![prefix_record.repodata_record.clone()],
            None, // reinstall
            ignored_packages.as_ref(),
            Platform::current(),
        )
        .unwrap();

        // Should have no operations because the package is ignored (not installed)
        assert!(transaction.operations.is_empty());
    }
}
