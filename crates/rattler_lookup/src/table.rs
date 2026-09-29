//! Reading one lookup table or packages file of a layer with as few byte
//! ranges as possible.
//!
//! 1. Read the footer of the file (the tail fetched when the source is
//!    opened).
//! 2. Pick the row groups whose key min/max overlaps the queried key range
//!    and load only their page index.
//! 3. Read the pages of the key column whose min/max overlaps the range and
//!    the value pages covering the same rows, and pick the matching rows.
//!
//! An exact lookup is a range containing one key. A prefix scan (`**/zlib.h`
//! in `reversed-paths`) reads its pages in rounds of growing size until
//! enough matching rows are found.

use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    sync::Arc,
};

use arrow_array::{cast::AsArray, types::UInt32Type};
use bytes::Bytes;
use futures::TryStreamExt;
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
use reqwest_middleware::ClientWithMiddleware;

use crate::{
    Kind, Location, LookupError,
    format::{self, KEY_LEAF, PACKAGE_LEAF, VALUES_LEAF},
    manifest::{FileRef, Layer, PackagesRef},
    source::{ByteSource, ParquetSource, TAIL_SIZE},
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

/// Row ranges (relative to the row group) per row group.
type RowsByRowGroup = BTreeMap<usize, Vec<Range<u64>>>;

/// A range of keys, compared bytewise: a single key, or all keys with a
/// prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRange {
    start: Vec<u8>,
    /// Exclusive end; `None` is unbounded.
    end: Option<Vec<u8>>,
}

impl KeyRange {
    /// The range containing exactly `key`.
    pub fn exact(key: &str) -> Self {
        let mut end = key.as_bytes().to_vec();
        end.push(0);
        Self {
            start: key.as_bytes().to_vec(),
            end: Some(end),
        }
    }

    /// All keys starting with `prefix`.
    pub fn prefix(prefix: &str) -> Self {
        let mut end = prefix.as_bytes().to_vec();
        // The smallest key after all keys with the prefix.
        while end.last() == Some(&0xff) {
            end.pop();
        }
        let end = match end.last_mut() {
            Some(last) => {
                *last += 1;
                Some(end)
            }
            None => None,
        };
        Self {
            start: prefix.as_bytes().to_vec(),
            end,
        }
    }

    /// The first key of the range.
    pub fn start(&self) -> &[u8] {
        &self.start
    }

    /// Whether `[min, max]` (lower and upper bounds, e.g. truncated
    /// statistics) may contain a key of the range.
    fn overlaps(&self, min: Option<&[u8]>, max: Option<&[u8]>) -> bool {
        max.is_none_or(|max| max >= self.start.as_slice())
            && match (&self.end, min) {
                (Some(end), Some(min)) => min < end.as_slice(),
                _ => true,
            }
    }

    /// Whether `key` lies in the range.
    pub fn contains(&self, key: &[u8]) -> bool {
        key >= self.start.as_slice() && self.end.as_ref().is_none_or(|end| key < end.as_slice())
    }
}

/// Matching rows of a scan, sorted by key.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scan {
    /// The matching keys with their package ids.
    pub rows: Vec<(String, Vec<u32>)>,
    /// The scan stopped at the limit; there may be more matches.
    pub truncated: bool,
}

/// A Parquet file read with byte ranges: its footer and the parts of its page
/// index that were needed so far.
struct Table {
    source: Arc<ByteSource>,
    metadata: ParquetMetaData,
    index: SparsePageIndex,
    /// The first row of every row group, plus the total row count at the end.
    row_group_starts: Vec<u64>,
    /// Pages read so far (a scan in several rounds reads a page only once).
    pages: HashMap<Range<u64>, Bytes>,
}

impl Table {
    async fn open(source: ByteSource) -> Result<Self, LookupError> {
        let source = Arc::new(source);
        let metadata = read_metadata(&source).await?;
        let mut row_group_starts = vec![0];
        for rg in metadata.row_groups() {
            row_group_starts
                .push(row_group_starts.last().expect("non-empty") + rg.num_rows() as u64);
        }
        Ok(Self {
            source,
            metadata,
            index: SparsePageIndex::default(),
            row_group_starts,
            pages: HashMap::new(),
        })
    }

    fn location(&self) -> Location {
        self.source.location()
    }

    fn key_value(&self, key: &str) -> Option<&str> {
        self.metadata
            .file_metadata()
            .key_value_metadata()
            .and_then(|kv| kv.iter().find(|kv| kv.key == key))
            .and_then(|kv| kv.value.as_deref())
    }

    fn num_rows(&self) -> u64 {
        self.row_group_starts.last().copied().unwrap_or_default()
    }

    fn check_format_version(&self) -> Result<(), LookupError> {
        match self.key_value(format::METADATA_FORMAT_VERSION) {
            Some(format::FORMAT_VERSION) => Ok(()),
            Some(other) => Err(LookupError::invalid_file(
                &self.location(),
                format!("unsupported {} {other}", format::METADATA_FORMAT_VERSION),
            )),
            None => Err(LookupError::invalid_file(
                &self.location(),
                format!("{} is missing", format::METADATA_FORMAT_VERSION),
            )),
        }
    }

    /// Creates a reader for the given rows and columns. All pages it will
    /// need are fetched up front in one batch of concurrent requests; the
    /// Parquet stream would otherwise fetch one row group after the other.
    async fn reader(
        &mut self,
        rows_by_rg: &RowsByRowGroup,
        leaves: &[usize],
    ) -> Result<ParquetRecordBatchStreamBuilder<ParquetSource>, LookupError> {
        let mut ranges = Vec::new();
        for (&rg, rows) in rows_by_rg {
            let num_rows = self.metadata.row_group(rg).num_rows() as u64;
            for &leaf in leaves {
                let Some(offsets) = self.index.offset.get(&(rg, leaf)) else {
                    continue;
                };
                let pages = offsets.page_locations();
                // A dictionary page sits between the start of the column
                // chunk and its first data page. Writers should not use
                // dictionaries, but readers must cope with them.
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
                    if rows.iter().any(|r| r.start < end && start < r.end) {
                        let offset = page.offset as u64;
                        ranges.push(offset..offset + page.compressed_page_size as u64);
                    }
                }
            }
        }
        ranges.sort_by_key(|r| (r.start, r.end));
        ranges.dedup();
        let missing: Vec<Range<u64>> = ranges
            .iter()
            .filter(|r| !self.pages.contains_key(*r))
            .cloned()
            .collect();
        let buffers = self.source.fetch(&missing).await?;
        self.pages.extend(missing.into_iter().zip(buffers));
        let cache = Arc::new(
            ranges
                .into_iter()
                .map(|r| {
                    let bytes = self.pages[&r].clone();
                    (r, bytes)
                })
                .collect::<Vec<_>>(),
        );

        let metadata = Arc::new(
            self.metadata
                .clone()
                .into_builder()
                .set_page_index(Some(Arc::new(self.index.clone())))
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

    /// Row ranges (relative to the row group) of the key pages that may
    /// contain keys of `range`, one per page, in order.
    fn candidate_pages(&self, rg: usize, range: &KeyRange) -> Result<Vec<Range<u64>>, LookupError> {
        let num_rows = self.metadata.row_group(rg).num_rows() as u64;
        let offsets = self
            .index
            .offset
            .get(&(rg, KEY_LEAF))
            .ok_or_else(|| {
                LookupError::invalid_file(&self.location(), "the key column has no offset index")
            })?
            .page_locations();
        let page_rows = |page: usize| {
            let start = offsets[page].first_row_index as u64;
            let end = offsets
                .get(page + 1)
                .map_or(num_rows, |next| next.first_row_index as u64);
            start..end
        };

        let Some(ColumnIndexMetaData::BYTE_ARRAY(column_index)) =
            self.index.column.get(&(rg, KEY_LEAF))
        else {
            // Without a column index every page has to be read.
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

    /// Loads the requested entries of the page index `(row group, column,
    /// is_column_index)` that aren't loaded yet, all with one batch of
    /// concurrent requests.
    async fn load_page_index(
        &mut self,
        wanted: &[(usize, usize, bool)],
    ) -> Result<(), LookupError> {
        let mut todo = Vec::new();
        let mut ranges = Vec::new();
        for &(rg, col, is_column_index) in wanted {
            let key = (rg, col);
            let loaded = if is_column_index {
                self.index.column.contains_key(&key)
            } else {
                self.index.offset.contains_key(&key)
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

        let buffers = self.source.fetch(&ranges).await?;
        for ((rg, col, is_column_index), bytes) in todo.into_iter().zip(buffers) {
            if is_column_index {
                self.index.column.insert(
                    (rg, col),
                    decode_column_index(&bytes, PhysicalType::BYTE_ARRAY)?,
                );
            } else {
                self.index
                    .offset
                    .insert((rg, col), decode_offset_index(&bytes)?);
            }
        }
        Ok(())
    }
}

/// One lookup table of a layer.
pub struct LookupTable {
    table: Table,
    kind: Kind,
}

impl LookupTable {
    /// Opens a lookup table at a location. If `expected_size` is given (from
    /// the manifest), the file must have that size.
    pub async fn open(
        location: &Location,
        expected_size: Option<u64>,
        client: &ClientWithMiddleware,
    ) -> Result<Self, LookupError> {
        Self::from_source(ByteSource::open(location, client).await?, expected_size).await
    }

    /// Opens a lookup table over an already opened source.
    pub async fn from_source(
        source: ByteSource,
        expected_size: Option<u64>,
    ) -> Result<Self, LookupError> {
        check_size(&source, expected_size)?;
        let table = Table::open(source).await?;
        let location = table.location();
        table.check_format_version()?;
        let kind = match table.key_value(format::METADATA_KIND) {
            Some(name) => Kind::from_name(name).ok_or_else(|| {
                LookupError::invalid_file(&location, format!("the kind `{name}` is not supported"))
            })?,
            None => {
                return Err(LookupError::invalid_file(
                    &location,
                    format!("{} is missing", format::METADATA_KIND),
                ));
            }
        };
        let schema = table.metadata.file_metadata().schema_descr();
        let valid_columns = schema.num_columns() == 2
            && schema.column(KEY_LEAF).name() == kind.key_column()
            && schema.column(KEY_LEAF).physical_type() == PhysicalType::BYTE_ARRAY
            && schema.column(VALUES_LEAF).path().parts()[0] == format::PACKAGE_IDS_COLUMN
            && schema.column(VALUES_LEAF).physical_type() == PhysicalType::INT32;
        if !valid_columns {
            return Err(LookupError::invalid_file(
                &location,
                format!("it does not have the columns of a `{kind}` table"),
            ));
        }
        Ok(Self { table, kind })
    }

    /// Opens the table of `kind` of a layer listed in a manifest at
    /// `manifest_location`.
    pub async fn open_layer(
        manifest_location: &Location,
        layer: &Layer,
        kind: Kind,
        client: &ClientWithMiddleware,
    ) -> Result<Self, LookupError> {
        let file: &FileRef = layer.table(kind).ok_or_else(|| LookupError::MissingKind {
            location: manifest_location.clone(),
            kind,
        })?;
        let location = manifest_location.sibling(&file.file);
        let table = Self::open(&location, Some(file.size), client).await?;
        if table.kind != kind {
            return Err(LookupError::invalid_file(
                &location,
                format!(
                    "it is a `{}` table, but the manifest lists it as `{kind}`",
                    table.kind
                ),
            ));
        }
        Ok(table)
    }

    /// The kind of the table.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The name of the layer's packages file, from the table's metadata.
    pub fn packages_file(&self) -> Option<&str> {
        self.table.key_value(format::METADATA_PACKAGES)
    }

    /// The subdir the table belongs to, from its metadata.
    pub fn subdir(&self) -> Option<&str> {
        self.table.key_value(format::METADATA_SUBDIR)
    }

    /// The channel the table belongs to, from its metadata.
    pub fn channel(&self) -> Option<&str> {
        self.table.key_value(format::METADATA_CHANNEL)
    }

    /// The number of rows.
    pub fn num_rows(&self) -> u64 {
        self.table.num_rows()
    }

    /// Requests and bytes read so far.
    pub fn stats(&self) -> (u64, u64) {
        let stats = self.table.source.stats();
        (stats.requests(), stats.bytes())
    }

    /// Returns the package ids of `key`, or `None` if the key is not in the
    /// table.
    pub async fn find(&mut self, key: &str) -> Result<Option<Vec<u32>>, LookupError> {
        let scan = self.scan(&KeyRange::exact(key), |k| k == key, None).await?;
        let mut rows = scan.rows.into_iter().map(|(_, ids)| ids);
        let mut ids = rows.next();
        if let Some(ids) = &mut ids {
            rows.for_each(|row| ids.extend(row));
            ids.sort_unstable();
            ids.dedup();
        }
        Ok(ids)
    }

    /// Returns the rows with keys in `range` that `filter` accepts, sorted by
    /// key. With a `limit`, stops once that many rows were found: pages are
    /// then read in rounds of 2, 4, 8, ... pages, so a scan over a wide range
    /// only reads what it needs.
    pub async fn scan(
        &mut self,
        range: &KeyRange,
        filter: impl Fn(&str) -> bool,
        limit: Option<usize>,
    ) -> Result<Scan, LookupError> {
        // Row groups whose [min, max] key range overlaps the range.
        let candidates: Vec<usize> = self
            .table
            .metadata
            .row_groups()
            .iter()
            .enumerate()
            .filter(|(_, rg)| {
                let Some(stats) = rg.column(KEY_LEAF).statistics() else {
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
            wanted.push((rg, KEY_LEAF, true));
            wanted.push((rg, KEY_LEAF, false));
            wanted.push((rg, VALUES_LEAF, false));
        }
        self.table.load_page_index(&wanted).await?;

        // Pages of the key column whose [min, max] overlaps the range.
        let mut pages: Vec<(usize, Range<u64>)> = Vec::new();
        for &rg in &candidates {
            pages.extend(
                self.table
                    .candidate_pages(rg, range)?
                    .into_iter()
                    .map(|rows| (rg, rows)),
            );
        }

        let mut result = Scan::default();
        let mut next = 0;
        let mut round = 2;
        while next < pages.len() {
            let take = match limit {
                Some(_) => round.min(pages.len() - next),
                None => pages.len() - next,
            };
            let mut rows_by_rg: RowsByRowGroup = BTreeMap::new();
            for (rg, rows) in &pages[next..next + take] {
                let ranges = rows_by_rg.entry(*rg).or_default();
                match ranges.last_mut() {
                    Some(last) if last.end == rows.start => last.end = rows.end,
                    _ => ranges.push(rows.clone()),
                }
            }
            next += take;
            round *= 2;

            let (row_groups, selection) = self.table.selection(&rows_by_rg);
            let mut stream = self
                .table
                .reader(&rows_by_rg, &[KEY_LEAF, VALUES_LEAF])
                .await?
                .with_row_groups(row_groups)
                .with_row_selection(selection)
                .build()?;
            while let Some(batch) = stream.try_next().await? {
                let keys = batch.column(0).as_string::<i32>();
                let lists = batch.column(1).as_list::<i32>();
                for row in 0..batch.num_rows() {
                    let key = keys.value(row);
                    if !range.contains(key.as_bytes()) || !filter(key) {
                        continue;
                    }
                    let ids = lists.value(row);
                    let ids = ids.as_primitive::<UInt32Type>().values().to_vec();
                    result.rows.push((key.to_string(), ids));
                }
            }
            if let Some(limit) = limit
                && result.rows.len() >= limit
            {
                result.truncated = result.rows.len() > limit || next < pages.len();
                result.rows.truncate(limit);
                break;
            }
        }
        Ok(result)
    }
}

/// The packages file of a layer: resolves package ids to filenames.
pub struct PackagesFile {
    table: Table,
}

impl PackagesFile {
    /// Opens a packages file and loads the offset index of its column (which
    /// is usually part of the tail fetched with the footer).
    pub async fn open(
        location: &Location,
        expected_size: Option<u64>,
        client: &ClientWithMiddleware,
    ) -> Result<Self, LookupError> {
        Self::from_source(ByteSource::open(location, client).await?, expected_size).await
    }

    /// Opens a packages file over an already opened source.
    pub async fn from_source(
        source: ByteSource,
        expected_size: Option<u64>,
    ) -> Result<Self, LookupError> {
        check_size(&source, expected_size)?;
        let mut table = Table::open(source).await?;
        table.check_format_version()?;
        let schema = table.metadata.file_metadata().schema_descr();
        // Further columns are allowed and ignored.
        let valid = schema.num_columns() >= 1
            && schema.column(PACKAGE_LEAF).name() == format::PACKAGE_COLUMN
            && schema.column(PACKAGE_LEAF).physical_type() == PhysicalType::BYTE_ARRAY;
        if !valid {
            return Err(LookupError::invalid_file(
                &table.location(),
                format!("it does not have the column `{}`", format::PACKAGE_COLUMN),
            ));
        }
        let wanted: Vec<_> = (0..table.metadata.num_row_groups())
            .map(|rg| (rg, PACKAGE_LEAF, false))
            .collect();
        table.load_page_index(&wanted).await?;
        Ok(Self { table })
    }

    /// Opens the packages file of a layer listed in a manifest at
    /// `manifest_location`.
    pub async fn open_layer(
        manifest_location: &Location,
        layer: &Layer,
        client: &ClientWithMiddleware,
    ) -> Result<Self, LookupError> {
        let packages: &PackagesRef = &layer.packages;
        let location = manifest_location.sibling(&packages.file);
        let file = Self::open(&location, Some(packages.size), client).await?;
        if file.num_rows() != packages.count {
            return Err(LookupError::invalid_file(
                &location,
                format!(
                    "it has {} rows, but the manifest expects {}",
                    file.num_rows(),
                    packages.count
                ),
            ));
        }
        Ok(file)
    }

    /// The number of artifacts.
    pub fn num_rows(&self) -> u64 {
        self.table.num_rows()
    }

    /// Requests and bytes read so far.
    pub fn stats(&self) -> (u64, u64) {
        let stats = self.table.source.stats();
        (stats.requests(), stats.bytes())
    }

    /// Returns the filename of each of the given package ids.
    pub async fn resolve(
        &mut self,
        ids: impl IntoIterator<Item = u32>,
    ) -> Result<HashMap<u32, String>, LookupError> {
        let mut ids: Vec<u32> = ids.into_iter().collect();
        ids.sort_unstable();
        ids.dedup();
        let Some(&last) = ids.last() else {
            return Ok(HashMap::new());
        };
        let num_rows = self.table.num_rows();
        if u64::from(last) >= num_rows {
            return Err(LookupError::invalid_file(
                &self.table.location(),
                format!("a table links to row {last}, but the packages file has {num_rows} rows"),
            ));
        }
        let mut rows_by_rg: RowsByRowGroup = BTreeMap::new();
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
        let (row_groups, selection) = self.table.selection(&rows_by_rg);
        let mut stream = self
            .table
            .reader(&rows_by_rg, &[PACKAGE_LEAF])
            .await?
            .with_row_groups(row_groups)
            .with_row_selection(selection)
            .build()?;
        let mut values = Vec::with_capacity(ids.len());
        while let Some(batch) = stream.try_next().await? {
            values.extend(
                batch
                    .column(0)
                    .as_string::<i32>()
                    .iter()
                    .map(|v| v.unwrap_or_default().to_string()),
            );
        }
        if values.len() != ids.len() {
            return Err(LookupError::invalid_file(
                &self.table.location(),
                "the packages file is truncated",
            ));
        }
        Ok(ids.into_iter().zip(values).collect())
    }

    /// Returns all filenames, in package id order.
    pub async fn read_all(&mut self) -> Result<Vec<String>, LookupError> {
        let num_rows = self.table.num_rows();
        if num_rows == 0 {
            return Ok(Vec::new());
        }
        let ids = 0..u32::try_from(num_rows).map_err(|_overflow| {
            LookupError::invalid_file(&self.table.location(), "more than 2^32 rows")
        })?;
        let resolved = self.resolve(ids.clone()).await?;
        Ok(ids.map(|id| resolved[&id].clone()).collect())
    }
}

fn check_size(source: &ByteSource, expected: Option<u64>) -> Result<(), LookupError> {
    if let Some(expected) = expected
        && source.len() != expected
    {
        return Err(LookupError::SizeMismatch {
            location: source.location(),
            expected,
            actual: source.len(),
        });
    }
    Ok(())
}

/// Reads the footer, using the tail that was fetched when the source was
/// opened.
async fn read_metadata(source: &ByteSource) -> Result<ParquetMetaData, LookupError> {
    let location = source.location();
    let len = source.len();
    if len < 12 {
        return Err(LookupError::invalid_file(
            &location,
            "the file is too small to be a Parquet file",
        ));
    }
    let tail_len = TAIL_SIZE.min(len);
    let tail = source
        .fetch(std::slice::from_ref(&(len - tail_len..len)))
        .await?
        .pop()
        .expect("one range");

    let footer = &tail[tail.len() - 8..];
    if &footer[4..] != b"PAR1" {
        return Err(LookupError::invalid_file(&location, "not a Parquet file"));
    }
    let metadata_len = u64::from(u32::from_le_bytes(footer[..4].try_into().expect("4 bytes")));
    if metadata_len + 8 > len {
        return Err(LookupError::invalid_file(
            &location,
            "invalid Parquet footer",
        ));
    }

    let metadata = if metadata_len + 8 <= tail_len {
        let start = (tail_len - 8 - metadata_len) as usize;
        tail.slice(start..tail.len() - 8)
    } else {
        let start = len - 8 - metadata_len;
        source
            .fetch(std::slice::from_ref(&(start..len - 8)))
            .await?
            .pop()
            .expect("one range")
    };
    Ok(ParquetMetaDataReader::decode_metadata(&metadata)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_ranges() {
        let range = KeyRange::prefix("zlib.h/include");
        assert!(range.contains(b"zlib.h/include"));
        assert!(range.contains(b"zlib.h/include/Library"));
        assert!(!range.contains(b"zlib.h/includf"));
        assert!(!range.contains(b"zlib.h"));
        // Pages [min, max] overlapping the range, with truncated bounds.
        assert!(range.overlaps(Some(b"a"), Some(b"zz")));
        assert!(range.overlaps(Some(b"zlib.h/include/x"), Some(b"zz")));
        assert!(!range.overlaps(Some(b"zlib.h/includf"), Some(b"zz")));
        assert!(!range.overlaps(Some(b"a"), Some(b"zlib.h/in")));
        assert!(range.overlaps(None, None));

        let unbounded = KeyRange::prefix("\u{10ffff}");
        assert!(unbounded.end.is_none() || unbounded.contains("\u{10ffff}x".as_bytes()));
        assert!(KeyRange::prefix("").contains(b"anything"));
    }

    #[test]
    fn exact_ranges() {
        let range = KeyRange::exact("bin/python");
        assert!(range.contains(b"bin/python"));
        assert!(!range.contains(b"bin/python3"));
        assert!(!range.contains(b"bin/pytho"));
        assert!(range.overlaps(Some(b"bin/python"), Some(b"bin/python")));
        assert!(!range.overlaps(Some(b"bin/python3"), Some(b"bin/z")));
    }
}
