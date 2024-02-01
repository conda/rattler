//! Sparse data parsing for conda package information such as, `repodata.json`, and `run_exports.json` files.

#![allow(clippy::mem_forget)]

use itertools::Itertools;
use rattler_conda_types::{Channel, ChannelInfo, PackageRecord};
use serde::{
    de::{Error, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::value::RawValue;
use std::{fmt, io, marker::PhantomData, path::Path};

pub mod repodata;
pub mod run_exports;

pub use repodata::*;
pub use run_exports::*;

/// Generalized struct for storing and loading data from large files using sparse data parsing.
pub struct SparseData<T: Sized> {
    /// Data structure that holds a memory mapped repodata.json file and an index into the the records
    /// store in that data.
    inner: SparseDataInner,

    /// The channel from which this data was downloaded.
    channel: Channel,

    /// The subdirectory from where the repodata is downloaded
    subdir: String,

    /// A function that can be used to patch the package record after it has been parsed.
    /// This is mainly used to add `pip` to `python` if desired
    patch_function: Option<fn(&mut T)>,
}

/// A struct that holds a memory map of a `repodata.json` file and also a self-referential field which
/// indexes the data in the memory map with a sparsely parsed json struct. See [`LazyRepoData`].

#[ouroboros::self_referencing]
struct SparseDataInner {
    /// Memory map of the `repodata.json` file
    memory_map: memmap2::Mmap,

    /// Sparsely parsed json content of the memory map. This data struct holds references into the memory
    /// map so we have to use ouroboros to make this legal.
    #[borrows(memory_map)]
    #[covariant]
    data: LazyMap<'this>,
}

impl<T: Sized> SparseData<T> {
    /// Construct an instance of self from a file on disk and a [`Channel`].
    /// The `patch_function` can be used to patch the package record after it has been parsed
    /// (e.g. to add `pip` to `python`).
    pub fn new(
        channel: Channel,
        subdir: impl Into<String>,
        path: impl AsRef<Path>,
        patch_function: Option<fn(&mut T)>,
    ) -> Result<Self, io::Error> {
        let file = std::fs::File::open(path)?;
        let memory_map = unsafe { memmap2::Mmap::map(&file) }?;
        Ok(SparseData {
            inner: SparseDataInnerTryBuilder {
                memory_map,
                data_builder: |memory_map| serde_json::from_slice(memory_map.as_ref()),
            }
            .try_build()?,
            subdir: subdir.into(),
            channel,
            patch_function,
        })
    }

    /// Returns an iterator over all package names in this repodata file.
    ///
    /// This works by iterating over all elements in the `packages` and `conda_packages` fields of
    /// the repodata and returning the unique package names.
    pub fn package_names(&self) -> impl Iterator<Item = &'_ str> + '_ {
        let repo_data = self.inner.borrow_data();
        repo_data
            .packages
            .iter()
            .chain(repo_data.conda_packages.iter())
            .map(|(name, _)| name.package)
            .dedup()
    }

    /// Returns the subdirectory from which this repodata was loaded
    pub fn subdir(&self) -> &str {
        &self.subdir
    }
}

/// A serde compatible struct that only sparsely parses a repodata.json file.
#[derive(Deserialize)]
struct LazyMap<'i> {
    /// The channel information contained in the repodata.json file
    info: Option<ChannelInfo>,

    /// The tar.bz2 packages contained in the data file
    #[serde(borrow, deserialize_with = "deserialize_filename_and_raw_record")]
    packages: Vec<(PackageFilename<'i>, &'i RawValue)>,

    /// The conda packages contained in the data file (under a different key for
    /// backwards compatibility with previous conda versions)
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_filename_and_raw_record",
        rename = "packages.conda"
    )]
    conda_packages: Vec<(PackageFilename<'i>, &'i RawValue)>,
}

fn deserialize_filename_and_raw_record<'d, D: Deserializer<'d>>(
    deserializer: D,
) -> Result<Vec<(PackageFilename<'d>, &'d RawValue)>, D::Error> {
    #[allow(clippy::type_complexity)]
    struct MapVisitor<I, K, V>(PhantomData<fn() -> (I, K, V)>);

    impl<'de, I, K, V> Visitor<'de> for MapVisitor<I, K, V>
    where
        I: FromIterator<(K, V)>,
        K: Deserialize<'de>,
        V: Deserialize<'de>,
    {
        type Value = I;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a map")
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let iter = MapIter(map, PhantomData);
            iter.collect()
        }
    }

    struct MapIter<'de, A, K, V>(A, PhantomData<(&'de (), A, K, V)>);

    impl<'de, A, K, V> Iterator for MapIter<'de, A, K, V>
    where
        A: MapAccess<'de>,
        K: Deserialize<'de>,
        V: Deserialize<'de>,
    {
        type Item = Result<(K, V), A::Error>;

        fn next(&mut self) -> Option<Self::Item> {
            match self.0.next_entry() {
                Ok(Some(x)) => Some(Ok(x)),
                Ok(None) => None,
                Err(err) => Some(Err(err)),
            }
        }
    }

    let mut entries: Vec<(PackageFilename<'d>, &'d RawValue)> =
        deserializer.deserialize_map(MapVisitor(PhantomData))?;

    // Although in general the filenames are sorted in repodata.json this doesnt necessarily mean
    // that the records are also sorted by package name.
    //
    // To illustrate, the following filenames are properly sorted by filename but they are NOT
    // properly sorted by package name.
    // - clang-format-12.0.1-default_he082bbe_4.tar.bz2 (package name: clang-format)
    // - clang-format-13-13.0.0-default_he082bbe_0.tar.bz2 (package name: clang-format-13)
    // - clang-format-13.0.0-default_he082bbe_0.tar.bz2 (package name: clang-format)
    //
    // Because most use-cases involve finding filenames by package name we reorder the entries here
    // by package name. This enables use the binary search for the packages we need.
    //
    // Since (in most cases) the repodata is already ordered by filename which does closely resemble
    // ordering by package name this sort operation will most likely be very fast.
    entries.sort_by(|(a, _), (b, _)| a.package.cmp(b.package));

    Ok(entries)
}

/// A struct that holds both a filename and the part of the filename thats just the package name.
struct PackageFilename<'i> {
    package: &'i str,
    filename: &'i str,
}

impl<'de> Deserialize<'de> for PackageFilename<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        <&str>::deserialize(deserializer)?
            .try_into()
            .map_err(D::Error::custom)
    }
}

impl<'de> TryFrom<&'de str> for PackageFilename<'de> {
    type Error = &'static str;

    fn try_from(s: &'de str) -> Result<Self, Self::Error> {
        let package = s.rsplitn(3, '-').nth(2).ok_or("invalid filename")?;
        Ok(PackageFilename {
            package,
            filename: s,
        })
    }
}
