//! Read a package archive through byte ranges, without a filesystem or
//! threads.
//!
//! [`crate::archive::PackageArchive`] streams archives with tokio and spools
//! whole downloads to disk, neither of which exists on `wasm32`. This module
//! reads the same archives through a [`RangeSource`]: anything that can report
//! the archive's size and hand back a byte range, such as an HTTP server
//! answering `Range` requests, or a buffer in memory.
//!
//! A `.conda` archive is a ZIP of stored (uncompressed) members: the
//! `info-*.tar.zst` metadata section and the `pkg-*.tar.zst` payload. Opening
//! one fetches the archive's tail, which holds the ZIP central directory and,
//! as the info section is written last, usually that whole section too. The
//! payload is fetched with one more range request the first time it is read.
//! A `.tar.bz2` archive is a single stream and is fetched whole on first use.
//!
//! Decompression and tar parsing run synchronously over bytes that have
//! already been fetched, so the futures here need not be `Send` and the
//! source may be backed by a browser's `fetch`.
//!
//! ```rust,no_run
//! # use rattler_package_streaming::range::{RangeArchive, Section};
//! # use rattler_conda_types::package::{CondaArchiveType, IndexJson};
//! # async fn example() -> Result<(), rattler_package_streaming::ExtractError> {
//! let bytes = bytes::Bytes::from(std::fs::read("pkg.conda")?);
//! let archive = RangeArchive::open(bytes, CondaArchiveType::Conda).await?;
//! let index: IndexJson = archive.read_package_file().await?;
//! for entry in archive.list_entries(Section::Content).await? {
//!     println!("{} ({} bytes)", entry.path.display(), entry.size);
//! }
//! # Ok(())
//! # }
//! ```

use std::{
    collections::HashMap,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Mutex,
};

use bytes::Bytes;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};

use crate::ExtractError;

/// Bytes fetched from the end of a remote archive on open: enough for the
/// ZIP central directory, with the surplus acting as a cache that often
/// contains the entire info section.
pub(crate) const TAIL_SIZE: u64 = 64 * 1024;

/// Signature of a ZIP local file header (`PK\x03\x04`).
pub(crate) const LOCAL_HEADER_MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

/// Cap for upfront buffer allocations based on (untrusted) tar header sizes.
const MAX_PREALLOC: u64 = 4 * 1024 * 1024;

/// The two sections of a conda package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    /// Package metadata: everything under `info/`. Stored in the
    /// `info-*.tar.zst` member of a `.conda` archive.
    Info,
    /// The package payload. Stored in the `pkg-*.tar.zst` member of a
    /// `.conda` archive.
    Content,
}

impl Section {
    /// Returns the section a path inside the package belongs to.
    pub(crate) fn containing(path: &Path) -> Section {
        let first = path
            .components()
            .find(|c| !matches!(c, std::path::Component::CurDir));
        match first {
            Some(std::path::Component::Normal(first)) if first == "info" => Section::Info,
            _ => Section::Content,
        }
    }

    /// The file name prefix of the ZIP member holding this section.
    pub(crate) fn zip_prefix(self) -> &'static str {
        match self {
            Section::Info => "info-",
            Section::Content => "pkg-",
        }
    }
}

/// The kind of an entry in a package archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArchiveEntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// A hard link.
    Hardlink,
    /// Another tar entry type.
    Other,
}

impl ArchiveEntryKind {
    /// Returns whether this entry is a symbolic or hard link.
    pub fn is_link(self) -> bool {
        matches!(self, Self::Symlink | Self::Hardlink)
    }

    fn of(entry_type: tar::EntryType) -> Self {
        if entry_type.is_file() {
            Self::File
        } else if entry_type.is_dir() {
            Self::Directory
        } else if entry_type.is_symlink() {
            Self::Symlink
        } else if entry_type.is_hard_link() {
            Self::Hardlink
        } else {
            Self::Other
        }
    }
}

/// A tar entry of a package section, as listed by [`RangeArchive::list_entries`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// The normalized package-relative path of the entry.
    pub path: PathBuf,
    /// The declared size of the entry in bytes. Zero for links and directories.
    pub size: u64,
    /// What the entry is.
    pub kind: ArchiveEntryKind,
    /// The target of a symbolic or hard link.
    pub link_target: Option<PathBuf>,
}

/// Something a package archive can be read from in pieces.
///
/// Implementations do not need `Send` futures: the archive reader awaits them
/// on the thread it runs on, which is how a browser's `fetch` has to be used.
#[allow(async_fn_in_trait)]
pub trait RangeSource {
    /// The total size of the archive in bytes.
    async fn len(&self) -> Result<u64, ExtractError>;

    /// The bytes in `start..end` of the archive. `end` is exclusive and never
    /// exceeds [`RangeSource::len`]; the returned buffer must be exactly
    /// `end - start` bytes long.
    async fn read_range(&self, start: u64, end: u64) -> Result<Bytes, ExtractError>;
}

/// An archive held in memory.
impl RangeSource for Bytes {
    async fn len(&self) -> Result<u64, ExtractError> {
        Ok(Bytes::len(self) as u64)
    }

    async fn read_range(&self, start: u64, end: u64) -> Result<Bytes, ExtractError> {
        let end = usize::try_from(end).map_err(|_| short_read(start, end))?;
        let start = usize::try_from(start).map_err(|_| short_read(start as u64, end as u64))?;
        if start > end || end > Bytes::len(self) {
            return Err(short_read(start as u64, end as u64));
        }
        Ok(self.slice(start..end))
    }
}

fn short_read(start: u64, end: u64) -> ExtractError {
    ExtractError::IoError(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("the archive does not contain the bytes {start}..{end}"),
    ))
}

fn invalid_data(message: impl Into<String>) -> ExtractError {
    ExtractError::IoError(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    ))
}

/// Byte span of a stored ZIP member inside a `.conda` archive.
#[derive(Debug, Clone)]
struct Member {
    name: String,
    /// Offset of the member's local file header.
    header_offset: u64,
    /// Size of the stored (uncompressed) member data.
    size: u64,
    /// Exclusive upper bound of the member's bytes in the archive: the next
    /// member's local header, or the end of the archive for the last one.
    end: u64,
}

enum Backend {
    Conda {
        members: Vec<Member>,
        /// Offset of the first byte of `tail` in the archive.
        tail_offset: u64,
        /// The end of the archive, fetched on open.
        tail: Bytes,
        /// The stored bytes of the members read so far.
        member_cache: Mutex<HashMap<Section, Bytes>>,
    },
    TarBz2 {
        /// The whole archive, fetched on first use.
        archive: Mutex<Option<Bytes>>,
    },
}

/// A conda package archive read through a [`RangeSource`], opened once and
/// read many times.
pub struct RangeArchive<S> {
    source: S,
    size: u64,
    archive_type: CondaArchiveType,
    backend: Backend,
}

impl<S: RangeSource> RangeArchive<S> {
    /// Opens the archive. For a `.conda` archive this fetches its tail and
    /// parses the ZIP central directory; a `.tar.bz2` archive is not fetched
    /// until something is read from it.
    pub async fn open(source: S, archive_type: CondaArchiveType) -> Result<Self, ExtractError> {
        let size = source.len().await?;
        let backend = match archive_type {
            CondaArchiveType::Conda => Self::open_conda(&source, size).await?,
            CondaArchiveType::TarBz2 => Backend::TarBz2 {
                archive: Mutex::new(None),
            },
        };
        Ok(Self {
            source,
            size,
            archive_type,
            backend,
        })
    }

    async fn open_conda(source: &S, size: u64) -> Result<Backend, ExtractError> {
        let mut tail_offset = size.saturating_sub(TAIL_SIZE);
        let mut tail = source.read_range(tail_offset, size).await?;
        let members = match parse_members(&tail, tail_offset, size) {
            Ok(members) => members,
            // The central directory starts before the tail, which happens
            // with an unusually long archive comment. Such an archive is
            // read whole; conda archives have only a handful of members, so
            // a central directory that large is not worth a third request.
            Err(ExtractError::IoError(err))
                if err.get_ref().is_some_and(|e| e.is::<MissingRange>()) =>
            {
                tail_offset = 0;
                tail = source.read_range(0, size).await?;
                parse_members(&tail, 0, size)?
            }
            Err(err) => return Err(err),
        };
        Ok(Backend::Conda {
            members,
            tail_offset,
            tail,
            member_cache: Mutex::new(HashMap::new()),
        })
    }

    /// The size of the whole archive in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The format of the archive.
    pub fn archive_type(&self) -> CondaArchiveType {
        self.archive_type
    }

    /// The number of bytes that have to be fetched to read `section`, or
    /// `None` when the archive has no such section.
    ///
    /// For a `.conda` archive this is the stored size of the section's
    /// member; the info section is often already present in the tail fetched
    /// on open. A `.tar.bz2` archive has to be fetched whole for either
    /// section.
    pub fn section_size(&self, section: Section) -> Option<u64> {
        match &self.backend {
            Backend::Conda { members, .. } => find_member(members, section).map(|m| m.size),
            Backend::TarBz2 { .. } => Some(self.size),
        }
    }

    /// Lists the tar entries of `section` in archive order.
    pub async fn list_entries(&self, section: Section) -> Result<Vec<ArchiveEntry>, ExtractError> {
        let mut archive = self.section_tar(section).await?;
        let mut entries = Vec::new();
        for entry in archive.entries()? {
            let entry = entry?;
            let path = normalize(&entry.path()?)?.into_owned();
            if self.archive_type == CondaArchiveType::TarBz2
                && Section::containing(&path) != section
            {
                continue;
            }
            entries.push(ArchiveEntry {
                path,
                size: entry.header().size()?,
                kind: ArchiveEntryKind::of(entry.header().entry_type()),
                link_target: entry.link_name()?.map(std::borrow::Cow::into_owned),
            });
        }
        Ok(entries)
    }

    /// Reads the contents of one file, or `None` when the archive has no such
    /// file. Requesting a symbolic or hard link is an error; links are not
    /// followed.
    pub async fn read_file(&self, path: impl AsRef<Path>) -> Result<Option<Vec<u8>>, ExtractError> {
        let path = normalize(path.as_ref())?.into_owned();
        let mut archive = self.section_tar(Section::containing(&path)).await?;
        for entry in archive.entries()? {
            let mut entry = entry?;
            if normalize(&entry.path()?)? != path {
                continue;
            }
            let kind = ArchiveEntryKind::of(entry.header().entry_type());
            if kind.is_link() {
                let target = entry
                    .link_name()?
                    .map(|target| target.display().to_string())
                    .unwrap_or_default();
                return Err(ExtractError::LinksNotFollowed(vec![format!(
                    "'{}' (links to '{target}')",
                    path.display()
                )]));
            }
            let size = entry.header().size()?;
            let mut buf = Vec::with_capacity(size.min(MAX_PREALLOC) as usize);
            entry.read_to_end(&mut buf)?;
            return Ok(Some(buf));
        }
        Ok(None)
    }

    /// Reads and parses a typed [`PackageFile`] such as
    /// [`rattler_conda_types::package::IndexJson`], or `None` when the archive
    /// does not contain it.
    pub async fn try_read_package_file<P: PackageFile>(&self) -> Result<Option<P>, ExtractError> {
        match self.read_file(P::package_path()).await? {
            Some(bytes) => P::from_slice(&bytes).map(Some).map_err(|e| {
                ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e)
            }),
            None => Ok(None),
        }
    }

    /// Reads and parses a typed [`PackageFile`], failing when the archive
    /// does not contain it.
    pub async fn read_package_file<P: PackageFile>(&self) -> Result<P, ExtractError> {
        self.try_read_package_file()
            .await?
            .ok_or(ExtractError::MissingComponent)
    }

    /// A tar reader over `section`. For a `.tar.bz2` archive this is the whole
    /// archive, and callers filter by path.
    async fn section_tar(
        &self,
        section: Section,
    ) -> Result<tar::Archive<Box<dyn Read>>, ExtractError> {
        let reader: Box<dyn Read> = match &self.backend {
            Backend::Conda { .. } => {
                let member = self.member_bytes(section).await?;
                Box::new(zstd::stream::read::Decoder::new(Cursor::new(member))?)
            }
            Backend::TarBz2 { .. } => {
                let archive = self.whole_archive().await?;
                Box::new(bzip2::read::BzDecoder::new(Cursor::new(archive)))
            }
        };
        Ok(tar::Archive::new(reader))
    }

    /// The stored bytes of the ZIP member holding `section`, from the tail
    /// when it is in there, otherwise with one range request, cached either
    /// way.
    async fn member_bytes(&self, section: Section) -> Result<Bytes, ExtractError> {
        let Backend::Conda {
            members,
            tail_offset,
            tail,
            member_cache,
        } = &self.backend
        else {
            unreachable!("member_bytes is only called for .conda archives");
        };
        if let Some(bytes) = lock(member_cache).get(&section) {
            return Ok(bytes.clone());
        }
        let member = find_member(members, section).ok_or(ExtractError::MissingComponent)?;

        let bytes = if member.header_offset >= *tail_offset {
            let rel = (member.header_offset - tail_offset) as usize;
            match member_data_range(&tail[rel..], member.size) {
                Some(range) => tail.slice(rel + range.start..rel + range.end),
                None => self.fetch_member(member).await?,
            }
        } else {
            self.fetch_member(member).await?
        };
        lock(member_cache).insert(section, bytes.clone());
        Ok(bytes)
    }

    async fn fetch_member(&self, member: &Member) -> Result<Bytes, ExtractError> {
        let bytes = self
            .source
            .read_range(member.header_offset, member.end)
            .await?;
        if bytes.len() as u64 != member.end - member.header_offset {
            return Err(short_read(member.header_offset, member.end));
        }
        let range = member_data_range(&bytes, member.size).ok_or_else(|| {
            invalid_data(format!(
                "member {} does not start with a ZIP local file header",
                member.name
            ))
        })?;
        Ok(bytes.slice(range))
    }

    async fn whole_archive(&self) -> Result<Bytes, ExtractError> {
        let Backend::TarBz2 { archive } = &self.backend else {
            unreachable!("whole_archive is only called for .tar.bz2 archives");
        };
        if let Some(bytes) = lock(archive).as_ref() {
            return Ok(bytes.clone());
        }
        let bytes = self.source.read_range(0, self.size).await?;
        if bytes.len() as u64 != self.size {
            return Err(short_read(0, self.size));
        }
        *lock(archive) = Some(bytes.clone());
        Ok(bytes)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // The caches hold plain bytes, so a panic while holding the lock cannot
    // leave them inconsistent.
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn find_member(members: &[Member], section: Section) -> Option<&Member> {
    let prefix = section.zip_prefix();
    members
        .iter()
        .find(|m| m.name.starts_with(prefix) && m.name.ends_with(".tar.zst"))
}

/// The ZIP central directory lies before the fetched tail of the archive, so
/// more of the archive has to be fetched before it can be parsed.
#[derive(Debug)]
struct MissingRange;

impl std::fmt::Display for MissingRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the ZIP central directory lies before the fetched tail of the archive")
    }
}

impl std::error::Error for MissingRange {}

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const EOCD64_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const EOCD64_SIGNATURE: u32 = 0x0606_4b50;
const CENTRAL_HEADER_SIGNATURE: u32 = 0x0201_4b50;
const ZIP64_EXTRA_FIELD: u16 = 0x0001;
/// A stored member: the only compression method a `.conda` archive may use.
const COMPRESSION_STORED: u16 = 0;

/// Little-endian reads with bounds checks over the tail of the archive.
struct Fields<'a>(&'a [u8]);

impl Fields<'_> {
    fn bytes(&self, at: usize, len: usize) -> Result<&[u8], ExtractError> {
        self.0
            .get(at..at.checked_add(len).ok_or_else(truncated)?)
            .ok_or_else(truncated)
    }

    fn u16(&self, at: usize) -> Result<u16, ExtractError> {
        Ok(u16::from_le_bytes(self.bytes(at, 2)?.try_into().unwrap()))
    }

    fn u32(&self, at: usize) -> Result<u32, ExtractError> {
        Ok(u32::from_le_bytes(self.bytes(at, 4)?.try_into().unwrap()))
    }

    fn u64(&self, at: usize) -> Result<u64, ExtractError> {
        Ok(u64::from_le_bytes(self.bytes(at, 8)?.try_into().unwrap()))
    }
}

fn truncated() -> ExtractError {
    invalid_data("the ZIP central directory is truncated")
}

/// Parses the ZIP central directory out of the archive's tail into member
/// spans. The exclusive end bound of each member is the offset of the next
/// member (or the end of the archive), which over-approximates by at most the
/// size of the central directory for the last member.
///
/// Only the tail is available, so this is a purpose-built parser rather than
/// the `zip` crate, which validates the first local file header at the start
/// of the archive when it opens one. Fails with [`MissingRange`] when the
/// central directory starts before the tail.
fn parse_members(tail: &[u8], tail_offset: u64, size: u64) -> Result<Vec<Member>, ExtractError> {
    let fields = Fields(tail);
    // Position in the tail of a byte at `offset` in the archive.
    let local = |offset: u64| -> Result<usize, ExtractError> {
        offset
            .checked_sub(tail_offset)
            .map(|rel| rel as usize)
            .ok_or_else(|| ExtractError::IoError(std::io::Error::other(MissingRange)))
    };

    // The end of central directory record is the last structure in the
    // archive, followed only by its comment of at most 64 KiB.
    let eocd = (0..=tail.len().saturating_sub(22))
        .rev()
        .find(|&at| fields.u32(at).ok() == Some(EOCD_SIGNATURE))
        .ok_or_else(|| invalid_data("the archive has no ZIP end of central directory record"))?;
    let mut entry_count = u64::from(fields.u16(eocd + 10)?);
    let mut directory_offset = u64::from(fields.u32(eocd + 16)?);

    // Values that do not fit the record are deferred to the zip64 record,
    // which the locator right before the end of central directory names.
    if entry_count == u64::from(u16::MAX) || directory_offset == u64::from(u32::MAX) {
        let locator = eocd
            .checked_sub(20)
            .filter(|&at| fields.u32(at).ok() == Some(EOCD64_LOCATOR_SIGNATURE))
            .ok_or_else(|| {
                invalid_data("the archive has no zip64 end of central directory locator")
            })?;
        let eocd64 = local(fields.u64(locator + 8)?)?;
        if fields.u32(eocd64)? != EOCD64_SIGNATURE {
            return Err(invalid_data(
                "invalid zip64 end of central directory record",
            ));
        }
        entry_count = fields.u64(eocd64 + 32)?;
        directory_offset = fields.u64(eocd64 + 48)?;
    }

    let mut at = local(directory_offset)?;
    let mut members = Vec::with_capacity(entry_count.min(16) as usize);
    for _ in 0..entry_count {
        if fields.u32(at)? != CENTRAL_HEADER_SIGNATURE {
            return Err(invalid_data("invalid ZIP central directory file header"));
        }
        let compression = fields.u16(at + 10)?;
        let mut compressed_size = u64::from(fields.u32(at + 20)?);
        let uncompressed_size = fields.u32(at + 24)?;
        let name_len = usize::from(fields.u16(at + 28)?);
        let extra_len = usize::from(fields.u16(at + 30)?);
        let comment_len = usize::from(fields.u16(at + 32)?);
        let mut header_offset = u64::from(fields.u32(at + 42)?);
        let name = String::from_utf8_lossy(fields.bytes(at + 46, name_len)?).into_owned();

        // The zip64 extra field carries, in this order, only the fields the
        // header could not hold.
        let extra = Fields(fields.bytes(at + 46 + name_len, extra_len)?);
        let mut extra_at = 0;
        while extra_at + 4 <= extra.0.len() {
            let id = extra.u16(extra_at)?;
            let len = usize::from(extra.u16(extra_at + 2)?);
            if id == ZIP64_EXTRA_FIELD {
                let zip64 = Fields(extra.bytes(extra_at + 4, len)?);
                let mut field = 0;
                if uncompressed_size == u32::MAX {
                    field += 8;
                }
                if compressed_size == u64::from(u32::MAX) {
                    compressed_size = zip64.u64(field)?;
                    field += 8;
                }
                if header_offset == u64::from(u32::MAX) {
                    header_offset = zip64.u64(field)?;
                }
                break;
            }
            extra_at += 4 + len;
        }

        if name.ends_with(".tar.zst") && compression != COMPRESSION_STORED {
            return Err(ExtractError::UnsupportedCompressionMethod);
        }
        members.push(Member {
            name,
            header_offset,
            size: compressed_size,
            end: size,
        });
        at += 46 + name_len + extra_len + comment_len;
    }

    members.sort_unstable_by_key(|m| m.header_offset);
    for i in 1..members.len() {
        members[i - 1].end = members[i].header_offset;
    }
    Ok(members)
}

/// Parses a ZIP local file header at the start of `buf` and returns the
/// range of the member data if `buf` contains all of it.
pub(crate) fn member_data_range(buf: &[u8], size: u64) -> Option<std::ops::Range<usize>> {
    if buf.len() < 30 || buf[0..4] != LOCAL_HEADER_MAGIC {
        return None;
    }
    let name_len = u16::from_le_bytes([buf[26], buf[27]]) as usize;
    let extra_len = u16::from_le_bytes([buf[28], buf[29]]) as usize;
    let data_start = 30 + name_len + extra_len;
    let data_end = data_start.checked_add(usize::try_from(size).ok()?)?;
    (data_end <= buf.len()).then_some(data_start..data_end)
}

/// Validates a package-relative path and strips `.` components.
///
/// Package paths may not be empty, absolute, or contain parent components.
pub(crate) fn normalize(path: &Path) -> Result<std::borrow::Cow<'_, Path>, ExtractError> {
    let mut needs_normalization = false;
    let mut has_component = false;
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) => has_component = true,
            std::path::Component::CurDir => needs_normalization = true,
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(ExtractError::InvalidArchivePath(path.to_owned()));
            }
        }
    }
    if !has_component {
        return Err(ExtractError::InvalidArchivePath(path.to_owned()));
    }
    if needs_normalization {
        Ok(std::borrow::Cow::Owned(
            path.components()
                .filter(|component| !matches!(component, std::path::Component::CurDir))
                .collect(),
        ))
    } else {
        Ok(std::borrow::Cow::Borrowed(path))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rattler_conda_types::package::{AboutJson, IndexJson, PathsJson};

    use super::*;

    /// Counts the ranges a reader asks for, and checks they are in bounds.
    struct CountingSource {
        bytes: Bytes,
        requests: AtomicUsize,
        fetched: AtomicUsize,
    }

    impl CountingSource {
        fn new(path: &str) -> Self {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
            Self {
                bytes: std::fs::read(path).unwrap().into(),
                requests: AtomicUsize::new(0),
                fetched: AtomicUsize::new(0),
            }
        }

        fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }
    }

    impl RangeSource for CountingSource {
        async fn len(&self) -> Result<u64, ExtractError> {
            Ok(self.bytes.len() as u64)
        }

        async fn read_range(&self, start: u64, end: u64) -> Result<Bytes, ExtractError> {
            assert!(
                start <= end && end <= self.bytes.len() as u64,
                "{start}..{end}"
            );
            self.requests.fetch_add(1, Ordering::SeqCst);
            self.fetched
                .fetch_add((end - start) as usize, Ordering::SeqCst);
            self.bytes.read_range(start, end).await
        }
    }

    async fn open(path: &str) -> RangeArchive<CountingSource> {
        let archive_type = CondaArchiveType::try_from(path).unwrap();
        RangeArchive::open(CountingSource::new(path), archive_type)
            .await
            .unwrap()
    }

    fn paths(entries: &[ArchiveEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.path.to_str().unwrap()).collect()
    }

    #[tokio::test]
    async fn info_is_served_from_the_tail() {
        let archive = open("../../test-data/sparse/sparse-test-1.0.0-0.conda").await;
        assert_eq!(
            archive.source.requests(),
            1,
            "opening fetches only the tail"
        );
        assert_eq!(archive.archive_type(), CondaArchiveType::Conda);
        assert_eq!(archive.size(), archive.source.bytes.len() as u64);

        let index: IndexJson = archive.read_package_file().await.unwrap();
        assert_eq!(index.name.as_normalized(), "sparse-test");
        let paths_json: PathsJson = archive.read_package_file().await.unwrap();
        assert_eq!(paths_json.paths.len(), 3);
        assert_eq!(
            paths(&archive.list_entries(Section::Info).await.unwrap()),
            ["info/index.json", "info/paths.json"]
        );
        assert!(
            archive
                .try_read_package_file::<AboutJson>()
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            archive.source.requests(),
            1,
            "the info section was in the tail"
        );
    }

    #[tokio::test]
    async fn payload_costs_one_more_request() {
        let archive = open("../../test-data/sparse/sparse-test-1.0.0-0.conda").await;
        let entries = archive.list_entries(Section::Content).await.unwrap();
        assert_eq!(
            paths(&entries),
            ["bin/first-file.txt", "lib/blob.bin", "share/last-file.txt"]
        );
        assert_eq!(entries[1].size, 150_000);
        assert_eq!(entries[1].kind, ArchiveEntryKind::File);
        assert_eq!(archive.source.requests(), 2);

        let first = archive
            .read_file("bin/first-file.txt")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first, b"first payload file\n");
        let last = archive
            .read_file("./share/last-file.txt")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(last, b"last payload file\n");
        assert!(
            archive
                .read_file("share/missing.txt")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(archive.source.requests(), 2, "the payload member is cached");
        assert!(
            archive.section_size(Section::Content).unwrap() > TAIL_SIZE,
            "the fixture payload does not fit in the tail"
        );
        assert!(archive.section_size(Section::Info).unwrap() < TAIL_SIZE);
    }

    #[tokio::test]
    async fn links_are_listed_but_not_followed() {
        let archive = open("../../test-data/sparse/symlink-test-1.0.0-0.conda").await;
        let entries = archive.list_entries(Section::Content).await.unwrap();
        let link = entries
            .iter()
            .find(|e| e.path.ends_with("liblink.so"))
            .unwrap();
        assert_eq!(link.kind, ArchiveEntryKind::Symlink);
        assert_eq!(link.link_target.as_deref(), Some(Path::new("libreal.so.1")));
        let hard = entries
            .iter()
            .find(|e| e.path.ends_with("libhard.so"))
            .unwrap();
        assert_eq!(hard.kind, ArchiveEntryKind::Hardlink);
        assert!(matches!(
            archive.read_file("lib/liblink.so").await,
            Err(ExtractError::LinksNotFollowed(_))
        ));
        assert_eq!(
            archive
                .read_file("lib/libreal.so.1")
                .await
                .unwrap()
                .unwrap(),
            b"real library bytes"
        );
    }

    #[tokio::test]
    async fn archive_without_payload() {
        let archive = open("../../test-data/sparse/info-only-1.0.0-0.conda").await;
        assert_eq!(archive.section_size(Section::Content), None);
        assert!(matches!(
            archive.list_entries(Section::Content).await,
            Err(ExtractError::MissingComponent)
        ));
        let index: IndexJson = archive.read_package_file().await.unwrap();
        assert_eq!(index.name.as_normalized(), "info-only");
    }

    #[tokio::test]
    async fn zip64_members() {
        let archive = open("../../test-data/sparse/zip64-test-1.0.0-0.conda").await;
        assert_eq!(
            archive.read_file("bin/hello.txt").await.unwrap().unwrap(),
            b"zip64 payload\n"
        );
    }

    #[tokio::test]
    async fn tar_bz2_is_fetched_whole_once() {
        let archive =
            open("../../test-data/test-server/repo/noarch/test-package-0.1-0.tar.bz2").await;
        assert_eq!(archive.archive_type(), CondaArchiveType::TarBz2);
        assert_eq!(archive.source.requests(), 0, "nothing is fetched on open");
        assert_eq!(archive.section_size(Section::Info), Some(archive.size()));

        let index: IndexJson = archive.read_package_file().await.unwrap();
        assert_eq!(index.name.as_normalized(), "test-package");
        let info = archive.list_entries(Section::Info).await.unwrap();
        assert!(paths(&info).contains(&"info/recipe/meta.yaml"), "{info:?}");
        assert!(paths(&info).iter().all(|p| p.starts_with("info/")));
        let content = archive.list_entries(Section::Content).await.unwrap();
        assert!(paths(&content).iter().all(|p| !p.starts_with("info/")));
        assert_eq!(archive.source.requests(), 1);
    }

    #[tokio::test]
    async fn rejects_paths_outside_the_package() {
        let archive = open("../../test-data/sparse/sparse-test-1.0.0-0.conda").await;
        for path in [
            "",
            "/etc/passwd",
            "../index.json",
            "info/../bin/first-file.txt",
        ] {
            assert!(
                matches!(
                    archive.read_file(path).await,
                    Err(ExtractError::InvalidArchivePath(_))
                ),
                "{path:?}"
            );
        }
    }

    #[test]
    fn section_of_a_path() {
        assert_eq!(
            Section::containing(Path::new("info/index.json")),
            Section::Info
        );
        assert_eq!(
            Section::containing(Path::new("./info/index.json")),
            Section::Info
        );
        assert_eq!(
            Section::containing(Path::new("info-custom.txt")),
            Section::Content
        );
        assert_eq!(Section::containing(Path::new("bin/info")), Section::Content);
    }
}
