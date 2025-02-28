use std::collections::HashSet;

use rattler_conda_types::{PackageName, PackageRecord, Platform};

use crate::install::{python::PythonInfoError, PythonInfo};

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
    /// Includes old and new because certains fields like the channel/url may have changed between installations
    Reinstall {
        /// The old record to remove
        old: Old,
        /// The new record to install
        new: New,
    },

    /// Completely remove a package
    Remove(Old),
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
}

impl<Old, New> Transaction<Old, New> {
    /// Return an iterator over the prefix records of all packages that are
    /// going to be removed.
    pub fn removed_packages(&self) -> impl Iterator<Item = &Old> + '_ {
        self.operations
            .iter()
            .filter_map(TransactionOperation::record_to_remove)
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

impl<Old: AsRef<PackageRecord>, New: AsRef<PackageRecord>> Transaction<Old, New> {
    /// Constructs a [`Transaction`] by taking the current situation and diffing
    /// that against the desired situation. You can specify a set of package names
    /// that should be reinstalled even if their content has not changed.
    pub fn from_current_and_desired<
        CurIter: IntoIterator<Item = Old>,
        NewIter: IntoIterator<Item = New>,
    >(
        current: CurIter,
        desired: NewIter,
        reinstall: Option<HashSet<PackageName>>,
        platform: Platform,
    ) -> Result<Self, TransactionError>
    where
        CurIter::IntoIter: Clone,
        NewIter::IntoIter: Clone,
    {
        let current_iter = current.into_iter();
        let desired_iter = desired.into_iter();

        // Determine the python version used in the current situation.
        let current_python_info = find_python_info(current_iter.clone(), platform)?;
        let desired_python_info = find_python_info(desired_iter.clone(), platform)?;
        let needs_python_relink = match (&current_python_info, &desired_python_info) {
            (Some(current), Some(desired)) => desired.is_relink_required(current),
            _ => false,
        };

        let mut operations = Vec::new();
        let reinstall = reinstall.unwrap_or_default();

        let mut current_map = current_iter
            .clone()
            .map(|r| (r.as_ref().name.clone(), r))
            .collect::<std::collections::HashMap<_, _>>();

        let desired_names = desired_iter
            .clone()
            .map(|r| r.as_ref().name.clone())
            .collect::<HashSet<_>>();

        // Remove all current packages that are not in desired (but keep order of
        // current)
        for record in current_iter {
            if !desired_names.contains(&record.as_ref().name) {
                operations.push(TransactionOperation::Remove(record));
            }
        }

        // reverse all removals, last in first out
        operations.reverse();

        // Figure out the operations to perform, but keep the order of the original
        // "desired" iterator
        for record in desired_iter {
            let name = &record.as_ref().name;
            let old_record = current_map.remove(name);

            if let Some(old_record) = old_record {
                if !describe_same_content(record.as_ref(), old_record.as_ref())
                    || reinstall.contains(&record.as_ref().name)
                {
                    // if the content changed, we need to reinstall (remove and install)
                    operations.push(TransactionOperation::Change {
                        old: old_record,
                        new: record,
                    });
                } else if needs_python_relink && old_record.as_ref().noarch.is_python() {
                    // when the python version changed, we need to relink all noarch packages
                    // to recompile the bytecode
                    operations.push(TransactionOperation::Reinstall {
                        old: old_record,
                        new: record,
                    });
                }
                // if the content is the same, we dont need to do anything
            } else {
                operations.push(TransactionOperation::Install(record));
            }
        }

        Ok(Self {
            operations,
            python_info: desired_python_info,
            current_python_info,
            platform,
        })
    }
}

/// Determine the version of Python used by a set of packages. Returns `None` if
/// none of the packages refers to a Python installation.
fn find_python_info(
    records: impl IntoIterator<Item = impl AsRef<PackageRecord>>,
    platform: Platform,
) -> Result<Option<PythonInfo>, PythonInfoError> {
    records
        .into_iter()
        .find(|r| is_python_record(r.as_ref()))
        .map(|record| PythonInfo::from_python_record(record.as_ref(), platform))
        .map_or(Ok(None), |info| info.map(Some))
}

/// Returns true if the specified record refers to Python.
fn is_python_record(record: &PackageRecord) -> bool {
    record.name.as_normalized() == "python"
}

/// Returns true if the `from` and `to` describe the same package content
fn describe_same_content(from: &PackageRecord, to: &PackageRecord) -> bool {
    // If one hash is set and the other is not, the packages are different
    if from.sha256.is_some() != to.sha256.is_some() || from.md5.is_some() != to.md5.is_some() {
        return false;
    }

    // If the hashes of the packages match we consider them to be equal
    if let (Some(a), Some(b)) = (from.sha256.as_ref(), to.sha256.as_ref()) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (from.md5.as_ref(), to.md5.as_ref()) {
        return a == b;
    }

    // If the size doesnt match, the contents must be different
    if matches!((from.size.as_ref(), to.size.as_ref()), (Some(a), Some(b)) if a == b) {
        return false;
    }

    // Otherwise, just check that the name, version and build string match
    from.name == to.name && from.version == to.version && from.build == to.build
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use rattler_conda_types::Platform;

    use crate::install::{
        test_utils::download_and_get_prefix_record, Transaction, TransactionOperation,
    };
    use assert_matches::assert_matches;

    #[tokio::test]
    async fn test_reinstall_package() {
        let environment_dir = tempfile::TempDir::new().unwrap();
        let prefix_record = download_and_get_prefix_record(
            environment_dir.path(),
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
            Some(HashSet::from_iter(vec![name])),
            Platform::current(),
        )
        .unwrap();

        assert_matches!(
            transaction.operations[0],
            TransactionOperation::Change { .. }
        );
    }
}
