//! The Parquet files of a layer: their schemas and how they are written.
//!
//! A layer consists of one *packages file* and one *lookup table* per
//! [`Kind`]. The tables link to the packages file: their values are *package
//! ids*, the row numbers of the artifacts in it.
//!
//! * `packages-<sha256>.parquet`, sorted by `package`:
//!
//!   | column    | type   | content                      |
//!   |-----------|--------|------------------------------|
//!   | `package` | `utf8` | the filename of an artifact  |
//!
//!   Its data pages hold at most [`PACKAGES_PAGE_ROWS`] rows and it carries no
//!   column statistics, so a reader can resolve a handful of ids by fetching a
//!   page or two, and its offset index arrives together with the footer.
//!
//! * `<kind>-<sha256>.parquet`, sorted by the key of the kind:
//!
//!   | column                       | type           | content                                      |
//!   |------------------------------|----------------|----------------------------------------------|
//!   | the key, [`Kind::key_column`] | `utf8`         | unique, sorted bytewise                      |
//!   | [`PACKAGE_IDS_COLUMN`]       | `list<uint32>` | the ids of the artifacts the key belongs to  |
//!
//!   The key is never dictionary-encoded (a dictionary of unique keys would
//!   only add a page that every lookup has to read) and has page statistics and
//!   a column index, which is what lets a lookup find its page.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::{
    basic::{Compression, Encoding, ZstdLevel},
    file::{
        metadata::{KeyValue, SortingColumn},
        properties::{EnabledStatistics, WriterProperties, WriterPropertiesBuilder, WriterVersion},
    },
    schema::types::ColumnPath,
};
use rattler_digest::Sha256Hash;

use crate::LookupError;

/// The name of the key column of a [`Kind::Paths`] table.
pub const PATH_COLUMN: &str = "path";
/// The name of the key column of a [`Kind::ReversedPaths`] table.
pub const REVERSED_PATH_COLUMN: &str = "reversed_path";
/// The name of the value column of every lookup table.
pub const PACKAGE_IDS_COLUMN: &str = "package_ids";
/// The name of the first column of the packages file.
pub const PACKAGE_COLUMN: &str = "package";

/// The leaf column index of the key of a lookup table.
pub const KEY_LEAF: usize = 0;
/// The leaf column index of the elements of `package_ids`.
pub const VALUES_LEAF: usize = 1;
/// The leaf column index of `package` in the packages file.
pub const PACKAGE_LEAF: usize = 0;

/// The maximum number of rows per data page of a packages file.
///
/// A lookup reads the pages that hold the ids it resolves, so they are small.
pub const PACKAGES_PAGE_ROWS: usize = 1024;

/// The value of the [`KEY_FORMAT_VERSION`] metadata of files of this format.
pub const FORMAT_VERSION: &str = "1";
/// File metadata key holding [`FORMAT_VERSION`].
pub const KEY_FORMAT_VERSION: &str = "conda_paths.format_version";
/// File metadata key of a lookup table holding its [`Kind`].
pub const KEY_KIND: &str = "conda_paths.kind";
/// File metadata key of a lookup table holding the name of the packages file
/// its ids refer to.
pub const KEY_PACKAGES: &str = "conda_paths.packages";
/// File metadata key holding the base url of the indexed channel.
pub const KEY_CHANNEL: &str = "conda_paths.channel";
/// File metadata key holding the indexed subdir.
pub const KEY_SUBDIR: &str = "conda_paths.subdir";
/// File metadata key holding the creation time of the layer (RFC 3339).
pub const KEY_CREATED: &str = "conda_paths.created";

/// What the keys of a lookup table are.
///
/// Every layer has one table per kind, and all tables of a layer describe the
/// same artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Kind {
    /// Keys are paths, so a path can be looked up directly.
    Paths,
    /// Keys are paths with their `/`-separated components reversed, so
    /// `**/zlib.h` is a prefix scan, see [`reverse_components`].
    ReversedPaths,
}

impl Kind {
    /// Every kind this crate knows.
    pub const ALL: [Kind; 2] = [Kind::Paths, Kind::ReversedPaths];

    /// The name of the kind in the manifest, which is also the prefix of the
    /// names of its files.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Paths => "paths",
            Kind::ReversedPaths => "reversed-paths",
        }
    }

    /// The kind of that name, or `None` for a kind this crate does not know.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// The name of the key column of a table of this kind.
    pub fn key_column(self) -> &'static str {
        match self {
            Kind::Paths => PATH_COLUMN,
            Kind::ReversedPaths => REVERSED_PATH_COLUMN,
        }
    }

    /// The key a path is stored under in a table of this kind.
    pub fn key_of(self, path: &str) -> String {
        match self {
            Kind::Paths => path.to_string(),
            Kind::ReversedPaths => reverse_components(path),
        }
    }

    /// The path a key of a table of this kind belongs to, the inverse of
    /// [`Kind::key_of`].
    pub fn path_of(self, key: &str) -> String {
        // Reversing the components is its own inverse.
        self.key_of(key)
    }

    /// The name a table of this kind with the given digest has to be stored
    /// under.
    pub fn file_name(self, sha256: &Sha256Hash) -> String {
        format!("{}-{}.parquet", self.name(), hex::encode(sha256))
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Reverses the `/`-separated components of a path: `include/openssl/ssl.h`
/// becomes `ssl.h/openssl/include`.
///
/// The reversal is its own inverse.
pub fn reverse_components(path: &str) -> String {
    let mut reversed = String::with_capacity(path.len());
    for (i, component) in path.rsplit('/').enumerate() {
        if i > 0 {
            reversed.push('/');
        }
        reversed.push_str(component);
    }
    reversed
}

/// The package name of an artifact filename (`<name>-<version>-<build>.<ext>`).
///
/// Versions and build strings never contain `-`, names may. A filename that is
/// not an artifact filename is returned as it is.
pub fn package_name(file_name: &str) -> &str {
    let Some(stem) = file_name
        .strip_suffix(".conda")
        .or_else(|| file_name.strip_suffix(".tar.bz2"))
    else {
        return file_name;
    };
    let mut parts = stem.rsplitn(3, '-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(_build), Some(_version), Some(name)) => name,
        _ => stem,
    }
}

/// The name a packages file with the given digest has to be stored under.
pub fn packages_file_name(sha256: &Sha256Hash) -> String {
    format!("packages-{}.parquet", hex::encode(sha256))
}

/// The Arrow schema of a lookup table of `kind`.
pub fn table_schema(kind: Kind) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(kind.key_column(), DataType::Utf8, false),
        Field::new(PACKAGE_IDS_COLUMN, package_ids_type(), false),
    ]))
}

/// The Arrow type of [`PACKAGE_IDS_COLUMN`].
pub fn package_ids_type() -> DataType {
    DataType::List(Arc::new(Field::new("element", DataType::UInt32, false)))
}

/// The Arrow schema of the packages file.
pub fn packages_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new(
        PACKAGE_COLUMN,
        DataType::Utf8,
        false,
    )]))
}

/// How the Parquet files of a layer are written.
///
/// The defaults are the values recommended by the CEP; they trade the size of
/// the files against the number of bytes a single lookup has to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    /// The zstd compression level of the data pages.
    pub compression_level: i32,
    /// The size (uncompressed) at which a data page is flushed.
    pub data_page_size: usize,
    /// The maximum number of rows in a data page of a lookup table.
    pub page_rows: usize,
    /// The maximum number of rows in a row group.
    pub row_group_size: usize,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            compression_level: 12,
            data_page_size: 256 * 1024,
            page_rows: 8192,
            row_group_size: 256_000,
        }
    }
}

/// A key/value metadata entry of a Parquet file.
pub fn key_value(key: &str, value: impl ToString) -> KeyValue {
    KeyValue::new(key.to_string(), value.to_string())
}

fn base_properties(options: &WriteOptions) -> Result<WriterPropertiesBuilder, LookupError> {
    let level = ZstdLevel::try_new(options.compression_level)?;
    Ok(WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::ZSTD(level))
        // Neither the unique keys nor the delta-encoded ids profit from one.
        .set_dictionary_enabled(false)
        .set_data_page_size_limit(options.data_page_size)
        .set_data_page_row_count_limit(options.page_rows)
        .set_write_batch_size(256)
        .set_max_row_group_row_count(Some(options.row_group_size))
        // Statistics only where they are enabled per column below.
        .set_statistics_enabled(EnabledStatistics::None)
        .set_offset_index_disabled(false))
}

/// The writer properties of a lookup table, including its file metadata.
pub fn table_properties(
    options: &WriteOptions,
    kind: Kind,
    metadata: Vec<KeyValue>,
) -> Result<WriterProperties, LookupError> {
    let key = ColumnPath::from(kind.key_column());
    let values = ColumnPath::new(vec![
        PACKAGE_IDS_COLUMN.to_string(),
        "list".to_string(),
        "element".to_string(),
    ]);
    Ok(base_properties(options)?
        // Sorted strings share long prefixes, so prefix (delta) encoding is ideal.
        .set_column_encoding(key.clone(), Encoding::DELTA_BYTE_ARRAY)
        // The ids of a row are sorted, so their deltas are small.
        .set_column_encoding(values, Encoding::DELTA_BINARY_PACKED)
        // Only the key needs min/max statistics: they drive the lookup.
        .set_column_statistics_enabled(key, EnabledStatistics::Page)
        .set_column_index_truncate_length(Some(128))
        .set_statistics_truncate_length(None)
        .set_sorting_columns(Some(vec![SortingColumn {
            column_idx: KEY_LEAF as i32,
            descending: false,
            nulls_first: false,
        }]))
        .set_key_value_metadata(Some(metadata))
        .build())
}

/// The writer properties of the packages file, including its file metadata.
pub fn packages_properties(
    options: &WriteOptions,
    metadata: Vec<KeyValue>,
) -> Result<WriterProperties, LookupError> {
    Ok(base_properties(options)?
        .set_column_encoding(ColumnPath::from(PACKAGE_COLUMN), Encoding::DELTA_BYTE_ARRAY)
        // Readers fetch rows by number, so small pages keep a lookup cheap.
        .set_data_page_row_count_limit(PACKAGES_PAGE_ROWS)
        .set_sorting_columns(Some(vec![SortingColumn {
            column_idx: PACKAGE_LEAF as i32,
            descending: false,
            nulls_first: false,
        }]))
        .set_key_value_metadata(Some(metadata))
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverses_components() {
        assert_eq!(
            reverse_components("include/openssl/ssl.h"),
            "ssl.h/openssl/include"
        );
        assert_eq!(reverse_components("bin/python"), "python/bin");
        assert_eq!(reverse_components("LICENSE"), "LICENSE");
        assert_eq!(reverse_components("a//b"), "b//a");
        let path = "lib/python3.12/site-packages/numpy/__init__.py";
        assert_eq!(reverse_components(&reverse_components(path)), path);
        assert_eq!(Kind::ReversedPaths.path_of("python/bin"), "bin/python");
        assert_eq!(Kind::Paths.key_of("bin/python"), "bin/python");
    }

    #[test]
    fn extracts_package_names() {
        assert_eq!(package_name("polars-1.44.2-pyh3138b34_0.conda"), "polars");
        assert_eq!(
            package_name("python-dateutil-2.9.0-pyhd8ed1ab_0.tar.bz2"),
            "python-dateutil"
        );
        assert_eq!(
            package_name("_libgcc_mutex-0.1-conda_forge.tar.bz2"),
            "_libgcc_mutex"
        );
        assert_eq!(package_name("not-a-package"), "not-a-package");
    }

    #[test]
    fn names_kinds() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(Kind::from_name("x-something"), None);
        assert_eq!(Kind::ReversedPaths.to_string(), "reversed-paths");
    }
}
