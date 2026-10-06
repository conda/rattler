//! A [`Read`] + [`Seek`] view of a remote file of which only some byte ranges
//! have been fetched.
//!
//! The synchronous readers of `rattler_package_streaming` (ZIP, zstd, tar)
//! run over a [`SparseReader`]. A read of bytes that have not been fetched
//! fails and records the offset, so the caller can fetch the missing range
//! and run the reader again; see [`Fetched::missing_range`].

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    ops::Range,
    rc::Rc,
};

use bytes::Bytes;

/// The byte ranges of a file fetched so far.
pub(crate) struct Fetched {
    len: u64,
    /// Fetched ranges by start offset. Ranges may overlap, e.g. when a
    /// server answered a range request with the whole file.
    blocks: BTreeMap<u64, Bytes>,
}

impl Fetched {
    pub fn new(len: u64) -> Self {
        Self {
            len,
            blocks: BTreeMap::new(),
        }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn insert(&mut self, start: u64, bytes: Bytes) {
        if !bytes.is_empty() {
            // A server may stop honoring ranges or return the full entity
            // after it changed. Do not let older sparse blocks shadow bytes
            // from this self-contained response.
            if start == 0 && bytes.len() as u64 == self.len {
                self.blocks.clear();
            }
            self.blocks.insert(start, bytes);
        }
    }

    /// The fetched bytes from `offset` to the end of the block holding it.
    fn block_at(&self, offset: u64) -> Option<Bytes> {
        self.blocks
            .range(..=offset)
            .rev()
            .find_map(|(&start, block)| {
                let rel = usize::try_from(offset - start).ok()?;
                (rel < block.len()).then(|| block.slice(rel..))
            })
    }

    /// The range to fetch after a read missed at `offset`: up to the next
    /// fetched block, or the end of the file. Filling the whole gap fetches a
    /// ZIP member in one request rather than one per read.
    /// Whether the byte at `offset` has been fetched.
    pub fn contains(&self, offset: u64) -> bool {
        self.block_at(offset).is_some()
    }

    pub fn missing_range(&self, offset: u64) -> Range<u64> {
        let end = self
            .blocks
            .range(offset + 1..)
            .next()
            .map_or(self.len, |(&start, _)| start);
        offset..end
    }

    /// The number of bytes of the file not fetched yet.
    pub fn missing_bytes(&self) -> u64 {
        let mut missing = 0;
        let mut at = 0;
        while at < self.len {
            if let Some(block) = self.block_at(at) {
                at += block.len() as u64;
            } else {
                let range = self.missing_range(at);
                missing += range.end - range.start;
                at = range.end;
            }
        }
        missing
    }
}

/// Reads a [`Fetched`] file. A read of bytes that are not there fails, and the
/// offset is left in [`SparseReader::miss`].
pub(crate) struct SparseReader {
    fetched: Rc<RefCell<Fetched>>,
    pos: u64,
    miss: Rc<Cell<Option<u64>>>,
}

impl SparseReader {
    pub fn new(fetched: Rc<RefCell<Fetched>>) -> Self {
        Self {
            fetched,
            pos: 0,
            miss: Rc::default(),
        }
    }

    /// Where the first read of missing bytes happened. The readers stacked
    /// on top wrap or swallow the error, so it is recorded here instead.
    pub fn miss(&self) -> Rc<Cell<Option<u64>>> {
        self.miss.clone()
    }
}

impl Read for SparseReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let fetched = self.fetched.borrow();
        if self.pos >= fetched.len || buf.is_empty() {
            return Ok(0);
        }
        let Some(block) = fetched.block_at(self.pos) else {
            if self.miss.get().is_none() {
                self.miss.set(Some(self.pos));
            }
            return Err(std::io::Error::other(format!(
                "byte {} has not been fetched yet",
                self.pos
            )));
        };
        let n = buf.len().min(block.len());
        buf[..n].copy_from_slice(&block[..n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SparseReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let len = self.fetched.borrow().len;
        let pos = match pos {
            SeekFrom::Start(pos) => Some(pos),
            SeekFrom::End(delta) => len.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
        };
        self.pos = pos.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek to a negative position",
            )
        })?;
        Ok(self.pos)
    }
}
