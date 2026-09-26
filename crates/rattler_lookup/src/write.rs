//! Producing the layer files of an index.
//!
//! Layer files are content-addressed, so their bytes exist before their name
//! does: a [`LayerWriter`] produces the bytes of every file of a layer and the
//! caller stores them under [`LayerFile::file_name`]. The packages file is
//! written first because the ids in the lookup tables are row numbers in it and
//! its name goes into their metadata.
//!
//! A [`LayerWriter`] streams the [`Kind::Paths`] table, but the keys of the
//! other kinds are not in the order the paths arrive in, so their rows are held
//! in memory until [`LayerWriter::finish`]. A layer therefore costs memory
//! proportional to the paths it indexes, which is why an indexer writes one
//! layer per run instead of rewriting the whole index every time.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap},
    sync::Arc,
};

use arrow_array::{
    ArrayRef, RecordBatch,
    builder::{ListBuilder, StringBuilder, UInt32Builder},
    cast::AsArray,
    types::UInt32Type,
};
use arrow_schema::{DataType, Field};
use bytes::Bytes;
use jiff::Timestamp;
use parquet::{
    arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder},
    file::metadata::KeyValue,
};
use rattler_digest::{Sha256, Sha256Hash, compute_bytes_digest};

use crate::{
    LookupError, Result,
    format::{self, Kind, WriteOptions, key_value},
    manifest::{FileRef, Layer, PackagesRef},
};

/// The number of rows after which a batch is written.
const BATCH_ROWS: usize = 16 * 1024;
/// Key bytes after which a batch is written early: Arrow string arrays have
/// 32-bit offsets, so a batch of long keys can exceed 2 GiB within
/// [`BATCH_ROWS`] rows.
const BATCH_BYTES: usize = 256 << 20;

/// One file of a layer, ready to be stored under its content-addressed name.
#[derive(Debug, Clone)]
pub struct LayerFile {
    /// The name the file has to be stored under.
    pub file_name: String,
    /// The contents of the file.
    pub bytes: Vec<u8>,
    /// The SHA-256 of [`Self::bytes`], also part of [`Self::file_name`].
    pub sha256: Sha256Hash,
}

impl LayerFile {
    fn new(bytes: Vec<u8>, name: impl Fn(&Sha256Hash) -> String) -> Self {
        let sha256 = compute_bytes_digest::<Sha256>(&bytes);
        Self {
            file_name: name(&sha256),
            bytes,
            sha256,
        }
    }

    /// The size of the file in bytes.
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// All files of a newly written layer.
#[derive(Debug, Clone)]
pub struct LayerFiles {
    /// The packages file, `packages-<sha256>.parquet`.
    pub packages: LayerFile,
    /// The lookup tables of the layer, `<kind>-<sha256>.parquet`.
    pub tables: BTreeMap<Kind, LayerFile>,
    /// When the layer was created.
    pub created_at: Timestamp,
    /// The number of artifacts the layer indexes.
    pub num_packages: u64,
    /// The number of distinct paths in the layer.
    pub num_paths: u64,
}

impl LayerFiles {
    /// The entry to add to the manifest for this layer.
    pub fn manifest_layer(&self) -> Layer {
        Layer {
            created_at: self.created_at,
            packages: PackagesRef {
                file: self.packages.file_name.clone(),
                size: self.packages.size(),
                count: self.num_packages,
            },
            tables: self
                .tables
                .iter()
                .map(|(kind, table)| {
                    (
                        kind.name().to_string(),
                        FileRef {
                            file: table.file_name.clone(),
                            size: table.size(),
                        },
                    )
                })
                .collect(),
        }
    }

    /// Every file of the layer, the packages file first.
    pub fn files(&self) -> impl Iterator<Item = &LayerFile> {
        std::iter::once(&self.packages).chain(self.tables.values())
    }
}

/// The rows of one lookup table, buffered until they are written.
struct Batch {
    keys: StringBuilder,
    values: ListBuilder<UInt32Builder>,
    rows: usize,
    bytes: usize,
}

impl Batch {
    fn new() -> Self {
        Self {
            keys: StringBuilder::new(),
            values: ListBuilder::new(UInt32Builder::new()).with_field(Arc::new(Field::new(
                "element",
                DataType::UInt32,
                false,
            ))),
            rows: 0,
            bytes: 0,
        }
    }

    fn push(&mut self, key: &str, ids: &[u32]) {
        self.keys.append_value(key);
        self.values.values().append_slice(ids);
        self.values.append(true);
        self.rows += 1;
        self.bytes += key.len();
    }

    fn finish(&mut self, kind: Kind) -> Result<RecordBatch> {
        self.rows = 0;
        self.bytes = 0;
        Ok(RecordBatch::try_new(
            format::table_schema(kind),
            vec![
                Arc::new(self.keys.finish()) as ArrayRef,
                Arc::new(self.values.finish()) as ArrayRef,
            ],
        )?)
    }
}

/// Writes one lookup table of a layer.
struct TableWriter {
    kind: Kind,
    writer: ArrowWriter<Vec<u8>>,
    batch: Batch,
    previous: Option<String>,
    rows: u64,
}

impl TableWriter {
    fn new(kind: Kind, metadata: Vec<KeyValue>, options: &WriteOptions) -> Result<Self> {
        Ok(Self {
            kind,
            writer: ArrowWriter::try_new(
                Vec::new(),
                format::table_schema(kind),
                Some(format::table_properties(options, kind, metadata)?),
            )?,
            batch: Batch::new(),
            previous: None,
            rows: 0,
        })
    }

    /// Adds one key and the ids of the artifacts it belongs to, which have to be
    /// sorted and unique. Keys have to be pushed in ascending order.
    fn push(&mut self, key: &str, ids: &[u32]) -> Result<()> {
        if let Some(previous) = &self.previous
            && previous.as_str() >= key
        {
            return Err(LookupError::NotSorted {
                previous: previous.clone(),
                path: key.to_string(),
            });
        }
        self.previous = Some(key.to_string());
        self.batch.push(key, ids);
        self.rows += 1;
        if self.batch.rows == BATCH_ROWS || self.batch.bytes >= BATCH_BYTES {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.batch.rows > 0 {
            let batch = self.batch.finish(self.kind)?;
            self.writer.write(&batch)?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<LayerFile> {
        self.flush()?;
        let kind = self.kind;
        Ok(LayerFile::new(self.writer.into_inner()?, |sha256| {
            kind.file_name(sha256)
        }))
    }
}

/// Writes all files of one layer.
///
/// Paths have to be pushed in ascending order and only once each, see
/// [`LayerBuilder`] for a writer that sorts for you.
pub struct LayerWriter {
    options: WriteOptions,
    packages: LayerFile,
    /// The id of every artifact of the layer, the row of its filename in the
    /// packages file.
    ids: HashMap<String, u32>,
    num_packages: u64,
    num_paths: u64,
    created_at: Timestamp,
    /// The table that is written while paths are pushed, if it was asked for.
    paths: Option<TableWriter>,
    /// The rows of the tables whose keys are not in the order paths arrive in.
    buffered: BTreeMap<Kind, Vec<(String, Vec<u32>)>>,
    metadata: Vec<KeyValue>,
    previous: Option<String>,
    ids_of_path: Vec<u32>,
}

impl LayerWriter {
    /// Writes the packages file of a layer and prepares its lookup tables.
    ///
    /// `packages` are the filenames of all artifacts the layer indexes, in
    /// ascending order and without duplicates. `kinds` are the tables to write;
    /// every layer of an index has to have the same ones.
    pub fn new<S: AsRef<str>>(
        channel: &str,
        subdir: &str,
        packages: impl IntoIterator<Item = S>,
        kinds: &[Kind],
        options: &WriteOptions,
    ) -> Result<Self> {
        let created_at = Timestamp::now();
        let packages_metadata = vec![
            key_value(format::KEY_FORMAT_VERSION, format::FORMAT_VERSION),
            key_value(format::KEY_CHANNEL, channel),
            key_value(format::KEY_SUBDIR, subdir),
            key_value(format::KEY_CREATED, created_at),
        ];
        let mut writer = ArrowWriter::try_new(
            Vec::new(),
            format::packages_schema(),
            Some(format::packages_properties(options, packages_metadata)?),
        )?;
        let mut ids = HashMap::new();
        let mut previous: Option<String> = None;
        let mut names = StringBuilder::new();
        let mut rows = 0;
        for package in packages {
            let package = package.as_ref();
            if let Some(previous) = &previous
                && previous.as_str() >= package
            {
                return Err(LookupError::NotSorted {
                    previous: previous.clone(),
                    path: package.to_string(),
                });
            }
            previous = Some(package.to_string());
            let id = u32::try_from(ids.len()).map_err(|_too_many| LookupError::TooManyPackages)?;
            ids.insert(package.to_string(), id);
            names.append_value(package);
            rows += 1;
            if rows == BATCH_ROWS {
                writer.write(&packages_batch(&mut names)?)?;
                rows = 0;
            }
        }
        if rows > 0 {
            writer.write(&packages_batch(&mut names)?)?;
        }
        let packages = LayerFile::new(writer.into_inner()?, format::packages_file_name);
        let num_packages = ids.len() as u64;

        let metadata = vec![
            key_value(format::KEY_FORMAT_VERSION, format::FORMAT_VERSION),
            key_value(format::KEY_PACKAGES, &packages.file_name),
            key_value(format::KEY_CHANNEL, channel),
            key_value(format::KEY_SUBDIR, subdir),
            key_value(format::KEY_CREATED, created_at),
        ];
        let mut paths = None;
        let mut buffered = BTreeMap::new();
        for &kind in kinds {
            match kind {
                Kind::Paths => {
                    let mut metadata = metadata.clone();
                    metadata.push(key_value(format::KEY_KIND, kind.name()));
                    paths = Some(TableWriter::new(kind, metadata, options)?);
                }
                kind => {
                    buffered.insert(kind, Vec::new());
                }
            }
        }
        Ok(Self {
            options: *options,
            packages,
            ids,
            num_packages,
            num_paths: 0,
            created_at,
            paths,
            buffered,
            metadata,
            previous: None,
            ids_of_path: Vec::new(),
        })
    }

    /// Adds one path and the filenames of the artifacts that contain it.
    ///
    /// Paths have to be pushed in ascending order; a path without artifacts is
    /// ignored. Every filename has to be one of the artifacts of the layer.
    pub fn push<S: AsRef<str>>(
        &mut self,
        path: &str,
        packages: impl IntoIterator<Item = S>,
    ) -> Result<()> {
        if let Some(previous) = &self.previous
            && previous.as_str() >= path
        {
            return Err(LookupError::NotSorted {
                previous: previous.clone(),
                path: path.to_string(),
            });
        }

        self.ids_of_path.clear();
        for package in packages {
            let package = package.as_ref();
            let id = *self
                .ids
                .get(package)
                .ok_or_else(|| LookupError::UnknownPackage(package.to_string()))?;
            self.ids_of_path.push(id);
        }
        if self.ids_of_path.is_empty() {
            return Ok(());
        }
        self.ids_of_path.sort_unstable();
        self.ids_of_path.dedup();
        self.previous = Some(path.to_string());
        self.num_paths += 1;

        if let Some(paths) = &mut self.paths {
            paths.push(path, &self.ids_of_path)?;
        }
        for (kind, rows) in &mut self.buffered {
            rows.push((kind.key_of(path), self.ids_of_path.clone()));
        }
        Ok(())
    }

    /// Finishes all files of the layer.
    pub fn finish(mut self) -> Result<LayerFiles> {
        let mut tables = BTreeMap::new();
        if let Some(paths) = self.paths.take() {
            tables.insert(Kind::Paths, paths.finish()?);
        }
        for (kind, mut rows) in std::mem::take(&mut self.buffered) {
            rows.sort_by(|(left, _), (right, _)| left.cmp(right));
            let mut metadata = self.metadata.clone();
            metadata.push(key_value(format::KEY_KIND, kind.name()));
            let mut writer = TableWriter::new(kind, metadata, &self.options)?;
            for (key, ids) in rows {
                writer.push(&key, &ids)?;
            }
            tables.insert(kind, writer.finish()?);
        }
        Ok(LayerFiles {
            packages: self.packages,
            tables,
            created_at: self.created_at,
            num_packages: self.num_packages,
            num_paths: self.num_paths,
        })
    }
}

fn packages_batch(names: &mut StringBuilder) -> Result<RecordBatch> {
    Ok(RecordBatch::try_new(
        format::packages_schema(),
        vec![Arc::new(names.finish()) as ArrayRef],
    )?)
}

/// Collects the paths of a set of artifacts and writes them as one layer.
///
/// Unlike [`LayerWriter`] this holds all paths of the layer in memory, so it is
/// meant for the artifacts added by one run of an indexer, not for a whole
/// channel.
pub struct LayerBuilder {
    channel: String,
    subdir: String,
    /// The filename of every artifact, by id.
    packages: Vec<String>,
    /// The ids of the artifacts that contain a path.
    paths: BTreeMap<String, Vec<u32>>,
    num_pairs: u64,
}

impl LayerBuilder {
    /// A builder for a layer of `subdir` of `channel`.
    pub fn new(channel: impl Into<String>, subdir: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            subdir: subdir.into(),
            packages: Vec::new(),
            paths: BTreeMap::new(),
            num_pairs: 0,
        }
    }

    /// Adds an artifact and the paths it contains.
    ///
    /// An artifact without paths is still indexed: the layer covers it, so it is
    /// not looked at again.
    pub fn add_package(&mut self, file_name: String, paths: impl IntoIterator<Item = String>) {
        let id = u32::try_from(self.packages.len()).expect("less than 4 billion artifacts");
        self.packages.push(file_name);
        for path in paths {
            self.num_pairs += 1;
            match self.paths.get_mut(path.as_str()) {
                Some(ids) => ids.push(id),
                None => {
                    self.paths.insert(path, vec![id]);
                }
            }
        }
    }

    /// The number of artifacts added so far.
    pub fn num_packages(&self) -> usize {
        self.packages.len()
    }

    /// The number of distinct paths added so far.
    pub fn num_paths(&self) -> usize {
        self.paths.len()
    }

    /// The number of (path, artifact) pairs added so far.
    pub fn num_pairs(&self) -> u64 {
        self.num_pairs
    }

    /// The paths of all artifacts, sorted, as a stream of entries that can be
    /// merged with the entries of other layers, see [`merge_entries`].
    pub fn into_entries(self) -> impl Iterator<Item = Result<Entry>> + Send {
        let (names, remap) = sorted_packages(&self.packages);
        self.paths.into_iter().map(move |(path, ids)| {
            let mut packages: Vec<String> = ids
                .into_iter()
                .map(|id| names[remap[id as usize] as usize].clone())
                .collect();
            packages.sort();
            packages.dedup();
            Ok((path, packages))
        })
    }

    /// Writes the layer with a table of every kind, or returns `None` if no
    /// artifact was added.
    pub fn finish(self, options: &WriteOptions) -> Result<Option<LayerFiles>> {
        self.finish_with_kinds(&Kind::ALL, options)
    }

    /// Writes the layer with a table of the given kinds, or returns `None` if no
    /// artifact was added.
    pub fn finish_with_kinds(
        self,
        kinds: &[Kind],
        options: &WriteOptions,
    ) -> Result<Option<LayerFiles>> {
        if self.packages.is_empty() {
            return Ok(None);
        }
        let (names, remap) = sorted_packages(&self.packages);
        let mut writer = LayerWriter::new(&self.channel, &self.subdir, &names, kinds, options)?;
        let mut packages: Vec<u32> = Vec::new();
        for (path, ids) in &self.paths {
            packages.clear();
            packages.extend(ids.iter().map(|&id| remap[id as usize]));
            packages.sort_unstable();
            packages.dedup();
            writer.push(path, packages.iter().map(|&id| &names[id as usize]))?;
        }
        writer.finish().map(Some)
    }
}

/// Sorts and deduplicates filenames and returns the new id of every old id.
fn sorted_packages(packages: &[String]) -> (Vec<String>, Vec<u32>) {
    let mut names = packages.to_vec();
    names.sort();
    names.dedup();
    let remap = packages
        .iter()
        .map(|name| {
            names
                .binary_search(name)
                .map(|idx| idx as u32)
                .expect("every name is in the sorted names")
        })
        .collect();
    (names, remap)
}

/// One path and the filenames of the artifacts that contain it, sorted.
pub type Entry = (String, Vec<String>);

/// A sorted stream of [`Entry`], e.g. the rows of an existing layer.
pub type EntrySource = Box<dyn Iterator<Item = Result<Entry>> + Send>;

/// Reads the rows of the [`Kind::Paths`] table of a layer, sorted by path.
///
/// `packages` are the filenames of the layer's artifacts as read from its
/// packages file with [`package_names`]; the ids in the table are rows in it.
pub fn layer_entries(
    bytes: Bytes,
    packages: Arc<Vec<String>>,
) -> Result<impl Iterator<Item = Result<Entry>> + Send> {
    let mut reader = ParquetRecordBatchReaderBuilder::try_new(bytes)?
        .with_batch_size(BATCH_ROWS)
        .build()?;
    let mut rows: std::vec::IntoIter<Result<Entry>> = Vec::new().into_iter();
    Ok(std::iter::from_fn(move || {
        loop {
            if let Some(row) = rows.next() {
                return Some(row);
            }
            rows = match reader.next()? {
                Ok(batch) => entries_of(&batch, &packages).into_iter(),
                Err(err) => return Some(Err(err.into())),
            };
        }
    }))
}

/// The filenames of the artifacts a layer covers, read from its packages file.
pub fn package_names(bytes: Bytes) -> Result<Vec<String>> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(bytes)?
        .with_batch_size(BATCH_ROWS)
        .build()?;
    let mut names = Vec::new();
    for batch in reader {
        let batch = batch?;
        names.extend(
            batch
                .column(0)
                .as_string::<i32>()
                .iter()
                .flatten()
                .map(str::to_string),
        );
    }
    Ok(names)
}

/// The rows of one record batch of a lookup table, with their ids resolved.
fn entries_of(batch: &RecordBatch, packages: &[String]) -> Vec<Result<Entry>> {
    let keys = batch.column(0).as_string::<i32>();
    let ids = batch.column(1).as_list::<i32>();
    (0..batch.num_rows())
        .map(|row| {
            let values = ids.value(row);
            let names = values
                .as_primitive::<UInt32Type>()
                .iter()
                .flatten()
                .map(|id| {
                    packages
                        .get(id as usize)
                        .cloned()
                        .ok_or(LookupError::UnknownPackageId(id))
                })
                .collect::<Result<Vec<String>>>()?;
            Ok((keys.value(row).to_string(), names))
        })
        .collect()
}

/// A k-way merge of sorted [`Entry`] streams, see [`merge_entries`].
pub struct MergeEntries {
    sources: Vec<EntrySource>,
    heads: Vec<Option<Vec<String>>>,
    last: Vec<Option<String>>,
    heap: BinaryHeap<Reverse<(String, usize)>>,
    removed: BTreeSet<String>,
    started: bool,
}

/// Merges sorted [`Entry`] streams into one, combining the artifacts of equal
/// paths and dropping the artifacts in `removed`.
///
/// Paths that only occur in removed artifacts are dropped entirely. The
/// resulting stream can be handed to a [`LayerWriter`] to compact several layers
/// into one.
pub fn merge_entries(sources: Vec<EntrySource>, removed: &BTreeSet<String>) -> MergeEntries {
    let num_sources = sources.len();
    MergeEntries {
        sources,
        heads: vec![None; num_sources],
        last: vec![None; num_sources],
        heap: BinaryHeap::with_capacity(num_sources),
        removed: removed.clone(),
        started: false,
    }
}

impl MergeEntries {
    fn advance(&mut self, idx: usize) -> Result<()> {
        if let Some(entry) = self.sources[idx].next() {
            let (path, packages) = entry?;
            if let Some(last) = &self.last[idx]
                && last.as_str() >= path.as_str()
            {
                return Err(LookupError::NotSorted {
                    previous: last.clone(),
                    path,
                });
            }
            self.last[idx] = Some(path.clone());
            self.heads[idx] = Some(packages);
            self.heap.push(Reverse((path, idx)));
        }
        Ok(())
    }

    fn next_entry(&mut self) -> Result<Option<Entry>> {
        if !self.started {
            self.started = true;
            for idx in 0..self.sources.len() {
                self.advance(idx)?;
            }
        }
        loop {
            let Some(Reverse((path, idx))) = self.heap.pop() else {
                return Ok(None);
            };
            let mut packages = self.heads[idx].take().unwrap_or_default();
            self.advance(idx)?;
            let mut merged = false;
            while let Some(Reverse((next_path, _))) = self.heap.peek() {
                if *next_path != path {
                    break;
                }
                let Reverse((_, other)) = self.heap.pop().expect("just peeked");
                packages.extend(self.heads[other].take().unwrap_or_default());
                self.advance(other)?;
                merged = true;
            }
            if !self.removed.is_empty() {
                packages.retain(|package| !self.removed.contains(package));
            }
            if merged || !packages.is_sorted() {
                packages.sort();
            }
            packages.dedup();
            if !packages.is_empty() {
                return Ok(Some((path, packages)));
            }
        }
    }
}

impl Iterator for MergeEntries {
    type Item = Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_entry().transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(entries: &[(&str, &[&str])]) -> EntrySource {
        let entries: Vec<_> = entries
            .iter()
            .map(|(path, packages)| {
                Ok((
                    (*path).to_string(),
                    packages.iter().map(|p| (*p).to_string()).collect(),
                ))
            })
            .collect();
        Box::new(entries.into_iter())
    }

    fn collect(merge: MergeEntries) -> Vec<Entry> {
        merge.collect::<Result<Vec<_>>>().unwrap()
    }

    fn entry(path: &str, packages: &[&str]) -> Entry {
        (
            path.to_string(),
            packages.iter().map(|p| (*p).to_string()).collect(),
        )
    }

    /// Reads a table of a layer back into entries.
    fn table_entries(files: &LayerFiles, kind: Kind) -> Vec<Entry> {
        let packages = Arc::new(package_names(Bytes::from(files.packages.bytes.clone())).unwrap());
        layer_entries(Bytes::from(files.tables[&kind].bytes.clone()), packages)
            .unwrap()
            .collect::<Result<_>>()
            .unwrap()
    }

    #[test]
    fn merges_and_deduplicates() {
        let merged = collect(merge_entries(
            vec![
                source(&[("a", &["p1", "p2"]), ("c", &["p3"])]),
                source(&[("b", &["p7"]), ("c", &["p1", "p3"])]),
                source(&[]),
            ],
            &BTreeSet::new(),
        ));
        assert_eq!(
            merged,
            vec![
                entry("a", &["p1", "p2"]),
                entry("b", &["p7"]),
                entry("c", &["p1", "p3"]),
            ]
        );
    }

    #[test]
    fn drops_removed_packages() {
        let merged = collect(merge_entries(
            vec![source(&[("a", &["p1", "p2"]), ("b", &["p1"])])],
            &BTreeSet::from(["p1".to_string()]),
        ));
        assert_eq!(
            merged,
            vec![entry("a", &["p2"])],
            "`b` only exists in the removed artifact"
        );
    }

    #[test]
    fn rejects_unsorted_input() {
        let merged: Result<Vec<Entry>> = merge_entries(
            vec![source(&[("b", &["p"]), ("a", &["p"])])],
            &BTreeSet::new(),
        )
        .collect();
        assert!(matches!(merged, Err(LookupError::NotSorted { .. })));
    }

    #[test]
    fn rejects_unsorted_paths() {
        let mut writer = LayerWriter::new(
            "c",
            "noarch",
            ["a.conda"],
            &Kind::ALL,
            &WriteOptions::default(),
        )
        .unwrap();
        writer.push("b", ["a.conda"]).unwrap();
        assert!(matches!(
            writer.push("a", ["a.conda"]),
            Err(LookupError::NotSorted { .. })
        ));
    }

    #[test]
    fn rejects_unsorted_packages() {
        assert!(matches!(
            LayerWriter::new(
                "c",
                "noarch",
                ["b.conda", "a.conda"],
                &Kind::ALL,
                &WriteOptions::default()
            ),
            Err(LookupError::NotSorted { .. })
        ));
    }

    #[test]
    fn rejects_unknown_packages() {
        let mut writer = LayerWriter::new(
            "c",
            "noarch",
            ["a.conda"],
            &Kind::ALL,
            &WriteOptions::default(),
        )
        .unwrap();
        assert!(matches!(
            writer.push("bin/b", ["b.conda"]),
            Err(LookupError::UnknownPackage(name)) if name == "b.conda"
        ));
    }

    #[test]
    fn writes_and_reads_a_layer() {
        let mut builder = LayerBuilder::new("https://x.org/c/", "noarch");
        builder.add_package("b.conda".into(), ["bin/b".to_string(), "l/x".to_string()]);
        builder.add_package("a.conda".into(), ["l/x".to_string()]);
        builder.add_package("empty.conda".into(), []);
        assert_eq!(builder.num_packages(), 3);
        assert_eq!(builder.num_paths(), 2);
        assert_eq!(builder.num_pairs(), 3);

        let files = builder
            .finish(&WriteOptions::default())
            .unwrap()
            .expect("a layer");
        assert_eq!(files.num_packages, 3);
        assert_eq!(files.num_paths, 2);
        assert!(files.packages.file_name.starts_with("packages-"));
        for kind in Kind::ALL {
            let table = &files.tables[&kind];
            assert_eq!(
                table.file_name,
                kind.file_name(&compute_bytes_digest::<Sha256>(&table.bytes))
            );
        }

        assert_eq!(
            table_entries(&files, Kind::Paths),
            vec![
                entry("bin/b", &["b.conda"]),
                entry("l/x", &["a.conda", "b.conda"])
            ]
        );
        // The reversed keys are sorted in their own order, so `b/bin` comes
        // after `x/l`.
        assert_eq!(
            table_entries(&files, Kind::ReversedPaths),
            vec![
                entry("b/bin", &["b.conda"]),
                entry("x/l", &["a.conda", "b.conda"])
            ]
        );

        // The artifact without paths is covered by the layer nonetheless.
        let packages = package_names(Bytes::from(files.packages.bytes.clone())).unwrap();
        assert_eq!(packages, ["a.conda", "b.conda", "empty.conda"]);
    }

    #[test]
    fn writes_only_the_requested_kinds() {
        let mut builder = LayerBuilder::new("https://x.org/c/", "noarch");
        builder.add_package("a.conda".into(), ["bin/a".to_string()]);
        let files = builder
            .finish_with_kinds(&[Kind::Paths], &WriteOptions::default())
            .unwrap()
            .expect("a layer");
        assert_eq!(
            files.tables.keys().copied().collect::<Vec<_>>(),
            [Kind::Paths]
        );
    }

    /// The layout the CEP prescribes, so that a lookup only has to read a page
    /// index and one data page per column.
    #[test]
    fn a_layer_has_the_layout_of_the_cep() {
        // Enough artifacts for the packages file to have several pages.
        const PACKAGES: usize = 3 * format::PACKAGES_PAGE_ROWS;

        let mut builder = LayerBuilder::new("https://x.org/c/", "noarch");
        for i in 0..PACKAGES {
            builder.add_package(format!("p-{i:04}.conda"), [format!("bin/{i:04}")]);
        }
        let files = builder
            .finish(&WriteOptions::default())
            .unwrap()
            .expect("a layer");

        for kind in Kind::ALL {
            let metadata = parquet::file::metadata::ParquetMetaDataReader::new()
                .with_page_index_policy(parquet::file::metadata::PageIndexPolicy::Required)
                .parse_and_finish(&Bytes::from(files.tables[&kind].bytes.clone()))
                .unwrap();
            assert_eq!(metadata.file_metadata().version(), 2);

            for row_group in metadata.row_groups() {
                assert_eq!(
                    row_group.sorting_columns().map(Vec::as_slice),
                    Some(
                        [parquet::file::metadata::SortingColumn {
                            column_idx: format::KEY_LEAF as i32,
                            descending: false,
                            nulls_first: false,
                        }]
                        .as_slice()
                    ),
                    "a {kind} table is sorted by its key"
                );

                let key = row_group.column(format::KEY_LEAF);
                let key_encodings: Vec<_> = key.encodings().collect();
                assert!(key.column_index_range().is_some(), "min/max per page");
                assert!(key.offset_index_range().is_some(), "page locations");
                assert!(
                    key_encodings.contains(&parquet::basic::Encoding::DELTA_BYTE_ARRAY),
                    "sorted keys share their prefixes: {key_encodings:?}"
                );
                assert!(
                    !key_encodings.contains(&parquet::basic::Encoding::RLE_DICTIONARY),
                    "a dictionary of all keys would have to be read as a whole"
                );

                let values = row_group.column(format::VALUES_LEAF);
                let value_encodings: Vec<_> = values.encodings().collect();
                assert!(
                    value_encodings.contains(&parquet::basic::Encoding::DELTA_BINARY_PACKED),
                    "sorted ids have small deltas: {value_encodings:?}"
                );
                assert!(values.offset_index_range().is_some());
            }

            let kv = metadata
                .file_metadata()
                .key_value_metadata()
                .expect("file metadata");
            let value = |key: &str| {
                kv.iter()
                    .find(|kv| kv.key == key)
                    .and_then(|kv| kv.value.clone())
                    .unwrap_or_else(|| panic!("{key} is missing"))
            };
            assert_eq!(value(format::KEY_FORMAT_VERSION), format::FORMAT_VERSION);
            assert_eq!(value(format::KEY_KIND), kind.name());
            assert_eq!(value(format::KEY_PACKAGES), files.packages.file_name);
            assert_eq!(value(format::KEY_CHANNEL), "https://x.org/c/");
            assert_eq!(value(format::KEY_SUBDIR), "noarch");
            assert_eq!(
                value(format::KEY_CREATED).parse::<Timestamp>().unwrap(),
                files.created_at
            );
        }

        // The packages file is read by row number, so it needs an offset index
        // and small pages, but no statistics.
        let metadata = parquet::file::metadata::ParquetMetaDataReader::new()
            .with_page_index_policy(parquet::file::metadata::PageIndexPolicy::Required)
            .parse_and_finish(&Bytes::from(files.packages.bytes.clone()))
            .unwrap();
        for row_group in metadata.row_groups() {
            let package = row_group.column(format::PACKAGE_LEAF);
            assert!(package.offset_index_range().is_some(), "page locations");
            assert!(
                package.column_index_range().is_none(),
                "statistics would move the offset index away from the footer"
            );
        }
        let pages = metadata
            .page_index_for_row_group(0)
            .page_locations(format::PACKAGE_LEAF)
            .expect("an offset index")
            .len();
        assert_eq!(
            pages,
            PACKAGES.div_ceil(format::PACKAGES_PAGE_ROWS),
            "{PACKAGES} rows in pages of at most {} rows",
            format::PACKAGES_PAGE_ROWS
        );
    }

    #[test]
    fn a_layer_of_nothing_is_no_layer() {
        let builder = LayerBuilder::new("https://x.org/c/", "noarch");
        assert!(builder.finish(&WriteOptions::default()).unwrap().is_none());
    }
}
