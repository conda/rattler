use std::sync::Arc;

use rattler_conda_types::RepoDataRecord;

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
}

impl RepoData {
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
