use super::ffi;
use super::pool::PoolRef;
use super::solvable::{Solvable, SolvableId};
use crate::libsolv::repo::RepoId;
use crate::package_operation::{PackageOperation, PackageOperationKind};
use rattler_conda_types::{NoArchType, PackageRecord, RepoDataRecord, Version};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::str::FromStr;
use url::Url;

/// Wraps a pointer to an `ffi::Transaction` which is freed when the instance is dropped.
#[repr(transparent)]
struct TransactionOwnedPtr(NonNull<ffi::Transaction>);

impl Drop for TransactionOwnedPtr {
    fn drop(&mut self) {
        // Safe because the pointer must not be null
        unsafe { ffi::transaction_free(self.0.as_mut()) }
    }
}

/// This represents a transaction in libsolv which is a abstraction over changes that need to be
/// done to satisfy the dependency constraint.
pub struct Transaction<'solver>(TransactionOwnedPtr, PhantomData<&'solver ffi::Transaction>);

/// A `TransactionRef` is a wrapper around an `ffi::Transaction` that provides a safe abstraction
/// over its functionality.
///
/// A `TransactionRef` can not be constructed by itself but is instead returned by dereferencing a
/// [`Transaction`].
#[repr(transparent)]
pub struct TransactionRef(ffi::Transaction);

impl Deref for Transaction<'_> {
    type Target = TransactionRef;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0 .0.cast().as_ref() }
    }
}

impl DerefMut for Transaction<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0 .0.cast().as_mut() }
    }
}

impl TransactionRef {
    /// Returns a pointer to the wrapped `ffi::Transaction`
    fn as_ptr(&self) -> NonNull<ffi::Transaction> {
        // Safe because a `TransactionRef` is a transparent wrapper around `ffi::Transaction`
        unsafe { NonNull::new_unchecked(self as *const Self as *mut Self).cast() }
    }

    /// Returns a reference to the wrapped `ffi::Transaction`.
    fn as_ref(&self) -> &ffi::Transaction {
        // Safe because a `TransactionRef` is a transparent wrapper around `ffi::Transaction`
        unsafe { std::mem::transmute(self) }
    }

    /// Returns the pool that owns this instance.
    pub fn pool(&self) -> &PoolRef {
        // Safe because a `PoolRef` is a wrapper around `ffi::Pool`
        unsafe { &*(self.as_ref().pool as *const PoolRef) }
    }

    /// Returns the package operations derived from the transaction
    ///
    /// If the transaction contains libsolv operations that have no mapping to `PackageOperation`,
    /// an error is returned containing their ids
    pub fn get_package_operations(
        &mut self,
        channel_mapping: &HashMap<RepoId, String>,
    ) -> Result<Vec<PackageOperation>, Vec<ffi::Id>> {
        let mut solvable_operations = Vec::new();
        let mut unsupported_operations = Vec::new();

        // Get inner transaction type
        let inner = self.as_ref();
        // Number of transaction details
        let count = inner.steps.count as usize;

        // TODO: simplify unsafe usage and explain why it is all right
        for index in 0..count {
            unsafe {
                // Get the id for the current solvable
                // Safe because we don't go past the count
                let raw_id = *inner.steps.elements.add(index);
                let id = SolvableId(raw_id);
                // Get the transaction type
                let id_type = ffi::transaction_type(
                    self.as_ptr().as_ptr(),
                    id.into(),
                    ffi::SOLVER_TRANSACTION_SHOW_ALL as std::os::raw::c_int,
                );

                let solvable = id.resolve(self.pool());
                match id_type as u32 {
                    ffi::SOLVER_TRANSACTION_DOWNGRADED
                    | ffi::SOLVER_TRANSACTION_UPGRADED
                    | ffi::SOLVER_TRANSACTION_CHANGED => {
                        let solvable_offset =
                            ffi::transaction_obs_pkg(self.as_ptr().as_ptr(), raw_id);
                        let new_solvable = SolvableId(solvable_offset);

                        solvable_operations.push(PackageOperation {
                            package: repodata_record_from_solvable(solvable, channel_mapping),
                            kind: PackageOperationKind::Remove,
                        });

                        solvable_operations.push(PackageOperation {
                            package: repodata_record_from_solvable(
                                new_solvable.resolve(self.pool()),
                                channel_mapping,
                            ),
                            kind: PackageOperationKind::Install,
                        });
                    }
                    ffi::SOLVER_TRANSACTION_REINSTALLED => {
                        solvable_operations.push(PackageOperation {
                            package: repodata_record_from_solvable(solvable, channel_mapping),
                            kind: PackageOperationKind::Reinstall,
                        });
                    }
                    ffi::SOLVER_TRANSACTION_INSTALL => {
                        solvable_operations.push(PackageOperation {
                            package: repodata_record_from_solvable(solvable, channel_mapping),
                            kind: PackageOperationKind::Install,
                        });
                    }
                    ffi::SOLVER_TRANSACTION_ERASE => {
                        solvable_operations.push(PackageOperation {
                            package: repodata_record_from_solvable(solvable, channel_mapping),
                            kind: PackageOperationKind::Remove,
                        });
                    }
                    ffi::SOLVER_TRANSACTION_IGNORE => {}
                    _ => {
                        unsupported_operations.push(id_type);
                    }
                }
            };
        }

        if !unsupported_operations.is_empty() {
            return Err(unsupported_operations);
        }

        Ok(solvable_operations)
    }
}

fn repodata_record_from_solvable(
    solvable: Solvable,
    channel_mapping: &HashMap<RepoId, String>,
) -> RepoDataRecord {
    let channel = channel_mapping
        .get(&solvable.repo().id())
        .map(|c| c.to_owned())
        .unwrap_or_else(|| "unknown".to_owned());

    RepoDataRecord {
        // TODO: if we want this we will probably need to add it separately to the solvable
        file_name: "unknown".to_string(),
        // TODO: if we want this we will probably need to add it separately to the solvable
        url: Url::from_str("https://example.com").unwrap(),
        channel,
        package_record: PackageRecord {
            name: solvable.name(),
            version: Version::from_str(&solvable.version()).unwrap(),
            build_number: solvable.build_number().unwrap(),
            build: solvable.build_string().unwrap(),
            // TODO: if we want this we will probably need to add it separately to the solvable
            subdir: "unknown".to_string(),
            md5: solvable.md5(),
            sha256: solvable.sha256(),
            // We have the data (see [`Repo::add_repodata_records`]), but I'm not sure if it is necessary to retrieve it
            depends: Vec::new(),
            constrains: Vec::new(),
            track_features: Vec::new(),
            timestamp: None,
            license: None,
            size: None,
            // Not sure where to get the following from (if it is even necessary), so using None for now
            arch: None,
            platform: None,
            features: None,
            noarch: NoArchType::default(),
            license_family: None,
        },
    }
}

impl Transaction<'_> {
    /// Constructs a new instance
    pub(super) fn new(ptr: NonNull<ffi::Transaction>) -> Self {
        Transaction(TransactionOwnedPtr(ptr), PhantomData::default())
    }
}
