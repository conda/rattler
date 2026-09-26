//! The file formats of a subdir index: the kinds of lookup tables, the columns
//! of the packages file and of the tables, and the Parquet writer settings.
//!
//! Every layer of a subdir index consists of:
//!
//! * `packages-<sha256>.parquet`: the filenames of the artifacts the layer
//!   indexes, sorted, in the column `package`. The row number of an artifact
//!   is its *package id*.
//! * One lookup table per [`Kind`], `<kind>-<sha256>.parquet`: a sorted,
//!   unique key column and a `package_ids` column (`list<uint32>`) with the
//!   package ids of the artifacts the key belongs to.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::{
    basic::{Compression, Encoding, ZstdLevel},
    file::{
        metadata::{KeyValue, SortingColumn},
        properties::{EnabledStatistics, WriterProperties, WriterVersion},
    },
    schema::types::ColumnPath,
};

use crate::LookupError;

/// The column of the packages file holding the artifact filenames.
pub const PACKAGE_COLUMN: &str = "package";
/// The values column of every lookup table: package ids.
pub const PACKAGE_IDS_COLUMN: &str = "package_ids";
/// The key column of `paths` tables.
pub const PATH_COLUMN: &str = "path";
/// The key column of `reversed-paths` tables.
pub const REVERSED_PATH_COLUMN: &str = "reversed_path";

/// Leaf column index of the key in a lookup table.
pub(crate) const KEY_LEAF: usize = 0;
/// Leaf column index of the values in a lookup table.
pub(crate) const VALUES_LEAF: usize = 1;
/// Leaf column index of `package` in the packages file.
pub(crate) const PACKAGE_LEAF: usize = 0;

/// The `conda_paths.format_version` every file of the index carries.
pub const FORMAT_VERSION: &str = "1";
/// Key-value metadata: the format version.
pub const METADATA_FORMAT_VERSION: &str = "conda_paths.format_version";
/// Key-value metadata: the kind of a lookup table.
pub const METADATA_KIND: &str = "conda_paths.kind";
/// Key-value metadata: the subdir the file belongs to.
pub const METADATA_SUBDIR: &str = "conda_paths.subdir";
/// Key-value metadata: the channel the file belongs to.
pub const METADATA_CHANNEL: &str = "conda_paths.channel";
/// Key-value metadata: the name of the layer's packages file.
pub const METADATA_PACKAGES: &str = "conda_paths.packages";

/// Rows per data page of a packages file: a lookup reads the pages of the
/// rows it links to, so they are small.
pub const PACKAGES_PAGE_ROWS: usize = 1024;

/// The kinds of lookup tables specified by the CEP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// `path` -> the artifacts containing it.
    Paths,
    /// A path with its `/`-separated components in reverse order
    /// (`zlib.h/include` for `include/zlib.h`) -> the artifacts containing
    /// it. Answers `**/zlib.h` with a prefix scan.
    ReversedPaths,
}

impl Kind {
    /// All kinds this crate knows.
    pub const ALL: [Kind; 2] = [Kind::Paths, Kind::ReversedPaths];

    /// The name of the kind in the manifest and in the file name.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Paths => "paths",
            Kind::ReversedPaths => "reversed-paths",
        }
    }

    /// The kind with the given name, if this crate knows it.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// The name of the key column of tables of this kind.
    pub fn key_column(self) -> &'static str {
        match self {
            Kind::Paths => PATH_COLUMN,
            Kind::ReversedPaths => REVERSED_PATH_COLUMN,
        }
    }

    /// Turns a path into the key of this kind.
    pub fn key_of(self, path: &str) -> String {
        match self {
            Kind::Paths => path.to_string(),
            Kind::ReversedPaths => reverse_components(path),
        }
    }

    /// Turns a key of this kind back into the path.
    pub fn path_of(self, key: &str) -> String {
        // Both transformations are their own inverse.
        self.key_of(key)
    }

    /// The name of a table file of this kind with the given digest.
    pub fn file_name(self, sha256: &str) -> String {
        format!("{}-{sha256}.parquet", self.name())
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for Kind {
    type Err = LookupError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_name(s).ok_or_else(|| LookupError::InvalidInput(format!("unknown kind `{s}`")))
    }
}

/// The name of a packages file with the given digest.
pub fn packages_file_name(sha256: &str) -> String {
    format!("packages-{sha256}.parquet")
}

/// Reverses the components of a path: `include/openssl/ssl.h` becomes
/// `ssl.h/openssl/include`. The reversal is its own inverse.
pub fn reverse_components(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for (i, component) in path.rsplit('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(component);
    }
    out
}

/// The package name of an artifact filename (`<name>-<version>-<build>.<ext>`).
/// Versions and build strings do not contain `-`, names may.
pub fn package_name(filename: &str) -> &str {
    let stem = filename
        .strip_suffix(".conda")
        .or_else(|| filename.strip_suffix(".tar.bz2"))
        .unwrap_or(filename);
    let mut parts = stem.rsplitn(3, '-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(_), Some(_), Some(name)) => name,
        _ => stem,
    }
}

/// The Arrow schema of a lookup table of the given kind.
pub fn table_schema(kind: Kind) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(kind.key_column(), DataType::Utf8, false),
        Field::new(
            PACKAGE_IDS_COLUMN,
            DataType::List(Arc::new(Field::new("element", DataType::UInt32, false))),
            false,
        ),
    ]))
}

/// The Arrow schema of a packages file.
pub fn packages_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new(
        PACKAGE_COLUMN,
        DataType::Utf8,
        false,
    )]))
}

/// Tuning of the Parquet files a writer produces. The defaults follow the
/// recommendations of the CEP.
#[derive(Debug, Clone)]
pub struct WriteOptions {
    /// The zstd compression level.
    pub compression_level: i32,
    /// The target (uncompressed) size of a data page in bytes.
    pub data_page_size: usize,
    /// The maximum number of rows per data page of a lookup table.
    pub page_rows: usize,
    /// The number of rows per row group.
    pub row_group_size: usize,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            compression_level: 12,
            data_page_size: 256 * 1024,
            page_rows: 8192,
            row_group_size: 256 * 1024,
        }
    }
}

pub(crate) fn key_value(key: &str, value: impl ToString) -> KeyValue {
    KeyValue::new(key.to_string(), value.to_string())
}

fn base_properties(
    options: &WriteOptions,
) -> Result<parquet::file::properties::WriterPropertiesBuilder, LookupError> {
    Ok(WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(
            options.compression_level,
        )?))
        .set_dictionary_enabled(false)
        .set_data_page_size_limit(options.data_page_size)
        .set_data_page_row_count_limit(options.page_rows)
        .set_write_batch_size(256)
        .set_max_row_group_row_count(Some(options.row_group_size))
        .set_statistics_enabled(EnabledStatistics::None)
        .set_offset_index_disabled(false))
}

/// The writer properties of a lookup table: delta-encoded keys with page
/// statistics (the column index the lookup navigates with), delta-encoded
/// package ids, no dictionaries.
pub(crate) fn table_properties(
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
        // Sorted keys share long prefixes, which prefix (delta) encoding
        // exploits.
        .set_column_encoding(key.clone(), Encoding::DELTA_BYTE_ARRAY)
        // The ids of a row are sorted: small deltas.
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

/// The writer properties of a packages file: small pages, no statistics, so
/// its offset index is part of the tail a reader fetches with the footer.
pub(crate) fn packages_properties(
    options: &WriteOptions,
    metadata: Vec<KeyValue>,
) -> Result<WriterProperties, LookupError> {
    Ok(base_properties(options)?
        .set_column_encoding(ColumnPath::from(PACKAGE_COLUMN), Encoding::DELTA_BYTE_ARRAY)
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
        assert_eq!(reverse_components("zlib.h"), "zlib.h");
        assert_eq!(reverse_components("a//b"), "b//a");
        let path = "lib/python3.12/site-packages/numpy/__init__.py";
        assert_eq!(reverse_components(&reverse_components(path)), path);
        assert_eq!(Kind::ReversedPaths.key_of("bin/python"), "python/bin");
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
    }

    #[test]
    fn names_kinds() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from_name(kind.name()), Some(kind));
            assert_eq!(kind.name().parse::<Kind>().ok(), Some(kind));
        }
        assert_eq!(Kind::from_name("file-sha256"), None);
        assert_eq!(
            Kind::ReversedPaths.file_name("ab"),
            "reversed-paths-ab.parquet"
        );
        assert_eq!(packages_file_name("ab"), "packages-ab.parquet");
    }
}
