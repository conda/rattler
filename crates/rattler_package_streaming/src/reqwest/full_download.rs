//! Streaming full-download helpers for remote Conda packages.
//!
//! These helpers download the full response body and extract the requested file
//! while streaming, without first writing the archive to disk.
//!
//! They are primarily used as a fallback for the higher-level APIs in
//! [`super::fetch`] when sparse range-request access is unavailable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_compression::tokio::bufread::{BzDecoder, ZstdDecoder};
use async_zip::base::read::stream::ZipFileReader;
use futures_util::stream::TryStreamExt;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use reqwest_middleware::ClientWithMiddleware;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tokio_util::io::StreamReader;
use url::Url;

use crate::tokio::async_read::{conda_entry_prefix, get_files_from_tar_archive};
use crate::ExtractError;

/// Stream the full package response and extract a single file by path.
///
/// Returns `Ok(None)` when the target path does not exist in the archive.
pub async fn fetch_file_from_remote_full_download(
    client: &ClientWithMiddleware,
    url: &Url,
    target_path: &Path,
) -> Result<Option<Vec<u8>>, ExtractError> {
    match fetch_files_from_remote_full_download(client, url, [target_path.to_path_buf()]).await {
        Ok(mut files) => Ok(files.pop().map(|(_, bytes)| bytes)),
        Err(ExtractError::MissingPaths { .. }) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Stream the full package response and extract multiple files by path.
///
/// The package archive is scanned once and the returned vector preserves the
/// first-seen order of `target_paths`.
pub async fn fetch_files_from_remote_full_download<I>(
    client: &ClientWithMiddleware,
    url: &Url,
    target_paths: I,
) -> Result<Vec<(PathBuf, Vec<u8>)>, ExtractError>
where
    I: IntoIterator<Item = PathBuf>,
{
    let archive_type = CondaArchiveType::try_from(std::path::Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?;

    let target_paths: Vec<PathBuf> = target_paths.into_iter().collect();
    if target_paths.is_empty() {
        return Ok(Vec::new());
    }

    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(ExtractError::ReqwestError)?
        .error_for_status()
        .map_err(|e| ExtractError::ReqwestError(e.into()))?;

    // Convert the response body into an AsyncRead stream (same pattern as reqwest/tokio.rs)
    let byte_stream = response.bytes_stream().map_err(|err| {
        if err.is_body() {
            std::io::Error::new(std::io::ErrorKind::Interrupted, err)
        } else if err.is_decode() {
            std::io::Error::new(std::io::ErrorKind::InvalidData, err)
        } else {
            std::io::Error::other(err)
        }
    });
    let stream_reader = StreamReader::new(byte_stream);

    let mut found_files = match archive_type {
        CondaArchiveType::TarBz2 => {
            let buf_reader = tokio::io::BufReader::new(stream_reader);
            let decoder = BzDecoder::new(buf_reader);
            let mut archive = tokio_tar::Archive::new(decoder);
            get_files_from_tar_archive(&mut archive, &target_paths)
                .await?
                .into_iter()
                .collect::<HashMap<_, _>>()
        }
        CondaArchiveType::Conda => {
            // async_zip uses futures IO traits, so bridge tokio → futures
            let compat_reader = stream_reader.compat();
            let mut buf_reader = futures::io::BufReader::new(compat_reader);
            let mut zip_reader = ZipFileReader::new(&mut buf_reader);

            let mut grouped_paths: HashMap<&'static str, Vec<PathBuf>> = HashMap::new();
            for path in &target_paths {
                grouped_paths
                    .entry(conda_entry_prefix(path))
                    .or_default()
                    .push(path.clone());
            }

            let mut found = HashMap::with_capacity(target_paths.len());
            let mut remaining = target_paths
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>();

            while let Some(mut entry) = zip_reader
                .next_with_entry()
                .await
                .map_err(|e| ExtractError::IoError(std::io::Error::other(e)))?
            {
                let filename = entry.reader().entry().filename().as_str().map_err(|e| {
                    ExtractError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                })?;

                let component_paths =
                    if filename.starts_with("info-") && filename.ends_with(".tar.zst") {
                        grouped_paths.get("info-")
                    } else if filename.starts_with("pkg-") && filename.ends_with(".tar.zst") {
                        grouped_paths.get("pkg-")
                    } else {
                        None
                    };

                if let Some(component_paths) = component_paths {
                    // Bridge the entry reader back from futures → tokio
                    let compat_entry = entry.reader_mut().compat();
                    let buf_entry = tokio::io::BufReader::new(compat_entry);
                    let decoder = ZstdDecoder::new(buf_entry);
                    let mut archive = tokio_tar::Archive::new(decoder);
                    for (path, bytes) in
                        get_files_from_tar_archive(&mut archive, component_paths).await?
                    {
                        remaining.remove(&path);
                        found.insert(path, bytes);
                    }

                    if remaining.is_empty() {
                        break;
                    }
                }

                // Skip to the next entry (required by async_zip API)
                (.., zip_reader) = entry
                    .skip()
                    .await
                    .map_err(|e| ExtractError::IoError(std::io::Error::other(e)))?;
            }

            found
        }
    };

    let missing_paths = target_paths
        .iter()
        .filter(|path| !found_files.contains_key(*path))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_paths.is_empty() {
        return Err(ExtractError::MissingPaths {
            paths: missing_paths,
        });
    }

    Ok(target_paths
        .into_iter()
        .filter_map(|path| found_files.remove(&path).map(|bytes| (path, bytes)))
        .collect())
}

/// Stream the full package response and extract a single typed [`PackageFile`].
///
/// This is a typed wrapper around [`fetch_file_from_remote_full_download`].
pub async fn fetch_package_file_full_download<P: PackageFile>(
    client: &ClientWithMiddleware,
    url: &Url,
) -> Result<P, ExtractError> {
    let content =
        fetch_files_from_remote_full_download(client, url, [P::package_path().to_path_buf()])
            .await?
            .into_iter()
            .next()
            .map(|(_, bytes)| bytes)
            .ok_or(ExtractError::MissingComponent)?;
    P::from_slice(&content)
        .map_err(|e| ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e))
}
