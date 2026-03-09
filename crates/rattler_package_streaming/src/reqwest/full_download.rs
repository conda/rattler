use std::path::Path;

use async_compression::tokio::bufread::{BzDecoder, ZstdDecoder};
use async_zip::base::read::stream::ZipFileReader;
use futures_util::stream::TryStreamExt;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use reqwest_middleware::ClientWithMiddleware;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tokio_util::io::StreamReader;
use url::Url;

use crate::tokio::async_read::get_file_from_tar_archive;
use crate::ExtractError;

/// Stream the full package and extract a single file.
pub async fn fetch_file_from_remote_full_download(
    client: &ClientWithMiddleware,
    url: &Url,
    target_path: &Path,
) -> Result<Option<Vec<u8>>, ExtractError> {
    let archive_type = CondaArchiveType::try_from(std::path::Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?;

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

    let content = match archive_type {
        CondaArchiveType::TarBz2 => {
            let buf_reader = tokio::io::BufReader::new(stream_reader);
            let decoder = BzDecoder::new(buf_reader);
            let mut archive = tokio_tar::Archive::new(decoder);
            get_file_from_tar_archive(&mut archive, target_path).await?
        }
        CondaArchiveType::Conda => {
            // async_zip uses futures IO traits, so bridge tokio → futures
            let compat_reader = stream_reader.compat();
            let mut buf_reader = futures::io::BufReader::new(compat_reader);
            let mut zip_reader = ZipFileReader::new(&mut buf_reader);

            let mut found: Option<Vec<u8>> = None;
            let prefix = crate::tokio::async_read::conda_entry_prefix(target_path);

            while let Some(mut entry) = zip_reader
                .next_with_entry()
                .await
                .map_err(|e| ExtractError::IoError(std::io::Error::other(e)))?
            {
                let filename = entry.reader().entry().filename().as_str().map_err(|e| {
                    ExtractError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                })?;

                if filename.starts_with(prefix) && filename.ends_with(".tar.zst") {
                    // Bridge the entry reader back from futures → tokio
                    let compat_entry = entry.reader_mut().compat();
                    let buf_entry = tokio::io::BufReader::new(compat_entry);
                    let decoder = ZstdDecoder::new(buf_entry);
                    let mut archive = tokio_tar::Archive::new(decoder);
                    found = get_file_from_tar_archive(&mut archive, target_path).await?;
                    break;
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
    Ok(content)
}

/// Stream the full package and extract a single [`PackageFile`].
pub async fn fetch_package_file_full_download<P: PackageFile>(
    client: &ClientWithMiddleware,
    url: &Url,
) -> Result<P, ExtractError> {
    let content = fetch_file_from_remote_full_download(client, url, P::package_path())
        .await?
        .ok_or(ExtractError::MissingComponent)?;
    P::from_slice(&content)
        .map_err(|e| ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e))
}
