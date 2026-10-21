use rattler_conda_types::PackageRecord;

use super::CacheKey;

/// Package identity and optional metadata for URL-based cache fetching.
///
/// Pass a [`PackageRecord`] when available. Wheels require its metadata to
/// generate conda package metadata. Conda archives already contain that metadata,
/// so direct-URL callers can supply only a [`CacheKey`].
#[derive(Debug)]
pub enum PackageCacheInput<'a> {
    /// Identity and hashes for a conda archive whose metadata is not yet known.
    Key(CacheKey),
    /// Package metadata, also used to derive the cache key and expected hashes.
    Record(&'a PackageRecord),
}

impl From<CacheKey> for PackageCacheInput<'_> {
    fn from(key: CacheKey) -> Self {
        Self::Key(key)
    }
}

impl<'a> From<&'a PackageRecord> for PackageCacheInput<'a> {
    fn from(record: &'a PackageRecord) -> Self {
        Self::Record(record)
    }
}
