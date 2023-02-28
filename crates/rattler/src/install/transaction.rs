use crate::install::python::PythonInfoError;
use crate::install::PythonInfo;
use rattler_conda_types::{PackageRecord, Platform};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    /// An error that happens if the python version could not be parsed.
    #[error(transparent)]
    PythonInfoError(#[from] PythonInfoError),
}

/// Describes an operation to perform
#[derive(Debug)]
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

    /// Reinstall a package. This can happen if the Python version changed in the environment, we
    /// need to relink all noarch python packages in that case.
    Reinstall(Old),

    /// Completely remove a package
    Remove(Old),
}

impl<Old: AsRef<New>, New> TransactionOperation<Old, New> {
    /// Returns the record of the package to install for this operation. If this operation does not
    /// refer to an installable package, `None` is returned.
    pub fn record_to_install(&self) -> Option<&New> {
        match self {
            TransactionOperation::Install(record) => Some(record),
            TransactionOperation::Change { new, .. } => Some(new),
            TransactionOperation::Reinstall(old) => Some(old.as_ref()),
            TransactionOperation::Remove(_) => None,
        }
    }
}

impl<Old, New> TransactionOperation<Old, New> {
    /// Returns the record of the package to remove for this operation. If this operation does not
    /// refer to an removable package, `None` is returned.
    pub fn record_to_remove(&self) -> Option<&Old> {
        match self {
            TransactionOperation::Install(_) => None,
            TransactionOperation::Change { old, .. } => Some(old),
            TransactionOperation::Reinstall(old) => Some(old),
            TransactionOperation::Remove(old) => Some(old),
        }
    }
}

/// Describes the operations to perform to bring an environment from one state into another.
pub struct Transaction<Old, New> {
    /// A list of operations to update an environment
    pub operations: Vec<TransactionOperation<Old, New>>,

    /// The python version of the target state, or None if python doesnt exist in the environment.
    pub python_info: Option<PythonInfo>,

    /// The target platform of the transaction
    pub platform: Platform,
}

impl<Old: AsRef<PackageRecord>, New: AsRef<PackageRecord>> Transaction<Old, New> {
    /// Constructs a [`Transaction`] by taking the current situation and diffing that against the
    /// desired situation.
    pub fn from_current_and_desired<
        CurIter: IntoIterator<Item = Old>,
        NewIter: IntoIterator<Item = New>,
    >(
        current: CurIter,
        desired: NewIter,
        platform: Platform,
    ) -> Result<Self, TransactionError>
    where
        CurIter::IntoIter: Clone,
        NewIter::IntoIter: Clone,
    {
        let current = current.into_iter();
        let desired = desired.into_iter();

        // Determine the python version used in the current situation.
        let current_python_info = find_python_info(current.clone(), platform)?;
        let desired_python_info = find_python_info(desired.clone(), platform)?;
        let needs_python_relink = match (&current_python_info, &desired_python_info) {
            (Some(current), Some(desired)) => desired.is_relink_required(current),
            _ => false,
        };

        // Create a lookup table by name for the desired packages.
        let mut desired: HashMap<String, New> = desired
            .into_iter()
            .map(|record| (record.as_ref().name.clone(), record))
            .collect();

        let mut operations = Vec::new();

        // Find all the elements that are no longer in the desired set
        for record in current {
            match desired.remove(&record.as_ref().name) {
                None => operations.push(TransactionOperation::Remove(record)),
                Some(desired) => {
                    // If the desired differs from the current it has to be updated.
                    if desired.as_ref() != record.as_ref() {
                        operations.push(TransactionOperation::Change {
                            old: record,
                            new: desired,
                        })
                    }
                    // If this is a noarch package and all python packages need to be relinked,
                    // reinstall the package completely.
                    else if desired.as_ref().noarch.is_python() && needs_python_relink {
                        operations.push(TransactionOperation::Reinstall(record));
                    }
                }
            }
        }

        // The remaining packages from the desired list need to be explicitly installed.
        for record in desired.into_values() {
            operations.push(TransactionOperation::Install(record))
        }

        Ok(Self {
            operations,
            python_info: desired_python_info,
            platform,
        })
    }
}

/// Determine the version of Python used by a set of packages. Returns `None` if none of the
/// packages refers to a Python installation.
fn find_python_info(
    records: impl IntoIterator<Item = impl AsRef<PackageRecord>>,
    platform: Platform,
) -> Result<Option<PythonInfo>, PythonInfoError> {
    records
        .into_iter()
        .find(|r| is_python_record(r.as_ref()))
        .map(|record| PythonInfo::from_version(&record.as_ref().version, platform))
        .map_or(Ok(None), |info| info.map(Some))
}

/// Returns true if the specified record refers to Python.
fn is_python_record(record: &PackageRecord) -> bool {
    record.name == "python"
}
