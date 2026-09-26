//! Looking up which artifacts contain a path.
//!
//! Per layer of a subdir's index, a lookup
//!
//! 1. reads the footer of the lookup table it needs and of the packages file
//!    (one request each, the tail of the file),
//! 2. picks the row groups whose key min/max overlaps the query and loads only
//!    those row groups' page index,
//! 3. reads the key pages whose min/max overlaps the query and the value pages
//!    next to them, and keeps the keys the query actually matches,
//! 4. reads the pages of the packages file that hold the ids it found.
//!
//! A scan with a limit reads its pages in rounds of 2, 4, 8, … pages, so a
//! pattern that matches early does not pay for the whole range it could match.

mod source;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Range,
    sync::{Arc, atomic::Ordering},
};

use arrow_array::{cast::AsArray, types::UInt32Type};
use futures::{TryStreamExt, future::try_join_all};
use parquet::{
    arrow::{
        ProjectionMask,
        arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions, RowSelection},
        async_reader::ParquetRecordBatchStreamBuilder,
    },
    basic::Type as PhysicalType,
    file::{
        metadata::{ParquetMetaData, ParquetMetaDataReader, page_index::PageIndexProvider},
        page_index::{
            column_index::ColumnIndexMetaData,
            index_reader::{decode_column_index, decode_offset_index},
            offset_index::OffsetIndexMetaData,
        },
    },
};
use rattler_conda_types::Platform;
use reqwest_middleware::ClientWithMiddleware;
use source::{ByteSource, ParquetSource};
use tokio::sync::{OnceCell, RwLock};
use url::Url;

use crate::{
    LookupError, Result,
    discover::discover_manifest_url,
    format::{self, Kind},
    manifest::{self, FileRef, Manifest},
    query::{KeyRange, Query, normalize},
};

/// A page index that only holds the entries that were actually loaded.
#[derive(Debug, Default, Clone)]
struct SparsePageIndex {
    column: HashMap<(usize, usize), ColumnIndexMetaData>,
    offset: HashMap<(usize, usize), OffsetIndexMetaData>,
}

impl PageIndexProvider for SparsePageIndex {
    fn has_offset_indexes(&self) -> bool {
        !self.offset.is_empty()
    }

    fn has_column_indexes(&self) -> bool {
        !self.column.is_empty()
    }

    fn column_index(
        &self,
        row_group_idx: usize,
        column_idx: usize,
    ) -> Option<&ColumnIndexMetaData> {
        self.column.get(&(row_group_idx, column_idx))
    }

    fn offset_index(
        &self,
        row_group_idx: usize,
        column_idx: usize,
    ) -> Option<&OffsetIndexMetaData> {
        self.offset.get(&(row_group_idx, column_idx))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

type RowsByRowGroup = BTreeMap<usize, Vec<Range<u64>>>;

/// The rows of a lookup table a scan found, sorted by key.
#[derive(Debug, Default)]
struct Scan {
    /// Every matching key and the ids of the artifacts it belongs to.
    rows: Vec<(String, Vec<u32>)>,
    /// The scan stopped at its limit; there may be more matches.
    truncated: bool,
}

/// A Parquet file read with range requests: its footer and the parts of its
/// page index that were needed so far.
struct Table {
    source: Arc<ByteSource>,
    metadata: ParquetMetaData,
    index: RwLock<SparsePageIndex>,
    /// The first row of every row group, plus the total row count at the end.
    row_group_starts: Vec<u64>,
}

impl Table {
    async fn open(source: ByteSource) -> Result<Self> {
        let source = Arc::new(source);
        let metadata = read_metadata(&source).await?;
        let mut row_group_starts = vec![0];
        for row_group in metadata.row_groups() {
            row_group_starts
                .push(row_group_starts.last().expect("not empty") + row_group.num_rows() as u64);
        }
        Ok(Self {
            source,
            metadata,
            index: RwLock::new(SparsePageIndex::default()),
            row_group_starts,
        })
    }

    /// Opens a file the manifest lists and checks that it has the promised size.
    ///
    /// The manifest is the only mutable file of an index: a layer file that does
    /// not have the size the manifest promises is not the file it refers to.
    async fn open_layer_file(
        manifest_url: &Url,
        file: &FileRef,
        client: &ClientWithMiddleware,
    ) -> Result<Self> {
        let url = manifest::layer_url(manifest_url, &file.file)?;
        let table = Self::open(ByteSource::open(&url, client).await?).await?;
        if table.source.len() != file.size {
            return Err(LookupError::SizeMismatch {
                url: Box::new(url),
                expected: file.size,
                actual: table.source.len(),
            });
        }
        table.check_format_version()?;
        Ok(table)
    }

    fn url(&self) -> &Url {
        self.source.url()
    }

    fn key_value(&self, key: &str) -> Option<&str> {
        self.metadata
            .file_metadata()
            .key_value_metadata()
            .and_then(|kv| kv.iter().find(|kv| kv.key == key))
            .and_then(|kv| kv.value.as_deref())
    }

    fn invalid(&self, reason: String) -> LookupError {
        LookupError::InvalidLayer {
            url: Box::new(self.url().clone()),
            reason,
        }
    }

    fn check_format_version(&self) -> Result<()> {
        match self.key_value(format::KEY_FORMAT_VERSION) {
            Some(format::FORMAT_VERSION) => Ok(()),
            Some(version) => Err(self.invalid(format!("unsupported format version {version}"))),
            None => Err(self.invalid(format!("{} is missing", format::KEY_FORMAT_VERSION))),
        }
    }

    fn num_rows(&self) -> u64 {
        self.row_group_starts.last().copied().unwrap_or_default()
    }

    /// Creates a reader for the given rows and columns. All pages it will need
    /// are fetched up front in one batch of concurrent requests; the Parquet
    /// stream would otherwise fetch one row group after the other.
    async fn reader(
        &self,
        index: &SparsePageIndex,
        rows_by_rg: &RowsByRowGroup,
        leaves: &[usize],
    ) -> Result<ParquetRecordBatchStreamBuilder<ParquetSource>> {
        let mut ranges = Vec::new();
        for (&rg, rows) in rows_by_rg {
            let num_rows = self.metadata.row_group(rg).num_rows() as u64;
            for &leaf in leaves {
                let Some(offsets) = index.offset.get(&(rg, leaf)) else {
                    continue;
                };
                let pages = offsets.page_locations();
                // A dictionary page sits between the start of the column chunk
                // and its first data page.
                let (chunk_start, _) = self.metadata.row_group(rg).column(leaf).byte_range();
                if let Some(first) = pages.first()
                    && (first.offset as u64) > chunk_start
                {
                    ranges.push(chunk_start..first.offset as u64);
                }
                for (i, page) in pages.iter().enumerate() {
                    let start = page.first_row_index as u64;
                    let end = pages
                        .get(i + 1)
                        .map_or(num_rows, |next| next.first_row_index as u64);
                    if rows.iter().any(|rows| rows.start < end && start < rows.end) {
                        let offset = page.offset as u64;
                        ranges.push(offset..offset + page.compressed_page_size as u64);
                    }
                }
            }
        }
        let buffers = self.source.fetch(&ranges).await?;
        let cache = Arc::new(ranges.into_iter().zip(buffers).collect::<Vec<_>>());

        let metadata = Arc::new(
            self.metadata
                .clone()
                .into_builder()
                .set_page_index(Some(Arc::new(index.clone())))
                .build(),
        );
        let reader_metadata =
            ArrowReaderMetadata::try_new(metadata.clone(), ArrowReaderOptions::new())?;
        let projection = ProjectionMask::leaves(
            metadata.file_metadata().schema_descr(),
            leaves.iter().copied(),
        );
        let input = ParquetSource {
            source: self.source.clone(),
            metadata,
            cache,
        };
        Ok(
            ParquetRecordBatchStreamBuilder::new_with_metadata(input, reader_metadata)
                .with_projection(projection)
                .with_batch_size(8192),
        )
    }

    /// Builds a selection over the concatenation of the given row groups.
    fn selection(&self, rows_by_rg: &RowsByRowGroup) -> (Vec<usize>, RowSelection) {
        let mut offset = 0;
        let mut ranges = Vec::new();
        for (&rg, rows) in rows_by_rg {
            for range in rows {
                ranges.push((offset + range.start) as usize..(offset + range.end) as usize);
            }
            offset += self.metadata.row_group(rg).num_rows() as u64;
        }
        (
            rows_by_rg.keys().copied().collect(),
            RowSelection::from_consecutive_ranges(ranges.into_iter(), offset as usize),
        )
    }

    /// Row ranges (relative to the row group) of the key pages that may hold a
    /// key of `range`, one per page and in order.
    fn candidate_pages(
        &self,
        index: &SparsePageIndex,
        rg: usize,
        range: &KeyRange,
    ) -> Result<Vec<Range<u64>>> {
        let num_rows = self.metadata.row_group(rg).num_rows() as u64;
        let offsets = index
            .offset
            .get(&(rg, format::KEY_LEAF))
            .ok_or_else(|| self.invalid(format!("the offset index of row group {rg} is missing")))?
            .page_locations();
        let page_rows = |page: usize| {
            let start = offsets[page].first_row_index as u64;
            let end = offsets
                .get(page + 1)
                .map_or(num_rows, |next| next.first_row_index as u64);
            start..end
        };

        let Some(ColumnIndexMetaData::BYTE_ARRAY(column_index)) =
            index.column.get(&(rg, format::KEY_LEAF))
        else {
            // Without a column index we have to read every page.
            return Ok((0..offsets.len()).map(page_rows).collect());
        };

        Ok((0..offsets.len())
            .filter(|&page| {
                !column_index.is_null_page(page)
                    && range.overlaps(column_index.min_value(page), column_index.max_value(page))
            })
            .map(page_rows)
            .collect())
    }

    /// The rows with a key in `range` that `accept` accepts, sorted by key.
    ///
    /// With a `limit` the scan stops once it found that many rows, reading its
    /// pages in rounds of 2, 4, 8, … so that a scan over a wide range only reads
    /// what it needs.
    async fn scan(
        &self,
        range: &KeyRange,
        accept: &dyn Fn(&str) -> bool,
        limit: Option<usize>,
    ) -> Result<Scan> {
        // Row groups whose [min, max] key range overlaps the query.
        let candidates: Vec<usize> = self
            .metadata
            .row_groups()
            .iter()
            .enumerate()
            .filter(|(_, row_group)| {
                let Some(stats) = row_group.column(format::KEY_LEAF).statistics() else {
                    return true;
                };
                range.overlaps(stats.min_bytes_opt(), stats.max_bytes_opt())
            })
            .map(|(idx, _)| idx)
            .collect();
        if candidates.is_empty() {
            return Ok(Scan::default());
        }

        let mut wanted = Vec::new();
        for &rg in &candidates {
            wanted.push((rg, format::KEY_LEAF, true));
            wanted.push((rg, format::KEY_LEAF, false));
            wanted.push((rg, format::VALUES_LEAF, false));
        }
        self.load_page_index(&wanted).await?;
        let index = self.index.read().await;

        // Pages of the key column whose [min, max] overlaps the query.
        let mut pages: Vec<(usize, Range<u64>)> = Vec::new();
        for &rg in &candidates {
            pages.extend(
                self.candidate_pages(&index, rg, range)?
                    .into_iter()
                    .map(|rows| (rg, rows)),
            );
        }

        let mut scan = Scan::default();
        let mut next = 0;
        let mut round = 2;
        while next < pages.len() {
            let take = match limit {
                Some(_) => round.min(pages.len() - next),
                None => pages.len() - next,
            };
            let mut rows_by_rg = RowsByRowGroup::new();
            for (rg, rows) in &pages[next..next + take] {
                let ranges = rows_by_rg.entry(*rg).or_default();
                match ranges.last_mut() {
                    Some(last) if last.end == rows.start => last.end = rows.end,
                    _ => ranges.push(rows.clone()),
                }
            }
            next += take;
            round *= 2;

            let (row_groups, selection) = self.selection(&rows_by_rg);
            let mut stream = self
                .reader(
                    &index,
                    &rows_by_rg,
                    &[format::KEY_LEAF, format::VALUES_LEAF],
                )
                .await?
                .with_row_groups(row_groups)
                .with_row_selection(selection)
                .build()?;
            while let Some(batch) = stream.try_next().await? {
                let keys = batch.column(0).as_string::<i32>();
                let lists = batch.column(1).as_list::<i32>();
                for row in 0..batch.num_rows() {
                    let key = keys.value(row);
                    if !range.contains(key.as_bytes()) || !accept(key) {
                        continue;
                    }
                    let ids = lists
                        .value(row)
                        .as_primitive::<UInt32Type>()
                        .values()
                        .to_vec();
                    scan.rows.push((key.to_string(), ids));
                }
            }
            if let Some(limit) = limit
                && scan.rows.len() >= limit
            {
                scan.truncated = scan.rows.len() > limit || next < pages.len();
                scan.rows.truncate(limit);
                break;
            }
        }
        Ok(scan)
    }

    /// Loads the requested entries of the page index `(row group, column, is_column_index)`
    /// that aren't loaded yet, all with one batch of concurrent requests.
    async fn load_page_index(&self, wanted: &[(usize, usize, bool)]) -> Result<()> {
        let mut todo = Vec::new();
        let mut ranges = Vec::new();
        {
            let index = self.index.read().await;
            for &(rg, col, is_column_index) in wanted {
                let key = (rg, col);
                let loaded = if is_column_index {
                    index.column.contains_key(&key)
                } else {
                    index.offset.contains_key(&key)
                };
                if loaded || todo.contains(&(rg, col, is_column_index)) {
                    continue;
                }
                let chunk = self.metadata.row_group(rg).column(col);
                let range = if is_column_index {
                    chunk.column_index_range()
                } else {
                    chunk.offset_index_range()
                };
                if let Some(range) = range {
                    todo.push((rg, col, is_column_index));
                    ranges.push(range);
                }
            }
        }
        if todo.is_empty() {
            return Ok(());
        }

        let buffers = self.source.fetch(&ranges).await?;
        let mut index = self.index.write().await;
        for ((rg, col, is_column_index), bytes) in todo.into_iter().zip(buffers) {
            if is_column_index {
                index.column.insert(
                    (rg, col),
                    decode_column_index(&bytes, PhysicalType::BYTE_ARRAY)?,
                );
            } else {
                index.offset.insert((rg, col), decode_offset_index(&bytes)?);
            }
        }
        Ok(())
    }

    fn stats(&self) -> LookupStats {
        let stats = self.source.stats();
        LookupStats {
            requests: stats.requests.load(Ordering::Relaxed),
            bytes: stats.bytes.load(Ordering::Relaxed),
        }
    }
}

/// The packages file of a layer: turns package ids into artifact filenames.
struct Packages {
    table: Table,
}

impl Packages {
    /// Opens a packages file and loads the offset index of its `package` column,
    /// which arrives with the footer.
    async fn open(
        manifest_url: &Url,
        packages: &manifest::PackagesRef,
        client: &ClientWithMiddleware,
    ) -> Result<Self> {
        let file = FileRef {
            file: packages.file.clone(),
            size: packages.size,
        };
        let table = Table::open_layer_file(manifest_url, &file, client).await?;

        // Further columns are allowed, and ignored.
        let schema = table.metadata.file_metadata().schema_descr();
        if schema.num_columns() == 0
            || schema.column(format::PACKAGE_LEAF).name() != format::PACKAGE_COLUMN
        {
            return Err(table.invalid(format!(
                "expected `{}` as its first column",
                format::PACKAGE_COLUMN
            )));
        }
        if table.num_rows() != packages.count {
            return Err(table.invalid(format!(
                "has {} rows, but the manifest expects {}",
                table.num_rows(),
                packages.count
            )));
        }

        let wanted: Vec<_> = (0..table.metadata.num_row_groups())
            .map(|rg| (rg, format::PACKAGE_LEAF, false))
            .collect();
        table.load_page_index(&wanted).await?;
        Ok(Self { table })
    }

    /// The filename of every given package id.
    async fn resolve(&self, ids: impl IntoIterator<Item = u32>) -> Result<HashMap<u32, String>> {
        let mut ids: Vec<u32> = ids.into_iter().collect();
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let num_rows = self.table.num_rows();
        if u64::from(*ids.last().expect("not empty")) >= num_rows {
            return Err(LookupError::UnknownPackageId(
                *ids.last().expect("not empty"),
            ));
        }

        let mut rows_by_rg = RowsByRowGroup::new();
        for &id in &ids {
            let id = u64::from(id);
            let rg = self
                .table
                .row_group_starts
                .partition_point(|&start| start <= id)
                - 1;
            let row = id - self.table.row_group_starts[rg];
            let ranges = rows_by_rg.entry(rg).or_default();
            match ranges.last_mut() {
                Some(last) if last.end == row => last.end = row + 1,
                _ => ranges.push(row..row + 1),
            }
        }

        let index = self.table.index.read().await;
        let (row_groups, selection) = self.table.selection(&rows_by_rg);
        let mut stream = self
            .table
            .reader(&index, &rows_by_rg, &[format::PACKAGE_LEAF])
            .await?
            .with_row_groups(row_groups)
            .with_row_selection(selection)
            .build()?;
        drop(index);

        let mut names = Vec::with_capacity(ids.len());
        while let Some(batch) = stream.try_next().await? {
            names.extend(
                batch
                    .column(0)
                    .as_string::<i32>()
                    .iter()
                    .map(|name| name.unwrap_or_default().to_string()),
            );
        }
        if names.len() != ids.len() {
            return Err(self
                .table
                .invalid("the packages file has fewer rows than its footer says".to_string()));
        }
        Ok(ids.into_iter().zip(names).collect())
    }
}

/// One layer of an index. Its files are opened when a lookup first needs them,
/// so an exact lookup never reads the footer of a `reversed-paths` table.
struct LayerReader {
    manifest_url: Url,
    layer: manifest::Layer,
    client: ClientWithMiddleware,
    tables: HashMap<Kind, OnceCell<Table>>,
    packages: OnceCell<Packages>,
}

impl LayerReader {
    fn new(
        manifest_url: &Url,
        layer: &manifest::Layer,
        kinds: &[Kind],
        client: &ClientWithMiddleware,
    ) -> Self {
        Self {
            manifest_url: manifest_url.clone(),
            layer: layer.clone(),
            client: client.clone(),
            tables: kinds.iter().map(|&kind| (kind, OnceCell::new())).collect(),
            packages: OnceCell::new(),
        }
    }

    /// The lookup table of `kind`, opening it if this is the first lookup that
    /// needs it.
    async fn table(&self, kind: Kind) -> Result<&Table> {
        let cell = self
            .tables
            .get(&kind)
            .ok_or(LookupError::KindUnavailable(kind))?;
        cell.get_or_try_init(|| async {
            let file = self
                .layer
                .table(kind)
                .ok_or(LookupError::KindUnavailable(kind))?;
            let table = Table::open_layer_file(&self.manifest_url, file, &self.client).await?;
            match table.key_value(format::KEY_KIND) {
                Some(name) if name == kind.name() => {}
                Some(name) => {
                    return Err(table.invalid(format!(
                        "is a `{name}` table, but the manifest lists it as `{kind}`"
                    )));
                }
                None => return Err(table.invalid(format!("{} is missing", format::KEY_KIND))),
            }
            let schema = table.metadata.file_metadata().schema_descr();
            if schema.num_columns() != 2
                || schema.column(format::KEY_LEAF).name() != kind.key_column()
                || schema.column(format::VALUES_LEAF).path().parts()[0]
                    != format::PACKAGE_IDS_COLUMN
                || schema.column(format::VALUES_LEAF).physical_type() != PhysicalType::INT32
            {
                return Err(table.invalid(format!(
                    "expected the columns `{}` and `{}`",
                    kind.key_column(),
                    format::PACKAGE_IDS_COLUMN
                )));
            }
            Ok(table)
        })
        .await
    }

    async fn packages(&self) -> Result<&Packages> {
        self.packages
            .get_or_try_init(|| {
                Packages::open(&self.manifest_url, &self.layer.packages, &self.client)
            })
            .await
    }

    /// The paths of this layer that `query` matches, each with the filenames of
    /// the artifacts that contain it.
    async fn scan(&self, query: &Query, limit: Option<usize>) -> Result<Scanned> {
        let kind = query.kind();
        let table = self.table(kind).await?;
        let accept = |key: &str| query.matches(&kind.path_of(key));
        let scan = table.scan(query.range(), &accept, limit).await?;
        if scan.rows.is_empty() {
            return Ok(Scanned::default());
        }

        let names = self
            .packages()
            .await?
            .resolve(scan.rows.iter().flat_map(|(_, ids)| ids.iter().copied()))
            .await?;
        let mut paths = BTreeMap::new();
        for (key, ids) in scan.rows {
            let found: BTreeSet<String> = ids
                .into_iter()
                .map(|id| {
                    names
                        .get(&id)
                        .cloned()
                        .ok_or(LookupError::UnknownPackageId(id))
                })
                .collect::<Result<_>>()?;
            paths.insert(kind.path_of(&key), found);
        }
        Ok(Scanned {
            paths,
            truncated: scan.truncated,
        })
    }

    fn stats(&self) -> LookupStats {
        let mut total = LookupStats::default();
        for table in self.tables.values().filter_map(OnceCell::get) {
            total += table.stats();
        }
        if let Some(packages) = self.packages.get() {
            total += packages.table.stats();
        }
        total
    }
}

/// The paths one layer or subdir matched, with the artifacts containing them.
#[derive(Debug, Default)]
struct Scanned {
    paths: BTreeMap<String, BTreeSet<String>>,
    truncated: bool,
}

/// How much a lookup read so far.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LookupStats {
    /// The number of HTTP requests (or reads of a local file).
    pub requests: u64,
    /// The number of bytes read.
    pub bytes: u64,
}

impl std::ops::AddAssign for LookupStats {
    fn add_assign(&mut self, other: Self) {
        self.requests += other.requests;
        self.bytes += other.bytes;
    }
}

/// An artifact that contains a path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PathMatch {
    /// The subdir the artifact is in.
    pub subdir: String,
    /// The filename of the artifact, e.g. `python-3.13.1-h1234567_0.conda`.
    pub file_name: String,
    /// The base url of the channel the artifact is in.
    pub channel: String,
}

impl PathMatch {
    /// The url of the artifact.
    pub fn url(&self) -> Result<Url> {
        let channel = manifest::directory_url(&Url::parse(&self.channel)?)?;
        Ok(channel.join(&format!("{}/{}", self.subdir, self.file_name))?)
    }

    /// The name of the package the artifact belongs to.
    pub fn package_name(&self) -> &str {
        format::package_name(&self.file_name)
    }
}

/// What a search found: the matching paths and the artifacts that contain them.
#[derive(Debug, Default)]
pub struct Search {
    /// Every path the query matched, with the artifacts that contain it.
    pub paths: BTreeMap<String, Vec<PathMatch>>,
    /// The search stopped at its limit, so there may be more matches.
    pub truncated: bool,
}

impl Search {
    /// All artifacts of all matching paths, sorted and deduplicated.
    pub fn artifacts(&self) -> Vec<PathMatch> {
        let mut artifacts: Vec<PathMatch> = self.paths.values().flatten().cloned().collect();
        artifacts.sort();
        artifacts.dedup();
        artifacts
    }
}

/// The lookup index of one subdir: all layers of one manifest.
pub struct SubdirPathLookup {
    manifest_url: Url,
    manifest: Manifest,
    channel_url: Url,
    layers: Vec<LayerReader>,
}

impl SubdirPathLookup {
    /// Opens the index a manifest describes, or returns `None` if there is no
    /// manifest at `manifest_url` (the subdir is not indexed).
    ///
    /// The url may also be a `file://` url, for an index on disk.
    pub async fn open(manifest_url: &Url, client: &ClientWithMiddleware) -> Result<Option<Self>> {
        let Some(manifest) = fetch_manifest(manifest_url, client).await? else {
            return Ok(None);
        };
        Self::open_with_manifest(manifest_url, manifest, client).map(Some)
    }

    /// Opens the index of a manifest that was already fetched.
    ///
    /// The files of its layers are read when a lookup needs them.
    pub fn open_with_manifest(
        manifest_url: &Url,
        manifest: Manifest,
        client: &ClientWithMiddleware,
    ) -> Result<Self> {
        let kinds: Vec<Kind> = manifest.known_kinds().collect();
        let layers = manifest
            .layers
            .iter()
            .map(|layer| LayerReader::new(manifest_url, layer, &kinds, client))
            .collect();
        Ok(Self {
            manifest_url: manifest_url.clone(),
            channel_url: channel_url(manifest_url, &manifest)?,
            manifest,
            layers,
        })
    }

    /// The url the manifest was read from.
    pub fn manifest_url(&self) -> &Url {
        &self.manifest_url
    }

    /// The manifest of this index.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The subdir this index covers.
    pub fn subdir(&self) -> &str {
        &self.manifest.subdir
    }

    /// The base url of the channel this index covers, as the manifest spells
    /// it; may be a relative reference.
    pub fn channel(&self) -> &str {
        &self.manifest.channel
    }

    /// The base url of the channel this index covers, resolved against the url
    /// of the manifest.
    pub fn channel_url(&self) -> &Url {
        &self.channel_url
    }

    /// The number of layers of this index.
    pub fn num_layers(&self) -> usize {
        self.layers.len()
    }

    /// Whether this index can answer a query of that kind.
    pub fn has_kind(&self, kind: Kind) -> bool {
        self.manifest.has_kind(kind)
    }

    /// How much this lookup read so far.
    pub fn stats(&self) -> LookupStats {
        let mut total = LookupStats::default();
        for layer in &self.layers {
            total += layer.stats();
        }
        total
    }

    /// The filenames of the artifacts of this subdir that contain `path`,
    /// sorted and deduplicated.
    pub async fn find(&self, path: &str) -> Result<Vec<String>> {
        let query = Query::parse(normalize(path))?;
        let found = self.scan(&query, None).await?;
        let mut file_names: Vec<String> = found.paths.into_values().flatten().collect();
        file_names.sort();
        file_names.dedup();
        Ok(file_names)
    }

    /// The paths of this subdir that `query` matches, at most `limit` of them,
    /// each with the filenames of the artifacts that contain it.
    async fn scan(&self, query: &Query, limit: Option<usize>) -> Result<Scanned> {
        if !self.manifest.has_kind(query.kind()) {
            return Err(LookupError::KindUnavailable(query.kind()));
        }
        let scans = try_join_all(self.layers.iter().map(|layer| layer.scan(query, limit))).await?;

        let mut found = Scanned::default();
        for scan in scans {
            found.truncated |= scan.truncated;
            for (path, artifacts) in scan.paths {
                found.paths.entry(path).or_default().extend(artifacts);
            }
        }
        // An artifact that is no longer part of the channel is still in the
        // layer that indexed it.
        if !self.manifest.removed.is_empty() {
            for artifacts in found.paths.values_mut() {
                artifacts.retain(|artifact| !self.manifest.removed.contains(artifact));
            }
            found.paths.retain(|_, artifacts| !artifacts.is_empty());
        }
        if let Some(limit) = limit
            && found.paths.len() > limit
        {
            found.truncated = true;
            found.paths = std::mem::take(&mut found.paths)
                .into_iter()
                .take(limit)
                .collect();
        }
        Ok(found)
    }
}

/// The lookup index of several subdirs of a channel.
pub struct PathLookup {
    subdirs: Vec<SubdirPathLookup>,
}

/// The base url of a channel, resolved against the url of its manifest.
///
/// A manifest may spell its channel as a relative reference, which a channel
/// that does not know its own url has to do. The default is the channel the
/// index is published in: `<channel>/<subdir>/lookup/manifest.json`.
fn channel_url(manifest_url: &Url, manifest: &Manifest) -> Result<Url> {
    let channel = if manifest.channel.is_empty() {
        "../../"
    } else {
        &manifest.channel
    };
    Ok(manifest::directory_url(&manifest_url.join(channel)?)?)
}

impl PathLookup {
    /// Opens the indices the channel advertises for the given subdirs.
    ///
    /// Subdirs that have no index (their repodata does not set
    /// `info.lookup_url`) are skipped, see [`discover_manifest_url`].
    pub async fn for_channel(
        channel_base: &Url,
        subdirs: &[Platform],
        client: ClientWithMiddleware,
    ) -> Result<Self> {
        let subdirs = try_join_all(subdirs.iter().map(|subdir| {
            let client = client.clone();
            async move {
                let Some(url) =
                    discover_manifest_url(channel_base, &subdir.to_string(), &client).await?
                else {
                    return Ok(None);
                };
                SubdirPathLookup::open(&url, &client).await
            }
        }))
        .await?;
        Ok(Self {
            subdirs: subdirs.into_iter().flatten().collect(),
        })
    }

    /// Opens `<base>/<subdir>/lookup/manifest.json` for every subdir, skipping
    /// the ones that don't exist.
    ///
    /// This is the escape hatch for an index that is not advertised by the
    /// channel it belongs to, e.g. one on disk or on a different host.
    pub async fn for_index_base(
        base: &Url,
        subdirs: &[Platform],
        client: ClientWithMiddleware,
    ) -> Result<Self> {
        let subdirs = try_join_all(subdirs.iter().map(|subdir| {
            let client = client.clone();
            async move {
                let url = manifest::manifest_url(base, &subdir.to_string())?;
                SubdirPathLookup::open(&url, &client).await
            }
        }))
        .await?;
        Ok(Self {
            subdirs: subdirs.into_iter().flatten().collect(),
        })
    }

    /// Opens a single manifest.
    pub async fn for_manifest(manifest_url: &Url, client: ClientWithMiddleware) -> Result<Self> {
        Ok(Self {
            subdirs: SubdirPathLookup::open(manifest_url, &client)
                .await?
                .into_iter()
                .collect(),
        })
    }

    /// The indices that were opened; empty if no subdir is indexed.
    pub fn subdirs(&self) -> &[SubdirPathLookup] {
        &self.subdirs
    }

    /// Whether no subdir has an index.
    pub fn is_empty(&self) -> bool {
        self.subdirs.is_empty()
    }

    /// The kinds of queries no opened subdir can answer.
    pub fn missing_kinds(&self, kind: Kind) -> bool {
        !self.subdirs.is_empty() && !self.subdirs.iter().any(|subdir| subdir.has_kind(kind))
    }

    /// How much this lookup read so far.
    pub fn stats(&self) -> LookupStats {
        let mut total = LookupStats::default();
        for subdir in &self.subdirs {
            total += subdir.stats();
        }
        total
    }

    /// The artifacts that contain `path`, across all subdirs.
    pub async fn find(&self, path: &str) -> Result<Vec<PathMatch>> {
        let mut matches = try_join_all(self.subdirs.iter().map(|subdir| async move {
            let file_names = subdir.find(path).await?;
            Ok::<_, LookupError>(
                file_names
                    .into_iter()
                    .map(|file_name| subdir.path_match(file_name))
                    .collect::<Vec<_>>(),
            )
        }))
        .await?
        .concat();
        matches.sort();
        Ok(matches)
    }

    /// The paths that `query` matches, across all subdirs, and the artifacts
    /// that contain them.
    ///
    /// At most `limit` paths are reported; [`Search::truncated`] says whether
    /// there may be more.
    pub async fn search(&self, query: &Query, limit: usize) -> Result<Search> {
        let scans = try_join_all(self.subdirs.iter().map(|subdir| async move {
            let found = subdir.scan(query, Some(limit)).await?;
            Ok::<_, LookupError>((subdir, found))
        }))
        .await?;

        let mut search = Search::default();
        for (subdir, found) in scans {
            search.truncated |= found.truncated;
            for (path, artifacts) in found.paths {
                search.paths.entry(path).or_default().extend(
                    artifacts
                        .into_iter()
                        .map(|file_name| subdir.path_match(file_name)),
                );
            }
        }
        if search.paths.len() > limit {
            search.truncated = true;
            search.paths = std::mem::take(&mut search.paths)
                .into_iter()
                .take(limit)
                .collect();
        }
        for artifacts in search.paths.values_mut() {
            artifacts.sort();
        }
        Ok(search)
    }
}

impl SubdirPathLookup {
    fn path_match(&self, file_name: String) -> PathMatch {
        PathMatch {
            subdir: self.subdir().to_string(),
            file_name,
            channel: self.channel_url().to_string(),
        }
    }
}

/// Fetches and validates a manifest, or returns `None` if it does not exist.
async fn fetch_manifest(url: &Url, client: &ClientWithMiddleware) -> Result<Option<Manifest>> {
    let bytes = match url.scheme() {
        "file" => {
            let path = url
                .to_file_path()
                .map_err(|()| LookupError::InvalidFileUrl(Box::new(url.clone())))?;
            match tokio::fs::read(&path).await {
                Ok(bytes) => bytes,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(source) => return Err(LookupError::Io { path, source }),
            }
        }
        "http" | "https" => {
            let response =
                client
                    .get(url.clone())
                    .send()
                    .await
                    .map_err(|source| LookupError::Http {
                        url: Box::new(url.clone()),
                        source,
                    })?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            let response = response
                .error_for_status()
                .map_err(|err| LookupError::HttpStatus {
                    url: Box::new(url.clone()),
                    status: err.status().unwrap_or_default(),
                })?;
            response
                .bytes()
                .await
                .map_err(|err| LookupError::Http {
                    url: Box::new(url.clone()),
                    source: err.into(),
                })?
                .to_vec()
        }
        scheme => return Err(LookupError::UnsupportedScheme(scheme.to_string())),
    };
    Manifest::from_bytes(&bytes)
        .map(Some)
        .map_err(|source| LookupError::InvalidManifest {
            url: Box::new(url.clone()),
            source,
        })
}

/// Reads the footer, using the tail that was fetched when opening the source.
async fn read_metadata(source: &ByteSource) -> Result<ParquetMetaData> {
    let invalid = |reason: &str| LookupError::InvalidLayer {
        url: Box::new(source.url().clone()),
        reason: reason.to_string(),
    };

    let len = source.len();
    if len < 12 {
        return Err(invalid("the file is too small to be a parquet file"));
    }
    let tail_len = source::TAIL_SIZE.min(len);
    let tail_range = len - tail_len..len;
    let tail = source
        .fetch(std::slice::from_ref(&tail_range))
        .await?
        .pop()
        .expect("one range was requested");

    let footer = &tail[tail.len() - 8..];
    if &footer[4..] != b"PAR1" {
        return Err(invalid("the file is not a parquet file"));
    }
    let metadata_len = u64::from(u32::from_le_bytes(footer[..4].try_into().expect("4 bytes")));
    if metadata_len + 8 > len {
        return Err(invalid("the parquet footer is invalid"));
    }

    let metadata = if metadata_len + 8 <= tail_len {
        let start = (tail_len - 8 - metadata_len) as usize;
        tail.slice(start..tail.len() - 8)
    } else {
        let metadata_range = len - 8 - metadata_len..len - 8;
        source
            .fetch(std::slice::from_ref(&metadata_range))
            .await?
            .pop()
            .expect("one range was requested")
    };
    Ok(ParquetMetaDataReader::decode_metadata(&metadata)?)
}
