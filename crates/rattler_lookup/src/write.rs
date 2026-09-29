//! Writing the files of a layer: the packages file and one lookup table per
//! kind, as content-addressed Parquet files.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufWriter, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use arrow_array::{
    ArrayRef, RecordBatch,
    builder::{ListBuilder, StringBuilder, UInt32Builder},
};
use arrow_schema::SchemaRef;
use parquet::{arrow::ArrowWriter, file::metadata::KeyValue};
use sha2::{Digest, Sha256};

use crate::{
    Kind, Location, LookupError, WriteOptions,
    format::{self, key_value},
    manifest::{FileRef, Layer, PackagesRef, now_rfc3339},
};

/// Rows per Arrow record batch handed to the Parquet writer.
const BATCH_ROWS: usize = 16 * 1024;

/// A file of a written layer.
#[derive(Debug, Clone)]
pub struct WrittenFile {
    /// The content-addressed file name (`<kind>-<sha256>.parquet` or
    /// `packages-<sha256>.parquet`).
    pub name: String,
    /// Where the file was written.
    pub path: PathBuf,
    /// The size in bytes.
    pub size: u64,
    /// The lowercase hex SHA-256 of the file.
    pub sha256: String,
}

/// A layer that was written to a directory.
#[derive(Debug, Clone)]
pub struct WrittenLayer {
    /// The entry for the manifest.
    pub layer: Layer,
    /// The packages file and the tables, in that order.
    pub files: Vec<WrittenFile>,
    /// The number of distinct paths.
    pub num_paths: u64,
    /// The number of (path, artifact) pairs.
    pub num_pairs: u64,
}

/// Writes a layer for `artifacts`, `(filename, paths)` pairs in any order,
/// to `dir`: the packages file and a table of every kind in `kinds`.
///
/// Everything is sorted in memory, which suits layers of up to a few
/// hundred thousand artifacts.
pub fn write_layer(
    dir: &Path,
    channel: &str,
    subdir: &str,
    kinds: &[Kind],
    mut artifacts: Vec<(String, Vec<String>)>,
    options: &WriteOptions,
) -> Result<WrittenLayer, LookupError> {
    artifacts.sort_by(|a, b| a.0.cmp(&b.0));
    let mut packages = Vec::with_capacity(artifacts.len());
    let mut paths: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (id, (filename, artifact_paths)) in artifacts.into_iter().enumerate() {
        if packages.last() == Some(&filename) {
            return Err(LookupError::InvalidInput(format!(
                "the artifact {filename} is given twice"
            )));
        }
        let id = u32::try_from(id)
            .map_err(|_overflow| LookupError::InvalidInput("more than 2^32 artifacts".into()))?;
        packages.push(filename);
        let mut artifact_paths = artifact_paths;
        artifact_paths.sort_unstable();
        artifact_paths.dedup();
        for path in artifact_paths {
            paths.entry(path).or_default().push(id);
        }
    }
    write_layer_sorted(dir, channel, subdir, kinds, &packages, paths, options)
}

/// Writes a layer to `dir` from already sorted data: `packages` are the
/// filenames of the artifacts, sorted and unique (their position is their
/// package id), and `paths` maps every path to the sorted, unique package
/// ids of the artifacts containing it.
pub fn write_layer_sorted(
    dir: &Path,
    channel: &str,
    subdir: &str,
    kinds: &[Kind],
    packages: &[String],
    paths: BTreeMap<String, Vec<u32>>,
    options: &WriteOptions,
) -> Result<WrittenLayer, LookupError> {
    if kinds.is_empty() {
        return Err(LookupError::InvalidInput("no kinds to write".into()));
    }
    for window in packages.windows(2) {
        if window[0] >= window[1] {
            return Err(LookupError::InvalidInput(format!(
                "the packages are not sorted and unique: {} is followed by {}",
                window[0], window[1]
            )));
        }
    }
    if let Some(filename) = packages.iter().find(|f| f.contains('/') || f.is_empty()) {
        return Err(LookupError::InvalidInput(format!(
            "`{filename}` is not an artifact filename"
        )));
    }
    let num_packages = u32::try_from(packages.len())
        .map_err(|_overflow| LookupError::InvalidInput("more than 2^32 artifacts".into()))?;
    std::fs::create_dir_all(dir).map_err(|e| LookupError::io(&Location::from(dir), e))?;

    // The packages file first: its name goes into the metadata of the tables.
    let packages_file = write_packages_file(dir, channel, subdir, packages, options)?;
    let metadata = vec![
        key_value(format::METADATA_FORMAT_VERSION, format::FORMAT_VERSION),
        key_value(format::METADATA_CHANNEL, channel),
        key_value(format::METADATA_SUBDIR, subdir),
        key_value(format::METADATA_PACKAGES, &packages_file.name),
    ];

    let mut files = vec![packages_file];
    let mut tables = BTreeMap::new();
    let mut num_paths = 0;
    let mut num_pairs = 0;
    for &kind in kinds {
        let mut metadata = metadata.clone();
        metadata.push(key_value(format::METADATA_KIND, kind.name()));
        let mut writer = TableWriter::create(dir, kind, metadata, options)?;
        match kind {
            Kind::Paths => {
                for (path, ids) in &paths {
                    check_ids(path, ids, num_packages)?;
                    writer.push(path, ids)?;
                    num_paths += 1;
                    num_pairs += ids.len() as u64;
                }
            }
            Kind::ReversedPaths => {
                // Reversing components is a bijection, so the keys stay
                // unique; they only have to be sorted again.
                let mut reversed: Vec<(String, &[u32])> = paths
                    .iter()
                    .map(|(path, ids)| (format::reverse_components(path), ids.as_slice()))
                    .collect();
                reversed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                for (key, ids) in reversed {
                    check_ids(&key, ids, num_packages)?;
                    writer.push(&key, ids)?;
                }
            }
        }
        let file = writer.finish()?;
        tables.insert(
            kind.name().to_string(),
            FileRef {
                file: file.name.clone(),
                size: file.size,
            },
        );
        files.push(file);
    }
    if !kinds.contains(&Kind::Paths) {
        num_paths = paths.len() as u64;
        num_pairs = paths.values().map(|ids| ids.len() as u64).sum();
    }

    Ok(WrittenLayer {
        layer: Layer {
            created_at: now_rfc3339(),
            packages: PackagesRef {
                file: files[0].name.clone(),
                size: files[0].size,
                count: u64::from(num_packages),
            },
            tables,
        },
        files,
        num_paths,
        num_pairs,
    })
}

fn check_ids(key: &str, ids: &[u32], num_packages: u32) -> Result<(), LookupError> {
    let valid = !ids.is_empty()
        && ids.windows(2).all(|w| w[0] < w[1])
        && ids.last().is_some_and(|&last| last < num_packages);
    if valid {
        Ok(())
    } else {
        Err(LookupError::InvalidInput(format!(
            "the package ids of `{key}` are empty, not sorted, not unique or out of range"
        )))
    }
}

fn write_packages_file(
    dir: &Path,
    channel: &str,
    subdir: &str,
    packages: &[String],
    options: &WriteOptions,
) -> Result<WrittenFile, LookupError> {
    let tmp = dir.join("packages.parquet.partial");
    let props = format::packages_properties(
        options,
        vec![
            key_value(format::METADATA_FORMAT_VERSION, format::FORMAT_VERSION),
            key_value(format::METADATA_CHANNEL, channel),
            key_value(format::METADATA_SUBDIR, subdir),
        ],
    )?;
    let schema = format::packages_schema();
    let file = File::create(&tmp).map_err(|e| LookupError::io(&Location::from(tmp.clone()), e))?;
    let mut writer = ArrowWriter::try_new(BufWriter::new(file), schema.clone(), Some(props))?;
    for chunk in packages.chunks(BATCH_ROWS) {
        let mut column = StringBuilder::new();
        for filename in chunk {
            column.append_value(filename);
        }
        let columns = vec![Arc::new(column.finish()) as ArrayRef];
        writer.write(&RecordBatch::try_new(schema.clone(), columns)?)?;
    }
    writer.close()?;
    finish_file(tmp, format::packages_file_name)
}

/// Writes one lookup table to `<dir>/<kind>.parquet.partial` and renames it
/// to its content-addressed name when finished.
struct TableWriter {
    kind: Kind,
    tmp: PathBuf,
    writer: ArrowWriter<BufWriter<File>>,
    schema: SchemaRef,
    keys: StringBuilder,
    ids: ListBuilder<UInt32Builder>,
    rows: usize,
}

impl TableWriter {
    fn create(
        dir: &Path,
        kind: Kind,
        metadata: Vec<KeyValue>,
        options: &WriteOptions,
    ) -> Result<Self, LookupError> {
        let schema = format::table_schema(kind);
        let props = format::table_properties(options, kind, metadata)?;
        let tmp = dir.join(format!("{}.parquet.partial", kind.name()));
        let file =
            File::create(&tmp).map_err(|e| LookupError::io(&Location::from(tmp.clone()), e))?;
        let writer = ArrowWriter::try_new(
            BufWriter::with_capacity(8 << 20, file),
            schema.clone(),
            Some(props),
        )?;
        Ok(Self {
            kind,
            tmp,
            writer,
            schema,
            keys: StringBuilder::new(),
            ids: ListBuilder::new(UInt32Builder::new()).with_field(Arc::new(
                arrow_schema::Field::new("element", arrow_schema::DataType::UInt32, false),
            )),
            rows: 0,
        })
    }

    fn push(&mut self, key: &str, ids: &[u32]) -> Result<(), LookupError> {
        self.keys.append_value(key);
        self.ids.values().append_slice(ids);
        self.ids.append(true);
        self.rows += 1;
        if self.rows == BATCH_ROWS {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), LookupError> {
        if self.rows > 0 {
            let batch = RecordBatch::try_new(
                self.schema.clone(),
                vec![
                    Arc::new(self.keys.finish()) as ArrayRef,
                    Arc::new(self.ids.finish()) as ArrayRef,
                ],
            )?;
            self.writer.write(&batch)?;
            self.rows = 0;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<WrittenFile, LookupError> {
        self.flush()?;
        self.writer.close()?;
        let kind = self.kind;
        finish_file(self.tmp, |sha256| kind.file_name(sha256))
    }
}

/// Hashes a finished file and renames it to its content-addressed name.
fn finish_file(
    tmp: PathBuf,
    name: impl FnOnce(&str) -> String,
) -> Result<WrittenFile, LookupError> {
    let location = Location::from(tmp.clone());
    let (sha256, size) = sha256_file(&tmp).map_err(|e| LookupError::io(&location, e))?;
    let name = name(&sha256);
    let path = tmp.with_file_name(&name);
    std::fs::rename(&tmp, &path).map_err(|e| LookupError::io(&location, e))?;
    Ok(WrittenFile {
        name,
        path,
        size,
        sha256,
    })
}

/// The lowercase hex SHA-256 and the size of a file.
pub fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), size))
}

/// The lowercase hex SHA-256 of bytes.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The SHA-256 a content-addressed layer file name claims, if the name has
/// the expected shape (`<prefix>-<64 hex digits>.parquet`).
pub fn sha256_of_name(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".parquet")?;
    let (_, sha256) = stem.rsplit_once('-')?;
    (sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    .then_some(sha256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_digests_from_names() {
        let sha = "a".repeat(64);
        assert_eq!(
            sha256_of_name(&format!("paths-{sha}.parquet")),
            Some(sha.as_str())
        );
        assert_eq!(
            sha256_of_name(&format!("reversed-paths-{sha}.parquet")),
            Some(sha.as_str())
        );
        assert_eq!(sha256_of_name("paths-abc.parquet"), None);
        assert_eq!(sha256_of_name(&format!("paths-{sha}.json")), None);
        assert_eq!(
            sha256_of_name(&format!("paths-{}.parquet", sha.to_uppercase())),
            None
        );
    }

    #[test]
    fn rejects_invalid_input() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_layer(
            dir.path(),
            "c",
            "noarch",
            &Kind::ALL,
            vec![
                ("a-1-0.conda".into(), vec!["x".into()]),
                ("a-1-0.conda".into(), vec!["y".into()]),
            ],
            &WriteOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(err, LookupError::InvalidInput(_)));
        let err = write_layer_sorted(
            dir.path(),
            "c",
            "noarch",
            &[],
            &[],
            BTreeMap::new(),
            &WriteOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(err, LookupError::InvalidInput(_)));
    }
}
