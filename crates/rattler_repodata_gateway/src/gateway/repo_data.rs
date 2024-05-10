use rattler_conda_types::RepoDataRecord;
use std::iter::FusedIterator;
use std::sync::Arc;

/// A container for [`RepoDataRecord`]s that are returned from the [`super::Gateway`].
///
/// This struct references the same memory as the `Gateway` therefor not
/// duplicating the records in memory.
///
/// `RepoData` uses internal reference counting, therefor it is relatively
/// cheap to clone.
#[derive(Debug, Default, Clone)]
pub struct RepoData {
    pub(crate) shards: Vec<Arc<[RepoDataRecord]>>,
    pub(crate) len: usize,
}

impl RepoData {
    /// Returns an iterator over all the records in this instance.
    pub fn iter(&self) -> RepoDataIterator<'_> {
        RepoDataIterator {
            records: self,
            shard_idx: 0,
            record_idx: 0,
            total: 0,
        }
    }

    /// Returns the total number of records stored in this instance.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if there are no records stored in this instance.
    pub fn is_empty(&self) -> bool {
        self.len == 0
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
    records: &'r RepoData,
    shard_idx: usize,
    record_idx: usize,
    total: usize,
}

impl<'r> Iterator for RepoDataIterator<'r> {
    type Item = &'r RepoDataRecord;

    fn next(&mut self) -> Option<Self::Item> {
        while self.shard_idx < self.records.shards.len() {
            let shard = &self.records.shards[self.shard_idx];
            if self.record_idx < shard.len() {
                let record = &shard[self.record_idx];
                self.record_idx += 1;
                self.total += 1;
                return Some(record);
            } else {
                self.shard_idx += 1;
                self.record_idx = 0;
            }
        }

        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.records.len - self.total;
        (remaining, Some(remaining))
    }
}

impl FusedIterator for RepoDataIterator<'_> {}

impl ExactSizeIterator for RepoDataIterator<'_> {
    fn len(&self) -> usize {
        self.records.len - self.total
    }
}
