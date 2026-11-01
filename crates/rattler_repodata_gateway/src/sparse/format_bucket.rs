//! Classification of package files by archive format, shared by every
//! repodata source.
//!
//! Files that share an
//! [`ArchiveIdentifier`](rattler_conda_types::package::ArchiveIdentifier)
//! (the `name-version-build` stem) are the same build in different archive
//! formats. After removed files are dropped and `v3` entries have replaced
//! legacy entries of the same file, every remaining file of a package falls
//! into exactly one [`FormatBucket`]. A [`PackageFormatSelection`] is a fixed
//! set of buckets, so every source that classifies its files this way returns
//! the same records for the same selection.

use std::iter::FusedIterator;
#[cfg(feature = "gateway")]
use std::{
    array,
    iter::Zip,
    ops::{Index, IndexMut},
};

use rattler_conda_types::package::{CondaArchiveType, DistArchiveType, WheelArchiveType};

use super::PackageFormatSelection;

/// The class of a package file, determined by its archive format and by which
/// other formats of the same build are available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormatBucket {
    /// A `.conda` file.
    Conda,
    /// A `.whl` file of a build that has no `.conda` file.
    WhlAlone,
    /// A `.whl` file of a build that also has a `.conda` file.
    WhlBehindConda,
    /// A `.tar.bz2` file of a build that has neither a `.conda` nor a `.whl`
    /// file.
    TarAlone,
    /// A `.tar.bz2` file of a build that has a `.whl` but no `.conda` file.
    TarBehindWhl,
    /// A `.tar.bz2` file of a build that also has a `.conda` file.
    TarBehindConda,
}

impl FormatBucket {
    /// Every bucket, in the order used by [`FormatBucketMap`].
    pub(crate) const ALL: [FormatBucket; 6] = [
        FormatBucket::Conda,
        FormatBucket::WhlAlone,
        FormatBucket::WhlBehindConda,
        FormatBucket::TarAlone,
        FormatBucket::TarBehindWhl,
        FormatBucket::TarBehindConda,
    ];

    /// Classifies a file with the given archive type. `has_twin` reports
    /// whether the same build is also available in another archive type; it
    /// is only consulted for the formats that matter for this file.
    pub(crate) fn classify(
        archive_type: DistArchiveType,
        mut has_twin: impl FnMut(DistArchiveType) -> bool,
    ) -> Self {
        let conda = DistArchiveType::Conda(CondaArchiveType::Conda);
        let whl = DistArchiveType::Wheel(WheelArchiveType::Whl);
        match archive_type {
            DistArchiveType::Conda(CondaArchiveType::Conda) => FormatBucket::Conda,
            DistArchiveType::Wheel(WheelArchiveType::Whl) => {
                if has_twin(conda) {
                    FormatBucket::WhlBehindConda
                } else {
                    FormatBucket::WhlAlone
                }
            }
            DistArchiveType::Conda(CondaArchiveType::TarBz2) => {
                if has_twin(conda) {
                    FormatBucket::TarBehindConda
                } else if has_twin(whl) {
                    FormatBucket::TarBehindWhl
                } else {
                    FormatBucket::TarAlone
                }
            }
        }
    }

    /// The archive type of every file in this bucket.
    pub(crate) fn archive_type(self) -> DistArchiveType {
        match self {
            FormatBucket::Conda => CondaArchiveType::Conda.into(),
            FormatBucket::WhlAlone | FormatBucket::WhlBehindConda => WheelArchiveType::Whl.into(),
            FormatBucket::TarAlone | FormatBucket::TarBehindWhl | FormatBucket::TarBehindConda => {
                CondaArchiveType::TarBz2.into()
            }
        }
    }

    /// The position of this bucket in [`Self::ALL`].
    const fn index(self) -> usize {
        match self {
            FormatBucket::Conda => 0,
            FormatBucket::WhlAlone => 1,
            FormatBucket::WhlBehindConda => 2,
            FormatBucket::TarAlone => 3,
            FormatBucket::TarBehindWhl => 4,
            FormatBucket::TarBehindConda => 5,
        }
    }
}

impl PackageFormatSelection {
    /// The buckets whose files this selection uses.
    pub(crate) fn buckets(self) -> FormatBucketSet {
        match self {
            PackageFormatSelection::OnlyConda => {
                FormatBucketSet::from_buckets(&[FormatBucket::Conda])
            }
            PackageFormatSelection::OnlyTarBz2 => FormatBucketSet::from_buckets(&[
                FormatBucket::TarAlone,
                FormatBucket::TarBehindWhl,
                FormatBucket::TarBehindConda,
            ]),
            PackageFormatSelection::PreferConda => FormatBucketSet::from_buckets(&[
                FormatBucket::Conda,
                FormatBucket::TarAlone,
                FormatBucket::TarBehindWhl,
            ]),
            PackageFormatSelection::PreferCondaWithWhl => FormatBucketSet::from_buckets(&[
                FormatBucket::Conda,
                FormatBucket::WhlAlone,
                FormatBucket::TarAlone,
            ]),
            PackageFormatSelection::Both => FormatBucketSet::from_buckets(&[
                FormatBucket::Conda,
                FormatBucket::TarAlone,
                FormatBucket::TarBehindWhl,
                FormatBucket::TarBehindConda,
            ]),
            PackageFormatSelection::All => FormatBucketSet::ALL,
        }
    }
}

/// A set of [`FormatBucket`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FormatBucketSet(u8);

impl FormatBucketSet {
    /// The set that contains every bucket.
    pub(crate) const ALL: FormatBucketSet = FormatBucketSet::from_buckets(&FormatBucket::ALL);

    /// Constructs the set of the given buckets.
    pub(crate) const fn from_buckets(buckets: &[FormatBucket]) -> Self {
        let mut bits = 0;
        let mut position = 0;
        while position < buckets.len() {
            bits |= 1 << buckets[position].index();
            position += 1;
        }
        FormatBucketSet(bits)
    }

    /// Returns true if `bucket` is part of this set.
    pub(crate) fn contains(self, bucket: FormatBucket) -> bool {
        self.0 & (1 << bucket.index()) != 0
    }

    /// Returns true if this set contains no bucket.
    #[cfg(feature = "gateway")]
    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns true if this set contains a bucket with files of the given
    /// archive type.
    pub(crate) fn contains_archive_type(self, archive_type: DistArchiveType) -> bool {
        self.iter()
            .any(|bucket| bucket.archive_type() == archive_type)
    }

    /// Iterates over the buckets of this set in the order of
    /// [`FormatBucket::ALL`].
    pub(crate) fn iter(self) -> impl FusedIterator<Item = FormatBucket> {
        FormatBucket::ALL
            .into_iter()
            .filter(move |bucket| self.contains(*bucket))
    }
}

#[cfg(feature = "gateway")]
impl FromIterator<FormatBucket> for FormatBucketSet {
    fn from_iter<I: IntoIterator<Item = FormatBucket>>(buckets: I) -> Self {
        FormatBucketSet(
            buckets
                .into_iter()
                .fold(0, |bits, bucket| bits | (1 << bucket.index())),
        )
    }
}

/// A value for every [`FormatBucket`].
#[cfg(feature = "gateway")]
#[derive(Debug, Default)]
pub(crate) struct FormatBucketMap<T>([T; 6]);

#[cfg(feature = "gateway")]
impl<T> IntoIterator for FormatBucketMap<T> {
    type Item = (FormatBucket, T);
    type IntoIter = Zip<array::IntoIter<FormatBucket, 6>, array::IntoIter<T, 6>>;

    fn into_iter(self) -> Self::IntoIter {
        FormatBucket::ALL.into_iter().zip(self.0)
    }
}

#[cfg(feature = "gateway")]
impl<T> Index<FormatBucket> for FormatBucketMap<T> {
    type Output = T;

    fn index(&self, bucket: FormatBucket) -> &T {
        &self.0[bucket.index()]
    }
}

#[cfg(feature = "gateway")]
impl<T> IndexMut<FormatBucket> for FormatBucketMap<T> {
    fn index_mut(&mut self, bucket: FormatBucket) -> &mut T {
        &mut self.0[bucket.index()]
    }
}
