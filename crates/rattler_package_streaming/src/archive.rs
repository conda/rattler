//! Read individual files from conda packages, local or remote, with as few
//! HTTP requests as possible.
//!
//! A [`PackageArchive`] is opened once and queried many times. Remote
//! `.conda` archives on range-capable servers are opened with a single
//! request for the archive tail (ZIP central directory and usually the whole
//! info section); reads then cost at most one streaming ranged request per
//! touched section, aborted once the last requested file has been read.
//! `.tar.bz2` archives and servers without range support transparently fall
//! back to downloading the archive once into a temporary spool file.
//!
//! Reads are not retried internally: a network error mid-read surfaces as an
//! [`ExtractError`] (check [`ExtractError::should_retry`]) and the call can
//! simply be repeated. Symbolic links inside the archive are surfaced but
//! never followed; reading one is an error.
//!
//! # Example
//!
//! ```rust,no_run
//! # #[tokio::main]
//! # async fn main() {
//! use rattler_conda_types::package::PathsJson;
//! use rattler_package_streaming::archive::PackageArchive;
//! use reqwest::Client;
//! use reqwest_middleware::ClientWithMiddleware;
//! use url::Url;
//!
//! let client = ClientWithMiddleware::from(Client::new());
//! let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.12.7-hc5c86c4_0_cpython.conda").unwrap();
//!
//! // One HTTP range request.
//! let archive = PackageArchive::from_url(client, url).await.unwrap();
//!
//! // Usually free: the info section often sits inside the cached tail.
//! let paths: PathsJson = archive.read_package_file().await.unwrap();
//!
//! // One streaming pass over the payload, aborted after the last hit.
//! let files = archive
//!     .read_files(paths.paths.iter().map(|entry| entry.relative_path.clone()))
//!     .await
//!     .unwrap();
//! # drop(files);
//! # }
//! ```

use std::collections::{HashMap, HashSet};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_compression::tokio::bufread::{BzDecoder, ZstdDecoder};
use async_http_range_reader::{
    AsyncHttpRangeReader, AsyncHttpRangeReaderError, CheckSupportMethod,
};
use async_zip::Compression;
use async_zip::base::read::seek::ZipFileReader;
use futures_util::TryStreamExt;
use http::HeaderMap;
use http::header::{ETAG, IF_RANGE, LAST_MODIFIED, RANGE};
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use reqwest_middleware::ClientWithMiddleware;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tokio_util::io::StreamReader;
use tracing::debug;
use url::Url;

use crate::ExtractError;

/// Bytes fetched from the end of a remote archive on open: enough for the
/// ZIP central directory, with the surplus acting as a cache that often
/// contains the entire info section.
const TAIL_SIZE: u64 = 64 * 1024;

/// Buffer size used for the decompression pipelines.
const STREAM_BUF_SIZE: usize = 128 * 1024;

/// Signature of a ZIP local file header (`PK\x03\x04`).
const LOCAL_HEADER_MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

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

/// How a remote archive should be opened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SparsePolicy {
    /// Prefer sparse access and fall back to a spooled download.
    #[default]
    Prefer,
    /// Require sparse access and fail when the server does not support it.
    Require,
    /// Skip the range probe and download the archive immediately.
    Disable,
}

/// Options for opening a remote package archive.
#[derive(Debug, Clone, Default)]
pub struct RemoteArchiveOptions {
    sparse_policy: SparsePolicy,
    max_spool_size: Option<u64>,
}

impl RemoteArchiveOptions {
    /// Creates options using the default sparse-access policy.
    pub const fn new() -> Self {
        Self {
            sparse_policy: SparsePolicy::Prefer,
            max_spool_size: None,
        }
    }

    /// Sets how HTTP range support and full-download fallback are handled.
    pub const fn with_sparse_policy(mut self, policy: SparsePolicy) -> Self {
        self.sparse_policy = policy;
        self
    }

    /// Limits the number of bytes that may be downloaded for a spooled
    /// fallback. By default there is no limit.
    pub const fn with_max_spool_size(mut self, max_size: u64) -> Self {
        self.max_spool_size = Some(max_size);
        self
    }
}

/// How a [`PackageArchive`] accesses the underlying archive.
///
/// This is diagnostic information. Use [`RemoteArchiveOptions`] to control
/// whether a remote archive may fall back to a spooled download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArchiveAccess {
    /// Remote archive read sparsely with HTTP range requests.
    Sparse,
    /// Local file on disk.
    Local,
    /// Remote archive that was downloaded once into a temporary spool file
    /// (server without range support, or a `.tar.bz2` archive).
    Spooled,
}

/// Byte span of a stored ZIP member inside a `.conda` archive.
///
/// `end` is the offset of the next member's local header (or the central
/// directory for the last member), which is a robust upper bound for the
/// member's data regardless of local-header extra field quirks.
#[derive(Debug, Clone)]
struct MemberSpan {
    name: String,
    /// Offset of the member's local file header.
    header_offset: u64,
    /// Size of the stored (uncompressed) member data.
    size: u64,
    /// Exclusive upper bound of the member's bytes in the archive.
    end: u64,
}

enum Backend {
    Conda {
        source: CondaSource,
        members: Vec<MemberSpan>,
    },
    TarBz2 {
        path: PathBuf,
        temp: Option<tempfile::TempPath>,
    },
}

/// Where the bytes of a `.conda` archive come from.
enum CondaSource {
    Sparse {
        client: ClientWithMiddleware,
        url: Url,
        /// Strong `ETag` (or `Last-Modified`) captured at open time; sent as
        /// `If-Range` on section requests so servers that honor it reject
        /// reads from a concurrently republished archive. Best-effort:
        /// servers may ignore `If-Range`.
        validator: Option<http::HeaderValue>,
        tail_offset: u64,
        tail: bytes::Bytes,
    },
    Local {
        path: PathBuf,
        /// Present when the archive was spooled from a remote; keeps the
        /// temporary file alive and distinguishes `Spooled` from `Local`.
        temp: Option<tempfile::TempPath>,
    },
}

/// A conda package archive that can be opened once and read many times.
///
/// Cloning is cheap; clones share the parsed archive index and (for spooled
/// archives) the temporary file.
#[derive(Clone)]
pub struct PackageArchive {
    backend: Arc<Backend>,
}

/// A boxed reader used for the section decompression pipelines.
type DynReader = Box<dyn AsyncRead + Send + Unpin>;
type RawSectionEntry = tokio_tar::Entry<tokio_tar::Archive<DynReader>>;

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
}

/// An entry yielded by [`SectionStream::next_entry`].
///
/// The underlying tar implementation is intentionally hidden so it can be
/// changed without affecting callers.
pub struct SectionEntry {
    inner: RawSectionEntry,
    path: PathBuf,
}

impl SectionEntry {
    /// Returns the normalized package-relative path of this entry.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the entry kind.
    pub fn kind(&self) -> ArchiveEntryKind {
        let kind = self.inner.header().entry_type();
        if kind.is_file() {
            ArchiveEntryKind::File
        } else if kind.is_dir() {
            ArchiveEntryKind::Directory
        } else if kind.is_symlink() {
            ArchiveEntryKind::Symlink
        } else if kind.is_hard_link() {
            ArchiveEntryKind::Hardlink
        } else {
            ArchiveEntryKind::Other
        }
    }

    /// Returns the declared entry size.
    pub fn size(&self) -> Result<u64, ExtractError> {
        Ok(self.inner.header().size()?)
    }

    /// Returns the raw link target for a symbolic or hard link.
    pub fn link_target(&self) -> Result<Option<PathBuf>, ExtractError> {
        Ok(self.inner.link_name()?.map(|path| path.into_owned()))
    }

    /// Reads the complete entry body.
    ///
    /// Links are surfaced by the iterator but are never followed.
    pub async fn read(&mut self) -> Result<Vec<u8>, ExtractError> {
        if let Some(link) = describe_link(self)? {
            return Err(ExtractError::LinksNotFollowed(vec![link]));
        }
        read_raw_entry_contents(&mut self.inner).await
    }
}

impl PackageArchive {
    /// Opens a remote package archive with a single range request, falling
    /// back to a one-time spooled download for `.tar.bz2` archives and
    /// servers without range support.
    pub async fn from_url(client: ClientWithMiddleware, url: Url) -> Result<Self, ExtractError> {
        Self::from_url_with_options(client, url, RemoteArchiveOptions::default()).await
    }

    /// Opens a remote package archive using the supplied access policy.
    pub async fn from_url_with_options(
        client: ClientWithMiddleware,
        url: Url,
        options: RemoteArchiveOptions,
    ) -> Result<Self, ExtractError> {
        let archive_type = CondaArchiveType::try_from(Path::new(url.path()))
            .ok_or(ExtractError::UnsupportedArchiveType)?;

        if archive_type == CondaArchiveType::Conda && options.sparse_policy != SparsePolicy::Disable
        {
            if let Some(archive) = Self::try_open_sparse(client.clone(), url.clone()).await? {
                return Ok(archive);
            }
            if options.sparse_policy == SparsePolicy::Require {
                return Err(ExtractError::SparseAccessUnsupported);
            }
        } else if options.sparse_policy == SparsePolicy::Require {
            return Err(ExtractError::SparseAccessUnsupported);
        }

        Self::open_spooled(client, url, archive_type, options.max_spool_size).await
    }

    /// Opens a package archive from a local file.
    ///
    /// ```rust,no_run
    /// # #[tokio::main]
    /// # async fn main() {
    /// use rattler_package_streaming::archive::PackageArchive;
    ///
    /// let archive = PackageArchive::from_path("numpy-2.1.3-py312h58c1407_0.conda")
    ///     .await
    ///     .unwrap();
    /// # drop(archive);
    /// # }
    /// ```
    pub async fn from_path(path: impl AsRef<Path>) -> Result<Self, ExtractError> {
        let path = path.as_ref();
        let archive_type =
            CondaArchiveType::try_from(path).ok_or(ExtractError::UnsupportedArchiveType)?;
        Self::open_local(path.to_owned(), archive_type, None).await
    }

    /// Returns how this handle accesses the archive.
    pub fn access(&self) -> ArchiveAccess {
        let temp = match &*self.backend {
            Backend::Conda {
                source: CondaSource::Sparse { .. },
                ..
            } => return ArchiveAccess::Sparse,
            Backend::Conda {
                source: CondaSource::Local { temp, .. },
                ..
            }
            | Backend::TarBz2 { temp, .. } => temp,
        };
        if temp.is_some() {
            ArchiveAccess::Spooled
        } else {
            ArchiveAccess::Local
        }
    }

    /// Reads a single file from the package, or `None` if the path does not
    /// exist.
    ///
    /// Contents are not cached: every call streams the containing section
    /// again up to the requested file. Prefer [`PackageArchive::read_files`]
    /// with one batch over repeated calls. Requesting a path that is a
    /// symbolic or hard link is an error; links are not followed.
    ///
    /// ```rust,no_run
    /// # #[tokio::main]
    /// # async fn main() {
    /// # let archive = rattler_package_streaming::archive::PackageArchive::from_path("pkg.conda").await.unwrap();
    /// match archive.read_file("info/recipe/meta.yaml").await.unwrap() {
    ///     Some(bytes) => println!("recipe: {}", String::from_utf8_lossy(&bytes)),
    ///     None => println!("package has no recipe"),
    /// }
    /// # }
    /// ```
    pub async fn read_file(&self, path: impl AsRef<Path>) -> Result<Option<Vec<u8>>, ExtractError> {
        let path = normalize(path.as_ref())?.into_owned();
        let mut result = self.read_files([path.clone()]).await?;
        Ok(result.remove(&path).flatten())
    }

    /// Reads multiple files in one pass per touched section (sections are
    /// fetched concurrently), aborting each stream after its last requested
    /// file. Maps every requested path to its contents, or `None` when
    /// absent.
    ///
    /// Calls are independent and may run concurrently, but contents are not
    /// cached: a repeated call streams its sections again, so batch all
    /// needed paths into a single call where possible. Requesting a path
    /// that is a symbolic or hard link is an error; links are not followed.
    ///
    /// ```rust,no_run
    /// # #[tokio::main]
    /// # async fn main() {
    /// # let archive = rattler_package_streaming::archive::PackageArchive::from_path("pkg.conda").await.unwrap();
    /// // One pass over the payload, one over info, fetched concurrently.
    /// let files = archive
    ///     .read_files(["info/index.json", "lib/libfoo.so", "bin/foo"])
    ///     .await
    ///     .unwrap();
    /// for (path, contents) in &files {
    ///     match contents {
    ///         Some(bytes) => println!("{}: {} bytes", path.display(), bytes.len()),
    ///         None => println!("{}: not in archive", path.display()),
    ///     }
    /// }
    /// # }
    /// ```
    pub async fn read_files(
        &self,
        paths: impl IntoIterator<Item = impl Into<PathBuf>>,
    ) -> Result<HashMap<PathBuf, Option<Vec<u8>>>, ExtractError> {
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .map(|path| {
                let path: PathBuf = path.into();
                normalize(&path).map(|path| path.into_owned())
            })
            .collect::<Result<_, _>>()?;
        if paths.is_empty() {
            return Ok(HashMap::new());
        }

        // A .tar.bz2 archive is one flat tar: serve everything in a single
        // unfiltered pass. Grouping per section here would decompress the
        // whole bz2 stream once per section.
        if let Backend::TarBz2 { path, .. } = &*self.backend {
            let mut stream = Self::tar_bz2_stream(path, None).await?;
            return scan_stream(&mut stream, paths).await;
        }

        let mut groups: HashMap<Section, Vec<PathBuf>> = HashMap::new();
        for path in paths {
            groups
                .entry(Section::containing(&path))
                .or_default()
                .push(path);
        }

        let passes = groups.into_iter().map(|(section, group)| async move {
            match self.stream(section).await {
                Ok(mut stream) => scan_stream(&mut stream, group).await,
                // A section that is absent from the archive simply does not
                // contain any of the requested paths.
                Err(ExtractError::MissingComponent) => {
                    Ok(group.into_iter().map(|path| (path, None)).collect())
                }
                Err(err) => Err(err),
            }
        });
        let results = futures::future::try_join_all(passes).await?;

        Ok(results.into_iter().flatten().collect())
    }

    /// Reads and parses a typed [`PackageFile`], or `None` when the file is
    /// not present in the package (common for `run_exports.json`).
    pub async fn try_read_package_file<P: PackageFile>(&self) -> Result<Option<P>, ExtractError> {
        match self.read_file(P::package_path()).await? {
            None => Ok(None),
            Some(bytes) => parse_package_file(&bytes).map(Some),
        }
    }

    /// Reads and parses a typed [`PackageFile`] (e.g. `IndexJson`,
    /// `PathsJson`) from the package.
    ///
    /// ```rust,no_run
    /// # #[tokio::main]
    /// # async fn main() {
    /// # let archive = rattler_package_streaming::archive::PackageArchive::from_path("pkg.conda").await.unwrap();
    /// use rattler_conda_types::package::IndexJson;
    ///
    /// let index: IndexJson = archive.read_package_file().await.unwrap();
    /// println!("{} {}", index.name.as_normalized(), index.version);
    /// # }
    /// ```
    pub async fn read_package_file<P: PackageFile>(&self) -> Result<P, ExtractError> {
        self.try_read_package_file()
            .await?
            .ok_or(ExtractError::MissingComponent)
    }

    /// Lists the paths of all files (including symbolic links) in one
    /// section.
    ///
    /// For [`Section::Info`] this is usually served from the cached archive
    /// tail without extra requests. For [`Section::Content`] it streams the
    /// entire section; prefer reading `info/paths.json` when only paths are
    /// needed.
    ///
    /// ```rust,no_run
    /// # #[tokio::main]
    /// # async fn main() {
    /// # let archive = rattler_package_streaming::archive::PackageArchive::from_path("pkg.conda").await.unwrap();
    /// use rattler_package_streaming::archive::Section;
    ///
    /// // Usually free: the info section tends to sit in the cached tail.
    /// for path in archive.list_files(Section::Info).await.unwrap() {
    ///     println!("{}", path.display());
    /// }
    /// # }
    /// ```
    pub async fn list_files(&self, section: Section) -> Result<Vec<PathBuf>, ExtractError> {
        let mut stream = self.stream(section).await?;
        let mut paths = Vec::new();
        while let Some(entry) = stream.next_entry().await? {
            if matches!(
                entry.kind(),
                ArchiveEntryKind::File | ArchiveEntryKind::Symlink | ArchiveEntryKind::Hardlink
            ) {
                paths.push(entry.path().to_owned());
            }
        }
        Ok(paths)
    }

    /// Streams the tar entries of one section. Unread entries are skipped
    /// cheaply; dropping the stream aborts any underlying HTTP transfer.
    ///
    /// Every call opens a new independent forward-only stream (for remote
    /// archives: a new request).
    ///
    /// ```rust,no_run
    /// # #[tokio::main]
    /// # async fn main() {
    /// # let archive = rattler_package_streaming::archive::PackageArchive::from_path("pkg.conda").await.unwrap();
    /// use rattler_package_streaming::archive::Section;
    ///
    /// let mut stream = archive.stream(Section::Content).await.unwrap();
    /// while let Some(mut entry) = stream.next_entry().await.unwrap() {
    ///     let path = entry.path().to_owned();
    ///     if path.extension().is_some_and(|ext| ext == "so") {
    ///         let bytes = entry.read().await.unwrap();
    ///         println!("{}: {} bytes", path.display(), bytes.len());
    ///     } // entries that are not read are skipped cheaply
    /// }
    /// # }
    /// ```
    pub async fn stream(&self, section: Section) -> Result<SectionStream, ExtractError> {
        match &*self.backend {
            Backend::Conda { source, members } => {
                let span = find_section_member(members, section)?;
                let raw = Self::conda_member_reader(source, span).await?;
                let decoder =
                    ZstdDecoder::new(tokio::io::BufReader::with_capacity(STREAM_BUF_SIZE, raw));
                Ok(SectionStream::new(Box::new(decoder), None))
            }
            // `read_files` bypasses the filter with `tar_bz2_stream(None)`
            // to serve both sections from a single pass.
            Backend::TarBz2 { path, .. } => Self::tar_bz2_stream(path, Some(section)).await,
        }
    }

    // ---------------------------------------------------------------------
    // opening
    // ---------------------------------------------------------------------

    /// Opens a remote `.conda` archive sparsely, or `None` when the server
    /// does not support the required range requests and the caller should
    /// fall back to a full download.
    pub(crate) async fn try_open_sparse(
        client: ClientWithMiddleware,
        url: Url,
    ) -> Result<Option<Self>, ExtractError> {
        match Self::open_sparse(client, url).await {
            Ok(archive) => Ok(Some(archive)),
            Err(err) if sparse_unsupported(&err) => {
                debug!("sparse access unavailable ({err}), falling back to full download");
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    /// Opens a remote `.conda` archive sparsely, without full-download fallback.
    pub(crate) async fn open_sparse(
        client: ClientWithMiddleware,
        url: Url,
    ) -> Result<Self, ExtractError> {
        // One suffix range request: fetches the last TAIL_SIZE bytes and
        // reveals the total archive size.
        let (reader, headers) = AsyncHttpRangeReader::new(
            client.clone(),
            url.clone(),
            CheckSupportMethod::NegativeRangeRequest(TAIL_SIZE),
            HeaderMap::default(),
        )
        .await?;
        // A weak ETag must not be sent in `If-Range` (RFC 9110 §13.1.5);
        // fall back to `Last-Modified` in that case.
        let validator = headers
            .get(ETAG)
            .filter(|v| !v.as_bytes().starts_with(b"W/"))
            .or_else(|| headers.get(LAST_MODIFIED))
            .cloned();
        let size = reader.len();
        debug!("opened remote archive ({size} bytes) with a {TAIL_SIZE} byte tail request");

        // Parse the central directory. The needed bytes are already cached
        // from the tail request; if the central directory is unusually large
        // the range reader transparently fetches the difference.
        let buf_reader = futures::io::BufReader::new(reader.compat());
        let zip = ZipFileReader::new(buf_reader).await?;
        let members = collect_members(zip.file(), size)?;

        // Recover the range reader and keep a copy of the tail bytes so
        // members that live inside the tail (usually the info section) can be
        // served without further requests.
        let mut reader = zip.into_inner().into_inner().into_inner();
        let tail_offset = size.saturating_sub(TAIL_SIZE);
        let mut tail = vec![0u8; (size - tail_offset) as usize];
        reader.seek(SeekFrom::Start(tail_offset)).await?;
        reader.read_exact(&mut tail).await?;

        Ok(Self {
            backend: Arc::new(Backend::Conda {
                source: CondaSource::Sparse {
                    client,
                    url,
                    validator,
                    tail_offset,
                    tail: tail.into(),
                },
                members,
            }),
        })
    }

    async fn open_spooled(
        client: ClientWithMiddleware,
        url: Url,
        archive_type: CondaArchiveType,
        max_spool_size: Option<u64>,
    ) -> Result<Self, ExtractError> {
        let response = client
            .get(url.clone())
            .send()
            .await?
            .error_for_status()
            .map_err(|e| ExtractError::ReqwestError(e.into()))?;

        if let (Some(limit), Some(content_length)) = (max_spool_size, response.content_length())
            && content_length > limit
        {
            return Err(ExtractError::SpoolLimitExceeded { limit });
        }

        // Spool to disk rather than memory: packages can be arbitrarily
        // large (multi-GB), so an in-memory copy is not an option.
        let temp = tempfile::NamedTempFile::new()?;
        let (file, temp_path) = temp.into_parts();
        let mut file = tokio::fs::File::from_std(file);
        let body = StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
        let copied = if let Some(limit) = max_spool_size {
            let mut body = body.take(limit.saturating_add(1));
            tokio::io::copy(&mut body, &mut file).await?
        } else {
            let mut body = body;
            tokio::io::copy(&mut body, &mut file).await?
        };
        if let Some(limit) = max_spool_size
            && copied > limit
        {
            return Err(ExtractError::SpoolLimitExceeded { limit });
        }
        file.flush().await?;

        Self::open_local(temp_path.to_path_buf(), archive_type, Some(temp_path)).await
    }

    async fn open_local(
        path: PathBuf,
        archive_type: CondaArchiveType,
        temp: Option<tempfile::TempPath>,
    ) -> Result<Self, ExtractError> {
        let backend = match archive_type {
            CondaArchiveType::Conda => {
                let file = tokio::fs::File::open(&path).await?;
                let size = file.metadata().await?.len();
                let buf_reader =
                    futures::io::BufReader::new(tokio::io::BufReader::new(file).compat());
                let zip = ZipFileReader::new(buf_reader).await?;
                let members = collect_members(zip.file(), size)?;
                Backend::Conda {
                    source: CondaSource::Local { path, temp },
                    members,
                }
            }
            CondaArchiveType::TarBz2 => Backend::TarBz2 { path, temp },
        };
        Ok(Self {
            backend: Arc::new(backend),
        })
    }

    // ---------------------------------------------------------------------
    // section readers
    // ---------------------------------------------------------------------

    /// Returns a reader over the stored bytes of a ZIP member.
    async fn conda_member_reader(
        source: &CondaSource,
        span: &MemberSpan,
    ) -> Result<DynReader, ExtractError> {
        match source {
            CondaSource::Sparse {
                client,
                url,
                validator,
                tail_offset,
                tail,
            } => {
                // Serve from the cached tail when the whole member is inside it.
                if span.header_offset >= *tail_offset {
                    let rel = (span.header_offset - tail_offset) as usize;
                    if let Some(range) = member_data_range(&tail[rel..], span.size) {
                        debug!("serving member {} from the cached tail", span.name);
                        let data = tail.slice(rel + range.start..rel + range.end);
                        return Ok(Box::new(std::io::Cursor::new(data)));
                    }
                }

                // One bounded streaming ranged GET for the member. Dropping
                // the returned reader aborts the transfer.
                debug!(
                    "requesting range {}-{} for member {}",
                    span.header_offset,
                    span.end - 1,
                    span.name
                );
                let mut request = client
                    .get(url.clone())
                    .header(
                        RANGE,
                        format!("bytes={}-{}", span.header_offset, span.end - 1),
                    )
                    // Forbid content-coding: byte math relies on the exact
                    // stored representation.
                    .header(http::header::ACCEPT_ENCODING, "identity");
                if let Some(validator) = validator {
                    request = request.header(IF_RANGE, validator);
                }
                let response = request
                    .send()
                    .await?
                    .error_for_status()
                    .map_err(|e| ExtractError::ReqwestError(e.into()))?;
                if response.status() != ::reqwest::StatusCode::PARTIAL_CONTENT {
                    // An honored `If-Range` mismatch (the archive changed since
                    // it was opened) or a server that stopped honoring ranges.
                    return Err(ExtractError::RemoteArchiveChanged);
                }
                let mut reader =
                    StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
                skip_local_header(&mut reader).await?;
                Ok(Box::new(reader.take(span.size)))
            }
            CondaSource::Local { path, .. } => {
                let mut file = tokio::fs::File::open(path).await?;
                file.seek(SeekFrom::Start(span.header_offset)).await?;
                let mut reader = tokio::io::BufReader::new(file);
                skip_local_header(&mut reader).await?;
                Ok(Box::new(reader.take(span.size)))
            }
        }
    }

    /// Opens a (optionally section-filtered) stream over a `.tar.bz2` archive.
    async fn tar_bz2_stream(
        path: &Path,
        section: Option<Section>,
    ) -> Result<SectionStream, ExtractError> {
        let file = tokio::fs::File::open(path).await?;
        let decoder = BzDecoder::new(tokio::io::BufReader::with_capacity(STREAM_BUF_SIZE, file));
        Ok(SectionStream::new(Box::new(decoder), section))
    }
}

/// A streaming view over the tar entries of one package section.
pub struct SectionStream {
    entries: tokio_tar::Entries<DynReader>,
    /// For `.tar.bz2` archives (one flat tar), entries are filtered to the
    /// requested section. `None` yields every entry.
    filter: Option<Section>,
}

impl SectionStream {
    fn new(reader: DynReader, filter: Option<Section>) -> Self {
        let mut archive = tokio_tar::Archive::new(reader);
        let entries = archive
            .entries()
            .expect("entries() cannot fail on a fresh archive");
        Self { entries, filter }
    }

    /// Advances to the next tar entry of the section, or `None` at the end of
    /// the section.
    pub async fn next_entry(&mut self) -> Result<Option<SectionEntry>, ExtractError> {
        use futures_util::StreamExt;
        while let Some(entry) = self.entries.next().await {
            let entry = entry?;
            let path = {
                let path = entry.path()?;
                normalize(&path)?.into_owned()
            };
            if let Some(section) = self.filter
                && Section::containing(&path) != section
            {
                continue;
            }
            return Ok(Some(SectionEntry { inner: entry, path }));
        }
        Ok(None)
    }
}

/// Reads the requested paths out of a section stream, aborting as soon as the
/// last one has been found.
async fn scan_stream(
    stream: &mut SectionStream,
    paths: Vec<PathBuf>,
) -> Result<HashMap<PathBuf, Option<Vec<u8>>>, ExtractError> {
    let mut remaining: HashSet<PathBuf> = paths.into_iter().collect();
    let mut out = HashMap::with_capacity(remaining.len());
    let mut links: Vec<String> = Vec::new();
    while !remaining.is_empty() {
        let Some(mut entry) = stream.next_entry().await? else {
            break;
        };
        let path = entry.path().to_owned();
        if remaining.remove(&path) {
            // Finish the scan so the error names every offending link
            // instead of discarding the whole batch on the first one.
            if let Some(link) = describe_link(&entry)? {
                links.push(link);
                continue;
            }
            let buf = entry.read().await?;
            out.insert(path, Some(buf));
        }
    }
    if !links.is_empty() {
        return Err(ExtractError::LinksNotFollowed(links));
    }
    for path in remaining {
        out.insert(path, None);
    }
    Ok(out)
}

/// Collects the member spans of a `.conda` ZIP archive from its parsed
/// central directory. The exclusive end bound of each member is the offset of
/// the next member (or the end of the archive), which over-approximates by at
/// most the size of the central directory for the last member.
fn collect_members(
    zip: &async_zip::ZipFile,
    archive_size: u64,
) -> Result<Vec<MemberSpan>, ExtractError> {
    let entries = zip.entries();
    let mut members = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry
            .filename()
            .as_str()
            .map_err(|e| {
                ExtractError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            })?
            .to_owned();
        if name.ends_with(".tar.zst") && entry.compression() != Compression::Stored {
            return Err(ExtractError::UnsupportedCompressionMethod);
        }
        members.push(MemberSpan {
            name,
            header_offset: entry.header_offset(),
            size: entry.compressed_size(),
            end: archive_size,
        });
    }
    // Bound each member by the next member's local header offset.
    members.sort_unstable_by_key(|m| m.header_offset);
    for i in 1..members.len() {
        members[i - 1].end = members[i].header_offset;
    }
    Ok(members)
}

fn find_section_member(
    members: &[MemberSpan],
    section: Section,
) -> Result<&MemberSpan, ExtractError> {
    let prefix = section.zip_prefix();
    members
        .iter()
        .find(|m| m.name.starts_with(prefix) && m.name.ends_with(".tar.zst"))
        .ok_or(ExtractError::MissingComponent)
}

/// Describes a link entry for [`ExtractError::LinksNotFollowed`], or `None`
/// for regular entries.
fn describe_link(entry: &SectionEntry) -> Result<Option<String>, ExtractError> {
    if !entry.kind().is_link() {
        return Ok(None);
    }
    let target = entry
        .link_target()?
        .map(|target| target.display().to_string())
        .unwrap_or_default();
    Ok(Some(format!(
        "'{}' (links to '{target}')",
        entry.path().display()
    )))
}

/// Reads the contents of a raw tar entry while capping the upfront allocation
/// derived from its untrusted header.
pub(crate) async fn read_raw_entry_contents<R: AsyncRead + Unpin>(
    entry: &mut tokio_tar::Entry<R>,
) -> Result<Vec<u8>, ExtractError> {
    let size = entry.header().size()?;
    let mut buf = Vec::with_capacity(size.min(MAX_PREALLOC) as usize);
    entry.read_to_end(&mut buf).await?;
    Ok(buf)
}

/// Parses the raw bytes of a typed [`PackageFile`].
pub(crate) fn parse_package_file<P: PackageFile>(bytes: &[u8]) -> Result<P, ExtractError> {
    P::from_slice(bytes)
        .map_err(|e| ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e))
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

/// Parses a ZIP local file header at the start of `buf` and returns the
/// range of the member data if `buf` contains all of it.
fn member_data_range(buf: &[u8], size: u64) -> Option<std::ops::Range<usize>> {
    if buf.len() < 30 || buf[0..4] != LOCAL_HEADER_MAGIC {
        return None;
    }
    let name_len = u16::from_le_bytes([buf[26], buf[27]]) as usize;
    let extra_len = u16::from_le_bytes([buf[28], buf[29]]) as usize;
    let data_start = 30 + name_len + extra_len;
    let data_end = data_start.checked_add(size as usize)?;
    (data_end <= buf.len()).then_some(data_start..data_end)
}

/// Reads and skips a ZIP local file header from a stream, leaving the reader
/// positioned at the start of the member data.
async fn skip_local_header<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(), ExtractError> {
    let mut header = [0u8; 30];
    reader.read_exact(&mut header).await?;
    if header[0..4] != LOCAL_HEADER_MAGIC {
        return Err(ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expected a ZIP local file header",
        )));
    }
    let name_len = u64::from(u16::from_le_bytes([header[26], header[27]]));
    let extra_len = u64::from(u16::from_le_bytes([header[28], header[29]]));
    let mut skip = reader.take(name_len + extra_len);
    tokio::io::copy(&mut skip, &mut tokio::io::sink()).await?;
    Ok(())
}

/// Returns true for errors that mean "sparse access is unavailable" and the
/// caller should fall back to a full download.
fn sparse_unsupported(err: &ExtractError) -> bool {
    match err {
        // Servers that ignore the `Range` header answer with a plain `200 OK`
        // that carries no `Content-Range` header.
        ExtractError::AsyncHttpRangeReaderError(
            AsyncHttpRangeReaderError::HttpRangeRequestUnsupported
            | AsyncHttpRangeReaderError::ContentRangeMissing,
        ) => true,
        // JFrog Artifactory returns 416 when querying more than the object length.
        ExtractError::AsyncHttpRangeReaderError(AsyncHttpRangeReaderError::HttpError(err)) => {
            err.status() == Some(::reqwest::StatusCode::RANGE_NOT_SATISFIABLE)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rattler_conda_types::package::{AboutJson, IndexJson};

    use super::*;
    use crate::reqwest::test_server;

    fn conda_test_file() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber/clobber-fd-1-0.1.0-h4616a5c_0.conda")
    }

    fn tar_bz2_test_file() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2")
    }

    /// A middleware that counts the HTTP requests going through a client.
    struct RequestCounter(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl reqwest_middleware::Middleware for RequestCounter {
        async fn handle(
            &self,
            req: ::reqwest::Request,
            extensions: &mut http::Extensions,
            next: reqwest_middleware::Next<'_>,
        ) -> reqwest_middleware::Result<::reqwest::Response> {
            self.0.fetch_add(1, Ordering::Relaxed);
            next.run(req, extensions).await
        }
    }

    fn counting_client() -> (ClientWithMiddleware, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let client = reqwest_middleware::ClientBuilder::new(::reqwest::Client::new())
            .with(RequestCounter(counter.clone()))
            .build();
        (client, counter)
    }

    #[tokio::test]
    async fn test_sparse_conda_round_trip() {
        let url = test_server::serve_file(conda_test_file()).await;
        let (client, requests) = counting_client();

        let archive = PackageArchive::from_url(client, url).await.unwrap();
        assert_eq!(archive.access(), ArchiveAccess::Sparse);
        assert_eq!(requests.load(Ordering::Relaxed), 1, "open = 1 request");

        // Typed metadata reads: the test package is tiny, so everything is
        // served from the cached tail without further requests.
        let index: IndexJson = archive.read_package_file().await.unwrap();
        assert_eq!(index.name.as_normalized(), "clobber-fd-1");
        let _about: AboutJson = archive.read_package_file().await.unwrap();
        assert_eq!(
            requests.load(Ordering::Relaxed),
            1,
            "metadata reads served from the tail cache"
        );

        // Payload + metadata in one batched call.
        let files = archive
            .read_files(["clobber", "info/index.json", "does/not/exist"])
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(files[Path::new("clobber")].clone().unwrap()).unwrap(),
            "clobber-fd-1\n"
        );
        assert!(files[Path::new("info/index.json")].is_some());
        assert!(files[Path::new("does/not/exist")].is_none());
        assert_eq!(
            requests.load(Ordering::Relaxed),
            1,
            "tiny package: payload also served from the tail cache"
        );
    }

    #[tokio::test]
    async fn test_stream_section() {
        let url = test_server::serve_file(conda_test_file()).await;
        let (client, _) = counting_client();

        let archive = PackageArchive::from_url(client, url).await.unwrap();
        let mut names = Vec::new();
        let mut stream = archive.stream(Section::Info).await.unwrap();
        while let Some(entry) = stream.next_entry().await.unwrap() {
            names.push(entry.path().display().to_string());
        }
        assert!(names.iter().any(|n| n == "info/index.json"), "{names:?}");
    }

    #[tokio::test]
    async fn test_tar_bz2_spooled() {
        let url = test_server::serve_file(tar_bz2_test_file()).await;
        let (client, requests) = counting_client();

        let archive = PackageArchive::from_url(client, url).await.unwrap();
        assert_eq!(archive.access(), ArchiveAccess::Spooled);
        assert_eq!(requests.load(Ordering::Relaxed), 1, "one full download");

        let files = archive
            .read_files(["info/index.json", "clobber.txt"])
            .await
            .unwrap();
        assert!(files[Path::new("info/index.json")].is_some());
        assert!(files[Path::new("clobber.txt")].is_some());

        let index: IndexJson = archive.read_package_file().await.unwrap();
        assert_eq!(index.name.as_normalized(), "clobber-1");
        assert_eq!(
            requests.load(Ordering::Relaxed),
            1,
            "spooled archive is downloaded exactly once"
        );

        // Section streaming filters the flat tar by prefix.
        let mut stream = archive.stream(Section::Content).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = stream.next_entry().await.unwrap() {
            names.push(entry.path().display().to_string());
        }
        assert!(names.iter().all(|n| !n.starts_with("info/")), "{names:?}");
        assert!(names.iter().any(|n| n == "clobber.txt"), "{names:?}");
    }

    #[tokio::test]
    async fn test_conda_no_range_support_fallback() {
        let url = test_server::serve_file_no_ranges(conda_test_file()).await;
        let (client, requests) = counting_client();

        let archive = PackageArchive::from_url(client, url).await.unwrap();
        assert_eq!(archive.access(), ArchiveAccess::Spooled);
        assert_eq!(
            requests.load(Ordering::Relaxed),
            2,
            "one failed range probe + one full download"
        );

        let index: IndexJson = archive.read_package_file().await.unwrap();
        assert_eq!(index.name.as_normalized(), "clobber-fd-1");
        let content = archive.read_file("clobber").await.unwrap().unwrap();
        assert_eq!(String::from_utf8(content).unwrap(), "clobber-fd-1\n");
        assert_eq!(
            requests.load(Ordering::Relaxed),
            2,
            "all reads served from the spool file"
        );
    }

    /// A package larger than the 64 KiB tail: payload reads must go through
    /// the ranged member-GET path (local header skip, `If-Range`, end bound).
    #[tokio::test]
    async fn test_sparse_large_package() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/sparse/sparse-test-1.0.0-0.conda");
        let url = test_server::serve_file(fixture).await;
        let (client, requests) = counting_client();

        let archive = PackageArchive::from_url(client, url).await.unwrap();
        assert_eq!(requests.load(Ordering::Relaxed), 1, "open = 1 request");

        // The info member sits inside the tail: no extra request. Leading
        // `./` components are normalized away.
        let index = archive
            .read_file("./info/index.json")
            .await
            .unwrap()
            .expect("index.json should exist");
        assert!(!index.is_empty());
        assert_eq!(requests.load(Ordering::Relaxed), 1);

        // The payload member lies outside the tail: exactly one ranged GET,
        // shared by both files.
        let files = archive
            .read_files(["bin/first-file.txt", "share/last-file.txt"])
            .await
            .unwrap();
        assert_eq!(
            files[Path::new("bin/first-file.txt")].as_deref(),
            Some(b"first payload file\n".as_slice())
        );
        assert_eq!(
            files[Path::new("share/last-file.txt")].as_deref(),
            Some(b"last payload file\n".as_slice())
        );
        assert_eq!(
            requests.load(Ordering::Relaxed),
            2,
            "payload batch = 1 ranged request"
        );

        let names = archive.list_files(Section::Content).await.unwrap();
        assert_eq!(names.len(), 3, "{names:?}");
    }

    #[tokio::test]
    async fn test_list_files() {
        let archive = PackageArchive::from_path(conda_test_file()).await.unwrap();
        let info = archive.list_files(Section::Info).await.unwrap();
        assert!(
            info.iter().any(|p| p == Path::new("info/index.json")),
            "{info:?}"
        );
        let content = archive.list_files(Section::Content).await.unwrap();
        assert_eq!(content, vec![PathBuf::from("clobber")]);
    }

    #[tokio::test]
    async fn test_symlinks_surfaced_not_followed() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/sparse/symlink-test-1.0.0-0.conda");
        let archive = PackageArchive::from_path(&fixture).await.unwrap();

        // Symbolic links show up in listings...
        let files = archive.list_files(Section::Content).await.unwrap();
        assert!(
            files.contains(&PathBuf::from("lib/liblink.so")),
            "{files:?}"
        );
        assert!(
            files.contains(&PathBuf::from("lib/libhard.so")),
            "{files:?}"
        );
        assert!(
            files.contains(&PathBuf::from("lib/libreal.so.1")),
            "{files:?}"
        );

        // ...their targets read fine...
        let real = archive.read_file("lib/libreal.so.1").await.unwrap();
        assert_eq!(real.as_deref(), Some(b"real library bytes".as_slice()));

        let mut stream = archive.stream(Section::Content).await.unwrap();
        let mut kinds = HashMap::new();
        while let Some(entry) = stream.next_entry().await.unwrap() {
            kinds.insert(entry.path().to_owned(), entry.kind());
        }
        assert_eq!(
            kinds[Path::new("lib/liblink.so")],
            ArchiveEntryKind::Symlink
        );
        assert_eq!(
            kinds[Path::new("lib/libhard.so")],
            ArchiveEntryKind::Hardlink
        );

        // ...but reading a link itself is an error, for both link kinds.
        for link in ["lib/liblink.so", "lib/libhard.so"] {
            let err = archive.read_file(link).await.unwrap_err();
            assert!(err.to_string().contains("links are not followed"), "{err}");
        }
    }

    #[tokio::test]
    async fn test_missing_section_reads_as_none() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/sparse/info-only-1.0.0-0.conda");
        let archive = PackageArchive::from_path(&fixture).await.unwrap();

        // A path in an absent section is simply not in the archive.
        let files = archive
            .read_files(["bin/missing", "info/index.json"])
            .await
            .unwrap();
        assert!(files[Path::new("bin/missing")].is_none());
        assert!(files[Path::new("info/index.json")].is_some());

        // Asking for the section itself is still an error.
        assert!(matches!(
            archive.stream(Section::Content).await,
            Err(ExtractError::MissingComponent)
        ));
    }

    /// The fixture uses zip64 local headers; the reader must skip their
    /// zip64 extra fields correctly (sizes themselves come from the central
    /// directory).
    #[tokio::test]
    async fn test_zip64_local_headers() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/sparse/zip64-test-1.0.0-0.conda");
        let url = test_server::serve_file(fixture).await;
        let (client, _) = counting_client();

        let archive = PackageArchive::from_url(client, url).await.unwrap();
        let content = archive.read_file("bin/hello.txt").await.unwrap();
        assert_eq!(content.as_deref(), Some(b"zip64 payload\n".as_slice()));
    }

    #[tokio::test]
    async fn test_tar_bz2_list_files() {
        let archive = PackageArchive::from_path(tar_bz2_test_file())
            .await
            .unwrap();
        let info = archive.list_files(Section::Info).await.unwrap();
        assert!(info.contains(&PathBuf::from("info/index.json")), "{info:?}");
        let content = archive.list_files(Section::Content).await.unwrap();
        assert!(
            content.contains(&PathBuf::from("clobber.txt")),
            "{content:?}"
        );
        assert!(
            content.iter().all(|p| !p.starts_with("info")),
            "{content:?}"
        );
    }

    #[tokio::test]
    async fn test_try_read_package_file_absent() {
        use rattler_conda_types::package::RunExportsJson;
        let archive = PackageArchive::from_path(conda_test_file()).await.unwrap();
        let run_exports: Option<RunExportsJson> = archive.try_read_package_file().await.unwrap();
        assert!(run_exports.is_none());
    }

    /// `JFrog` Artifactory answers suffix ranges beyond the object length with
    /// 416; the handle must fall back to spooling.
    #[tokio::test]
    async fn test_conda_416_suffix_fallback() {
        let url = test_server::serve_file_416_suffix(conda_test_file()).await;
        let (client, requests) = counting_client();

        let archive = PackageArchive::from_url(client, url).await.unwrap();
        assert_eq!(archive.access(), ArchiveAccess::Spooled);
        assert_eq!(
            requests.load(Ordering::Relaxed),
            2,
            "rejected range probe + one full download"
        );
        let content = archive.read_file("clobber").await.unwrap().unwrap();
        assert_eq!(String::from_utf8(content).unwrap(), "clobber-fd-1\n");
    }

    /// A republished archive must fail loudly, not yield garbage — even on
    /// servers (like this test server) that ignore `If-Range`.
    #[tokio::test]
    async fn test_archive_replaced_mid_read_errors() {
        let dir = tempfile::tempdir().unwrap();
        let served = dir.path().join("replaced-test-1.0.0-0.conda");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../test-data/sparse/sparse-test-1.0.0-0.conda"),
            &served,
        )
        .unwrap();
        let url = test_server::serve_file(&served).await;
        let (client, _) = counting_client();

        let archive = PackageArchive::from_url(client, url).await.unwrap();

        // Replace the archive on the server with a different package.
        std::fs::copy(conda_test_file(), &served).unwrap();

        // The payload member no longer matches the parsed index; the read
        // must error rather than return bytes from the wrong archive.
        assert!(archive.read_file("bin/first-file.txt").await.is_err());
    }

    /// Entries stored with a leading `./` (as `tar -C dir -c .` produces)
    /// must round-trip between `list_files` and `read_file`.
    #[tokio::test]
    async fn test_dot_slash_entries_round_trip() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/sparse/dotslash-test-1.0.0-0.conda");
        let archive = PackageArchive::from_path(&fixture).await.unwrap();

        let files = archive.list_files(Section::Content).await.unwrap();
        assert_eq!(files, vec![PathBuf::from("lib/data.txt")]);

        for spelling in ["lib/data.txt", "./lib/data.txt"] {
            let content = archive.read_file(spelling).await.unwrap();
            assert_eq!(
                content.as_deref(),
                Some(b"dot slash payload\n".as_slice()),
                "{spelling}"
            );
        }
        assert!(
            archive
                .read_file("info/index.json")
                .await
                .unwrap()
                .is_some()
        );
    }

    /// A link in a batch fails the read, but only after the scan completes,
    /// with an error naming the offending path.
    #[tokio::test]
    async fn test_link_error_names_offending_path() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/sparse/symlink-test-1.0.0-0.conda");
        let archive = PackageArchive::from_path(&fixture).await.unwrap();

        let err = archive
            .read_files(["lib/libreal.so.1", "lib/liblink.so"])
            .await
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("'lib/liblink.so'"), "{message}");
        assert!(
            !message.contains("'lib/libreal.so.1'"),
            "the regular file must not be reported as offending: {message}"
        );
    }

    #[test]
    fn test_section_containing() {
        assert_eq!(
            Section::containing(Path::new("info/index.json")),
            Section::Info
        );
        assert_eq!(
            Section::containing(Path::new("./info/index.json")),
            Section::Info
        );
        assert_eq!(Section::containing(Path::new("info")), Section::Info);
        assert_eq!(
            Section::containing(Path::new("info-custom.txt")),
            Section::Content
        );
        assert_eq!(
            Section::containing(Path::new("information/file")),
            Section::Content
        );
        assert_eq!(
            Section::containing(Path::new("lib/libz.so")),
            Section::Content
        );
    }

    #[tokio::test]
    async fn test_remote_access_policy() {
        let url = test_server::serve_file_no_ranges(conda_test_file()).await;
        let (client, requests) = counting_client();
        let options = RemoteArchiveOptions::new().with_sparse_policy(SparsePolicy::Require);
        let error = match PackageArchive::from_url_with_options(client, url, options).await {
            Ok(_) => panic!("range support should have been required"),
            Err(error) => error,
        };
        assert!(matches!(error, ExtractError::SparseAccessUnsupported));
        assert_eq!(requests.load(Ordering::Relaxed), 1);

        let url = test_server::serve_file(conda_test_file()).await;
        let (client, requests) = counting_client();
        let options = RemoteArchiveOptions::new().with_sparse_policy(SparsePolicy::Disable);
        let archive = PackageArchive::from_url_with_options(client, url, options)
            .await
            .unwrap();
        assert_eq!(archive.access(), ArchiveAccess::Spooled);
        assert_eq!(requests.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_spool_size_limit() {
        let url = test_server::serve_file_no_ranges(conda_test_file()).await;
        let (client, _) = counting_client();
        let options = RemoteArchiveOptions::new().with_max_spool_size(1);
        let error = match PackageArchive::from_url_with_options(client, url, options).await {
            Ok(_) => panic!("the spool limit should have rejected the download"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            ExtractError::SpoolLimitExceeded { limit: 1 }
        ));
    }

    #[tokio::test]
    async fn test_invalid_archive_paths() {
        let archive = PackageArchive::from_path(conda_test_file()).await.unwrap();
        for path in ["", ".", "../clobber", "/clobber"] {
            assert!(matches!(
                archive.read_file(path).await,
                Err(ExtractError::InvalidArchivePath(_))
            ));
        }
    }

    #[tokio::test]
    async fn test_local_conda() {
        let archive = PackageArchive::from_path(conda_test_file()).await.unwrap();
        assert_eq!(archive.access(), ArchiveAccess::Local);
        let index: IndexJson = archive.read_package_file().await.unwrap();
        assert_eq!(index.name.as_normalized(), "clobber-fd-1");
        let content = archive.read_file("clobber").await.unwrap().unwrap();
        assert_eq!(String::from_utf8(content).unwrap(), "clobber-fd-1\n");
    }
}
