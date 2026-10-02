//! A conda package archive read over HTTP with range requests, from the
//! browser.
//!
//! The reader is [`rattler_package_streaming::range::RangeArchive`]; this
//! module supplies the byte source, which uses `fetch` through reqwest, and
//! the JavaScript class around it.

use std::{cell::RefCell, path::Path, rc::Rc};

use bytes::Bytes;
use rattler_conda_types::package::{
    AboutJson, CondaArchiveType, IndexJson, PackageFile, PathsJson, RunExportsJson,
};
use rattler_package_streaming::{
    ExtractError,
    range::{ArchiveEntry, ArchiveEntryKind, RangeArchive, RangeSource, Section},
};
use reqwest::{StatusCode, header::RANGE};
use serde::Serialize;
use url::Url;
use wasm_bindgen::prelude::*;

use crate::{JsError, JsResult};

/// Reads an archive over HTTP with `Range` requests.
///
/// Browsers do not expose the `Content-Range` header of a cross-origin
/// response, so the archive's size is taken from the caller when known (the
/// repodata record carries it) and from a `HEAD` request otherwise. A server
/// that ignores `Range` and answers `200 OK` with the whole archive is
/// accepted as well; the body is kept and later reads are served from it.
struct HttpRangeSource {
    client: reqwest::Client,
    url: Url,
    known_size: Option<u64>,
    /// The whole archive, when a server made us download it.
    whole: RefCell<Option<Bytes>>,
}

impl HttpRangeSource {
    fn io_error(message: String) -> ExtractError {
        ExtractError::IoError(std::io::Error::other(message))
    }

    async fn download_whole(&self) -> Result<Bytes, ExtractError> {
        let response = self
            .client
            .get(self.url.clone())
            .send()
            .await
            .map_err(|err| Self::io_error(format!("could not download {}: {err}", self.url)))?;
        if !response.status().is_success() {
            return Err(Self::io_error(format!(
                "could not download {}: HTTP {}",
                self.url,
                response.status()
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|err| Self::io_error(format!("could not download {}: {err}", self.url)))?;
        *self.whole.borrow_mut() = Some(bytes.clone());
        Ok(bytes)
    }
}

impl RangeSource for HttpRangeSource {
    async fn len(&self) -> Result<u64, ExtractError> {
        if let Some(size) = self.known_size {
            return Ok(size);
        }
        if let Some(whole) = self.whole.borrow().as_ref() {
            return Ok(whole.len() as u64);
        }
        let response = self
            .client
            .head(self.url.clone())
            .send()
            .await
            .map_err(|err| Self::io_error(format!("could not reach {}: {err}", self.url)))?;
        if !response.status().is_success() {
            return Err(Self::io_error(format!(
                "could not reach {}: HTTP {}",
                self.url,
                response.status()
            )));
        }
        match response.content_length() {
            Some(size) => Ok(size),
            // No usable `Content-Length`: the only way to learn the size is
            // to download the archive.
            None => Ok(self.download_whole().await?.len() as u64),
        }
    }

    async fn read_range(&self, start: u64, end: u64) -> Result<Bytes, ExtractError> {
        // Clone the handle so no borrow is held across the await below.
        let whole = self.whole.borrow().clone();
        if let Some(whole) = whole {
            return whole.read_range(start, end).await;
        }
        if start >= end {
            return Ok(Bytes::new());
        }
        let response = self
            .client
            .get(self.url.clone())
            .header(RANGE, format!("bytes={start}-{}", end - 1))
            .send()
            .await
            .map_err(|err| Self::io_error(format!("could not read {}: {err}", self.url)))?;
        match response.status() {
            StatusCode::PARTIAL_CONTENT => {
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|err| Self::io_error(format!("could not read {}: {err}", self.url)))?;
                if bytes.len() as u64 != end - start {
                    return Err(Self::io_error(format!(
                        "{} answered a request for bytes {start}..{end} with {} bytes",
                        self.url,
                        bytes.len()
                    )));
                }
                Ok(bytes)
            }
            // The server ignored the range and sent everything.
            StatusCode::OK => {
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|err| Self::io_error(format!("could not read {}: {err}", self.url)))?;
                *self.whole.borrow_mut() = Some(bytes.clone());
                bytes.read_range(start, end).await
            }
            status => Err(Self::io_error(format!(
                "could not read bytes {start}..{end} of {}: HTTP {status}",
                self.url
            ))),
        }
    }
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

impl From<ArchiveEntry> for JsArchiveEntry {
    fn from(entry: ArchiveEntry) -> Self {
        Self {
            path: entry.path.to_string_lossy().into_owned(),
            size: entry.size,
            kind: match entry.kind {
                ArchiveEntryKind::File => "file",
                ArchiveEntryKind::Directory => "directory",
                ArchiveEntryKind::Symlink => "symlink",
                ArchiveEntryKind::Hardlink => "hardlink",
                _ => "other",
            },
            link_target: entry
                .link_target
                .map(|target| target.to_string_lossy().into_owned()),
        }
    }
}

fn parse_section(section: &str) -> JsResult<Section> {
    match section {
        "info" => Ok(Section::Info),
        "pkg" => Ok(Section::Content),
        other => Err(JsError::InvalidSection(other.to_owned())),
    }
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
    inner: Rc<RangeArchive<HttpRangeSource>>,
    url: String,
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
        let parsed = Url::parse(&url)?;
        let archive_type = CondaArchiveType::try_from(Path::new(parsed.path()))
            .ok_or(ExtractError::UnsupportedArchiveType)?;
        let source = HttpRangeSource {
            client: reqwest::Client::new(),
            url: parsed,
            known_size: size
                .filter(|size| size.is_finite() && *size >= 0.0)
                .map(|size| size as u64),
            whole: RefCell::new(None),
        };
        let inner = RangeArchive::open(source, archive_type).await?;
        Ok(Self {
            inner: Rc::new(inner),
            url,
        })
    }

    /// The URL the archive was opened from.
    #[wasm_bindgen(getter)]
    pub fn url(&self) -> String {
        self.url.clone()
    }

    /// The size of the whole archive in bytes.
    #[wasm_bindgen(getter)]
    pub fn size(&self) -> f64 {
        self.inner.size() as f64
    }

    /// The archive format: `"conda"` or `"tar.bz2"`.
    #[wasm_bindgen(
        getter,
        js_name = "archiveType",
        unchecked_return_type = "\"conda\" | \"tar.bz2\""
    )]
    pub fn archive_type(&self) -> String {
        match self.inner.archive_type() {
            CondaArchiveType::Conda => "conda".to_owned(),
            CondaArchiveType::TarBz2 => "tar.bz2".to_owned(),
        }
    }

    /// The number of bytes that have to be fetched to read a section, or
    /// `undefined` when the archive has no such section. For a `.conda`
    /// archive this is the size of the section's compressed member; a
    /// `.tar.bz2` archive has to be fetched whole for either section.
    #[wasm_bindgen(js_name = "sectionSize")]
    pub fn section_size(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ArchiveSection")] section: String,
    ) -> JsResult<Option<f64>> {
        Ok(self
            .inner
            .section_size(parse_section(&section)?)
            .map(|size| size as f64))
    }

    /// Lists the files of a section in archive order: `"info"` for the
    /// metadata under `info/`, `"pkg"` for the package payload.
    #[wasm_bindgen(js_name = "listFiles", unchecked_return_type = "ArchiveEntry[]")]
    pub async fn list_files(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ArchiveSection")] section: String,
    ) -> JsResult<JsValue> {
        let entries = self.inner.list_entries(parse_section(&section)?).await?;
        let entries: Vec<JsArchiveEntry> = entries.into_iter().map(Into::into).collect();
        to_js(&entries)
    }

    /// Reads one file of the package, or `undefined` when there is no such
    /// file. Symbolic and hard links cannot be read; they are listed with
    /// their target by {@link PackageArchive.listFiles}.
    #[wasm_bindgen(js_name = "readFile")]
    pub async fn read_file(&self, path: String) -> JsResult<Option<js_sys::Uint8Array>> {
        let contents = self.inner.read_file(Path::new(&path)).await?;
        Ok(contents.map(|bytes| js_sys::Uint8Array::from(bytes.as_slice())))
    }

    /// The parsed `info/index.json`, which every package has.
    #[wasm_bindgen(js_name = "indexJson", unchecked_return_type = "IndexJson")]
    pub async fn index_json(&self) -> JsResult<JsValue> {
        to_js(&self.inner.read_package_file::<IndexJson>().await?)
    }

    /// The parsed `info/about.json`, or `undefined` when the package has none.
    #[wasm_bindgen(js_name = "aboutJson", unchecked_return_type = "AboutJson | undefined")]
    pub async fn about_json(&self) -> JsResult<JsValue> {
        self.optional_package_file::<AboutJson>().await
    }

    /// The parsed `info/paths.json`, which lists every file of the payload.
    #[wasm_bindgen(js_name = "pathsJson", unchecked_return_type = "PathsJson")]
    pub async fn paths_json(&self) -> JsResult<JsValue> {
        to_js(&self.inner.read_package_file::<PathsJson>().await?)
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

    async fn optional_package_file<P: PackageFile + Serialize>(&self) -> JsResult<JsValue> {
        match self.inner.try_read_package_file::<P>().await? {
            Some(file) => to_js(&file),
            None => Ok(JsValue::UNDEFINED),
        }
    }
}

#[wasm_bindgen(typescript_custom_section)]
const PACKAGE_ARCHIVE_D_TS: &'static str = include_str!("package_archive.d.ts");
