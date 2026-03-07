use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use rattler_conda_types::{PackageName, RepoDataRecord};

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
    /// All package names present in this source, keyed by channel URL.
    /// Includes names whose records were filtered out by version constraints.
    pub(crate) channel_package_names: HashMap<Option<String>, HashSet<PackageName>>,
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

    /// Returns the package names present per channel in this source,
    /// including names whose records were filtered out by version constraints.
    pub fn channel_package_names(&self) -> &HashMap<Option<String>, HashSet<PackageName>> {
        &self.channel_package_names
    }

    /// Build a list of `(channel, package_names)` entries from a sequence of
    /// [`RepoData`] results, preserving priority order (first entry = highest
    /// priority). Channels that appear in multiple entries are merged.
    pub fn collect_channel_package_names(
        all: &[RepoData],
    ) -> Vec<(Option<String>, HashSet<PackageName>)> {
        let mut result: Vec<(Option<String>, HashSet<PackageName>)> = Vec::new();
        let mut channel_index: HashMap<Option<String>, usize> = HashMap::new();
        for rd in all {
            for (channel, names) in &rd.channel_package_names {
                if let Some(&idx) = channel_index.get(channel) {
                    result[idx].1.extend(names.iter().cloned());
                } else {
                    let idx = result.len();
                    channel_index.insert(channel.clone(), idx);
                    result.push((channel.clone(), names.clone()));
                }
            }
        }
        result
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
