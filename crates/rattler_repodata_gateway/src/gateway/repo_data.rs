use std::sync::Arc;

use ahash::HashMap;
use rattler_conda_types::RepoDataRecord;
use url::Url;

use crate::sparse::RemovedPackage;

/// A container for [`RepoDataRecord`]s that are returned from the [`super::Gateway`].
///
/// Records are stored as `Arc<RepoDataRecord>` so that cloning is cheap
/// (reference count bump rather than deep copy).
///
/// `RepoData` uses internal reference counting, therefor it is relatively
/// cheap to clone.
#[derive(Debug, Default, Clone)]
pub struct RepoData {
    pub(crate) records: Vec<Arc<RepoDataRecord>>,
    pub(crate) removed: RemovedPackages,
}

impl RepoData {
    /// Returns the packages that the source lists as removed for the package
    /// names the query fetched.
    ///
    /// Removed packages are never part of [`Self::iter`]. The match specs of
    /// the query do not filter this collection: every removed entry of a
    /// fetched package name is included, so callers look up the URL they are
    /// interested in with [`RemovedPackages::contains`] or
    /// [`RemovedPackages::get`].
    pub fn removed(&self) -> &RemovedPackages {
        &self.removed
    }

    /// Returns an iterator over all the records in this instance.
    pub fn iter(&self) -> RepoDataIterator<'_> {
        RepoDataIterator {
            inner: self.records.iter(),
        }
    }

    /// Returns the total number of records stored in this instance.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Returns true if there are no records stored in this instance.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Returns an iterator over the Arc-wrapped records.
    ///
    /// This is useful when you want to clone records cheaply (Arc clone
    /// instead of deep copy).
    pub fn iter_arc(&self) -> impl ExactSizeIterator<Item = &Arc<RepoDataRecord>> {
        self.records.iter()
    }

    /// Returns true if there is at least one [`RepoDataRecord`] with
    /// [`rattler_conda_types::package::RunExportsJson`] missing.
    pub fn is_run_exports_missing(&self) -> bool {
        self.iter().any(|r| r.package_record.run_exports.is_none())
    }

    /// Returns number of missing run exports from the underlying
    /// [`RepoDataRecord`]s.
    pub fn run_exports_missing(&self) -> usize {
        self.iter()
            .filter(|r| r.package_record.run_exports.is_none())
            .count()
    }
}

impl<'r> IntoIterator for &'r RepoData {
    type Item = &'r RepoDataRecord;
    type IntoIter = RepoDataIterator<'r>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// The [`RemovedPackage`]s of a [`RepoData`] instance, indexed by their URL so
/// that a lock file entry or a previously fetched record can be checked in
/// constant time.
#[derive(Debug, Default, Clone)]
pub struct RemovedPackages {
    by_url: HashMap<Url, RemovedPackage>,
}

impl RemovedPackages {
    /// Returns true if the package served from `url` is listed as removed.
    pub fn contains(&self, url: &Url) -> bool {
        self.by_url.contains_key(url)
    }

    /// Returns the removed package that was served from `url`, if any.
    pub fn get(&self, url: &Url) -> Option<&RemovedPackage> {
        self.by_url.get(url)
    }

    /// Returns an iterator over all removed packages in no particular order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &RemovedPackage> {
        self.by_url.values()
    }

    /// Returns the number of removed packages.
    pub fn len(&self) -> usize {
        self.by_url.len()
    }

    /// Returns true if no packages are listed as removed.
    pub fn is_empty(&self) -> bool {
        self.by_url.is_empty()
    }

    pub(crate) fn extend(&mut self, removed: impl IntoIterator<Item = RemovedPackage>) {
        self.by_url.extend(
            removed
                .into_iter()
                .map(|package| (package.url.clone(), package)),
        );
    }
}

impl<'r> IntoIterator for &'r RemovedPackages {
    type Item = &'r RemovedPackage;
    type IntoIter = std::collections::hash_map::Values<'r, Url, RemovedPackage>;

    fn into_iter(self) -> Self::IntoIter {
        self.by_url.values()
    }
}

/// An iterator over the records in a [`RepoData`] instance.
pub struct RepoDataIterator<'r> {
    inner: std::slice::Iter<'r, Arc<RepoDataRecord>>,
}

impl<'r> Iterator for RepoDataIterator<'r> {
    type Item = &'r RepoDataRecord;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(std::convert::AsRef::as_ref)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl std::iter::FusedIterator for RepoDataIterator<'_> {}

impl ExactSizeIterator for RepoDataIterator<'_> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}
