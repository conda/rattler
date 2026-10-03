//! A conda package archive read over HTTP with range requests, from the
//! browser.
//!
//! The archive is parsed by the synchronous readers of
//! `rattler_package_streaming` over a [`SparseReader`]. When they need bytes
//! that have not been fetched yet, the missing range is fetched and the read
//! runs again.

use std::{
    cell::RefCell,
    io::Read,
    ops::Range,
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use bytes::Bytes;
use rattler_conda_types::package::{
    AboutJson, CondaArchiveType, IndexJson, PackageFile, PathsJson, RunExportsJson,
};
use rattler_package_streaming::{ExtractError, read::stream_tar_bz2, seek};
use reqwest::{StatusCode, header::RANGE};
use serde::Serialize;
use url::Url;
use wasm_bindgen::prelude::*;

use crate::{
    JsError, JsResult,
    sparse_reader::{Fetched, SparseReader},
};

/// Bytes fetched from the end of a `.conda` archive on open: enough for the
/// ZIP central directory, and as the info member is written last, usually
/// the whole info section too.
const TAIL_SIZE: u64 = 64 * 1024;

/// Cap for upfront buffer allocations based on (untrusted) tar header sizes.
const MAX_PREALLOC: u64 = 4 * 1024 * 1024;

/// Fetches byte ranges of a file over HTTP.
///
/// Browsers do not expose the `Content-Range` header of a cross-origin
/// response, so a server that ignores `Range` and answers `200 OK` is told
/// apart by its status, and its whole body is kept.
struct HttpSource {
    client: reqwest::Client,
    url: Url,
}

impl HttpSource {
    fn error(&self, what: &str, err: impl std::fmt::Display) -> JsError {
        JsError::Fetch(format!("could not {what} {}: {err}", self.url))
    }

    /// The size of the file, from a `HEAD` request. Without a usable
    /// `Content-Length` the file is downloaded, and returned as well.
    async fn len(&self) -> JsResult<(u64, Option<Bytes>)> {
        let response = self
            .client
            .head(self.url.clone())
            .send()
            .await
            .map_err(|err| self.error("reach", err))?;
        if !response.status().is_success() {
            return Err(self.error("reach", response.status()));
        }
        if let Some(len) = response.content_length() {
            return Ok((len, None));
        }
        let response = self
            .client
            .get(self.url.clone())
            .send()
            .await
            .map_err(|err| self.error("download", err))?;
        if !response.status().is_success() {
            return Err(self.error("download", response.status()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|err| self.error("download", err))?;
        Ok((bytes.len() as u64, Some(bytes)))
    }

    /// Fetches `range` and returns the offset the returned bytes start at:
    /// `range.start`, or 0 when the server sent the whole file.
    async fn fetch(&self, range: Range<u64>, len: u64) -> JsResult<(u64, Bytes)> {
        let response = self
            .client
            .get(self.url.clone())
            .header(RANGE, format!("bytes={}-{}", range.start, range.end - 1))
            .send()
            .await
            .map_err(|err| self.error("read", err))?;
        let (start, expected) = match response.status() {
            StatusCode::PARTIAL_CONTENT => (range.start, range.end - range.start),
            // The server ignored the range and sent everything.
            StatusCode::OK => (0, len),
            status => return Err(self.error("read", status)),
        };
        let bytes = response
            .bytes()
            .await
            .map_err(|err| self.error("read", err))?;
        if bytes.len() as u64 != expected {
            return Err(self.error(
                "read",
                format!(
                    "expected {expected} bytes from offset {start}, got {}",
                    bytes.len()
                ),
            ));
        }
        Ok((start, bytes))
    }
}

/// The two sections of a conda package.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    /// Everything under `info/`.
    Info,
    /// The package payload.
    Pkg,
}

impl Section {
    fn parse(section: &str) -> JsResult<Self> {
        match section {
            "info" => Ok(Section::Info),
            "pkg" => Ok(Section::Pkg),
            other => Err(JsError::InvalidSection(other.to_owned())),
        }
    }

    fn containing(path: &Path) -> Self {
        match path.components().next() {
            Some(Component::Normal(first)) if first == "info" => Section::Info,
            _ => Section::Pkg,
        }
    }
}

/// Validates a package-relative path and strips `.` components.
fn normalize(path: &Path) -> Result<PathBuf, ExtractError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ExtractError::InvalidArchivePath(path.to_owned()));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(ExtractError::InvalidArchivePath(path.to_owned()));
    }
    Ok(normalized)
}

/// The tar archive holding `section`. For a `.tar.bz2` archive this is the
/// whole package, and callers filter by path.
fn section_tar(
    reader: SparseReader,
    archive_type: CondaArchiveType,
    section: Section,
) -> Result<tar::Archive<Box<dyn Read>>, ExtractError> {
    let inner: Box<dyn Read> = match (archive_type, section) {
        (CondaArchiveType::Conda, Section::Info) => {
            Box::new(seek::stream_conda_info(reader)?.into_inner())
        }
        (CondaArchiveType::Conda, Section::Pkg) => {
            Box::new(seek::stream_conda_content(reader)?.into_inner())
        }
        (CondaArchiveType::TarBz2, _) => Box::new(stream_tar_bz2(reader).into_inner()),
    };
    Ok(tar::Archive::new(inner))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsArchiveEntry {
    path: String,
    size: u64,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    link_target: Option<String>,
}

fn list_entries(
    reader: SparseReader,
    archive_type: CondaArchiveType,
    section: Section,
) -> Result<Vec<JsArchiveEntry>, ExtractError> {
    let mut archive = section_tar(reader, archive_type, section)?;
    let mut entries = Vec::new();
    for entry in archive.entries()? {
        let entry = entry?;
        let path = normalize(&entry.path()?)?;
        if Section::containing(&path) != section {
            continue;
        }
        let entry_type = entry.header().entry_type();
        entries.push(JsArchiveEntry {
            path: path.to_string_lossy().into_owned(),
            size: entry.header().size()?,
            kind: if entry_type.is_file() {
                "file"
            } else if entry_type.is_dir() {
                "directory"
            } else if entry_type.is_symlink() {
                "symlink"
            } else if entry_type.is_hard_link() {
                "hardlink"
            } else {
                "other"
            },
            link_target: entry
                .link_name()?
                .map(|target| target.to_string_lossy().into_owned()),
        });
    }
    Ok(entries)
}

fn read_entry(
    reader: SparseReader,
    archive_type: CondaArchiveType,
    path: &Path,
) -> Result<Option<Vec<u8>>, ExtractError> {
    let mut archive = section_tar(reader, archive_type, Section::containing(path))?;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if normalize(&entry.path()?)? != path {
            continue;
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
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

fn to_js<T: Serialize>(value: &T) -> JsResult<JsValue> {
    let serializer = serde_wasm_bindgen::Serializer::json_compatible();
    Ok(value.serialize(&serializer)?)
}

/// A conda package archive on a server, opened once and read many times with
/// HTTP range requests.
///
/// Opening a `.conda` archive fetches its last 64 KiB, which hold the ZIP
/// directory and usually the whole `info` section, so the metadata files cost
/// no further request. The package payload is fetched with one more request
/// the first time a file outside `info/` is read or listed. A `.tar.bz2`
/// archive is a single compressed stream and is downloaded whole on first
/// use.
///
/// @public
#[wasm_bindgen(js_name = "PackageArchive")]
pub struct JsPackageArchive {
    source: HttpSource,
    archive_type: CondaArchiveType,
    fetched: Rc<RefCell<Fetched>>,
}

impl JsPackageArchive {
    /// Runs `read` over the fetched bytes, fetching what it is missing until
    /// it gets through.
    async fn run<T>(&self, read: impl Fn(SparseReader) -> Result<T, ExtractError>) -> JsResult<T> {
        loop {
            let reader = SparseReader::new(self.fetched.clone());
            let miss = reader.miss();
            let result = read(reader);
            let Some(offset) = miss.get() else {
                return Ok(result?);
            };
            let (range, len) = {
                let fetched = self.fetched.borrow();
                (fetched.missing_range(offset), fetched.len())
            };
            let (start, bytes) = self.source.fetch(range, len).await?;
            self.fetched.borrow_mut().insert(start, bytes);
        }
    }

    async fn read_entry(&self, path: &Path) -> JsResult<Option<Vec<u8>>> {
        let path = normalize(path)?;
        let archive_type = self.archive_type;
        self.run(|reader| read_entry(reader, archive_type, &path))
            .await
    }

    async fn package_file<P: PackageFile>(&self) -> JsResult<Option<P>> {
        let Some(bytes) = self.read_entry(P::package_path()).await? else {
            return Ok(None);
        };
        let file = P::from_slice(&bytes).map_err(|err| {
            ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), err)
        })?;
        Ok(Some(file))
    }

    async fn required_package_file<P: PackageFile + Serialize>(&self) -> JsResult<JsValue> {
        let file = self
            .package_file::<P>()
            .await?
            .ok_or(ExtractError::MissingComponent)?;
        to_js(&file)
    }

    async fn optional_package_file<P: PackageFile + Serialize>(&self) -> JsResult<JsValue> {
        match self.package_file::<P>().await? {
            Some(file) => to_js(&file),
            None => Ok(JsValue::UNDEFINED),
        }
    }
}

#[wasm_bindgen(js_class = "PackageArchive")]
impl JsPackageArchive {
    /// Opens the archive at `url`. The archive type follows from the URL's
    /// extension (`.conda` or `.tar.bz2`).
    ///
    /// Pass `size`, the archive's size in bytes as the repodata record states
    /// it, to save the `HEAD` request that is otherwise needed to learn it.
    #[wasm_bindgen(js_name = "fromUrl")]
    pub async fn from_url(
        url: String,
        #[wasm_bindgen(param_description = "The size of the archive in bytes, if known")]
        size: Option<f64>,
    ) -> JsResult<JsPackageArchive> {
        let url = Url::parse(&url)?;
        let archive_type = CondaArchiveType::try_from(Path::new(url.path()))
            .ok_or(ExtractError::UnsupportedArchiveType)?;
        let source = HttpSource {
            client: reqwest::Client::new(),
            url,
        };
        let size = size
            .filter(|size| size.is_finite() && *size >= 0.0)
            .map(|size| size as u64);
        let (len, whole) = match size {
            Some(size) => (size, None),
            None => source.len().await?,
        };
        let mut fetched = Fetched::new(len);
        match whole {
            Some(whole) => fetched.insert(0, whole),
            None if archive_type == CondaArchiveType::Conda && len > 0 => {
                let (start, bytes) = source
                    .fetch(len.saturating_sub(TAIL_SIZE)..len, len)
                    .await?;
                fetched.insert(start, bytes);
            }
            None => {}
        }
        Ok(Self {
            source,
            archive_type,
            fetched: Rc::new(RefCell::new(fetched)),
        })
    }

    /// The URL the archive was opened from.
    #[wasm_bindgen(getter)]
    pub fn url(&self) -> String {
        self.source.url.to_string()
    }

    /// The size of the whole archive in bytes.
    #[wasm_bindgen(getter)]
    pub fn size(&self) -> f64 {
        self.fetched.borrow().len() as f64
    }

    /// The archive format: `"conda"` or `"tar.bz2"`.
    #[wasm_bindgen(
        getter,
        js_name = "archiveType",
        unchecked_return_type = "\"conda\" | \"tar.bz2\""
    )]
    pub fn archive_type(&self) -> String {
        match self.archive_type {
            CondaArchiveType::Conda => "conda".to_owned(),
            CondaArchiveType::TarBz2 => "tar.bz2".to_owned(),
        }
    }

    /// The number of bytes that still have to be fetched to read a section,
    /// or `undefined` when the archive has no such section. Zero once the
    /// section has been read, and usually for the `info` section of a
    /// `.conda` archive right after opening it. A `.tar.bz2` archive has to
    /// be fetched whole for either section.
    #[wasm_bindgen(js_name = "bytesToFetch")]
    pub fn bytes_to_fetch(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ArchiveSection")] section: String,
    ) -> JsResult<Option<f64>> {
        let section = Section::parse(&section)?;
        if self.archive_type == CondaArchiveType::TarBz2 {
            return Ok(Some(self.fetched.borrow().missing_bytes() as f64));
        }
        // Opening the section reads the ZIP directory and the member's
        // header, which fails at the first missing byte.
        let reader = SparseReader::new(self.fetched.clone());
        let miss = reader.miss();
        let result = section_tar(reader, self.archive_type, section).map(drop);
        Ok(match (miss.get(), result) {
            (Some(offset), _) => {
                let range = self.fetched.borrow().missing_range(offset);
                Some((range.end - range.start) as f64)
            }
            (None, Ok(())) => Some(0.0),
            (None, Err(ExtractError::MissingComponent)) => None,
            (None, Err(err)) => return Err(err.into()),
        })
    }

    /// Lists the files of a section in archive order: `"info"` for the
    /// metadata under `info/`, `"pkg"` for the package payload.
    #[wasm_bindgen(js_name = "listFiles", unchecked_return_type = "ArchiveEntry[]")]
    pub async fn list_files(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ArchiveSection")] section: String,
    ) -> JsResult<JsValue> {
        let section = Section::parse(&section)?;
        let archive_type = self.archive_type;
        let entries = self
            .run(|reader| list_entries(reader, archive_type, section))
            .await?;
        to_js(&entries)
    }

    /// Reads one file of the package, or `undefined` when there is no such
    /// file. Symbolic and hard links cannot be read; they are listed with
    /// their target by {@link PackageArchive.listFiles}.
    #[wasm_bindgen(js_name = "readFile")]
    pub async fn read_file(&self, path: String) -> JsResult<Option<js_sys::Uint8Array>> {
        let contents = self.read_entry(Path::new(&path)).await?;
        Ok(contents.map(|bytes| js_sys::Uint8Array::from(bytes.as_slice())))
    }

    /// The parsed `info/index.json`, which every package has.
    #[wasm_bindgen(js_name = "indexJson", unchecked_return_type = "IndexJson")]
    pub async fn index_json(&self) -> JsResult<JsValue> {
        self.required_package_file::<IndexJson>().await
    }

    /// The parsed `info/about.json`, or `undefined` when the package has none.
    #[wasm_bindgen(js_name = "aboutJson", unchecked_return_type = "AboutJson | undefined")]
    pub async fn about_json(&self) -> JsResult<JsValue> {
        self.optional_package_file::<AboutJson>().await
    }

    /// The parsed `info/paths.json`, which lists every file of the payload.
    #[wasm_bindgen(js_name = "pathsJson", unchecked_return_type = "PathsJson")]
    pub async fn paths_json(&self) -> JsResult<JsValue> {
        self.required_package_file::<PathsJson>().await
    }

    /// The parsed `info/run_exports.json`, or `undefined` when the package
    /// has none.
    #[wasm_bindgen(
        js_name = "runExportsJson",
        unchecked_return_type = "RunExportsJson | undefined"
    )]
    pub async fn run_exports_json(&self) -> JsResult<JsValue> {
        self.optional_package_file::<RunExportsJson>().await
    }
}

#[wasm_bindgen(typescript_custom_section)]
const PACKAGE_ARCHIVE_D_TS: &'static str = include_str!("package_archive.d.ts");
