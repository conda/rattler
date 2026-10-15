//! Streaming full-download helpers for remote Conda packages.
//!
//! These helpers download the full response body and extract the requested file
//! while streaming, without first writing the archive to disk.
//!
//! They are primarily used as a fallback for the higher-level APIs in
//! [`super::fetch`] when sparse range-request access is unavailable.

use std::path::Path;

use async_compression::tokio::bufread::{BzDecoder, ZstdDecoder};
use async_zip::base::read::stream::ZipFileReader;
use futures_util::stream::TryStreamExt;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use reqwest_middleware::ClientWithMiddleware;
use tar_codec::{
    Archive as _, DecodePolicy, Member, MemberPayload as _, PaxDecodePolicy,
    PaxVendorExtensionPolicy, TarArchive,
};
use tokio::io::AsyncRead;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tokio_util::io::StreamReader;
use url::Url;

use crate::ExtractError;
use crate::section::{MAX_PREALLOC, Section, normalize};

/// Size of the chunks requested from a tar member payload.
const PAYLOAD_CHUNK_SIZE: usize = 64 * 1024;

/// Stream the full package response and extract a single file by path.
///
/// Returns `Ok(None)` when the target path does not exist in the archive.
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
            get_file_from_tar_archive(decoder, target_path).await?
        }
        CondaArchiveType::Conda => {
            // async_zip uses futures IO traits, so bridge tokio → futures
            let compat_reader = stream_reader.compat();
            let mut buf_reader = futures::io::BufReader::new(compat_reader);
            let mut zip_reader = ZipFileReader::new(&mut buf_reader);

            let mut found: Option<Vec<u8>> = None;
            let prefix = Section::containing(target_path).zip_prefix();

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
                    found = get_file_from_tar_archive(decoder, target_path).await?;
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

/// Stream the full package response and extract a single typed [`PackageFile`].
///
/// This is a typed wrapper around [`fetch_file_from_remote_full_download`].
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

/// The tar decode policy used for conda package sections.
///
/// Conda packages are produced by many different tar writers, so this accepts
/// unknown vendor pax records (e.g. `SCHILY.*` and `LIBARCHIVE.*` written by
/// bsdtar) and unterminated `uname`/`gname` header fields.
fn conda_decode_policy() -> DecodePolicy {
    DecodePolicy::default()
        .allow_header_uname_and_gname_without_terminator(true)
        .pax_policy(
            PaxDecodePolicy::default()
                .vendor_extension_policy(PaxVendorExtensionPolicy::AllowUnknown),
        )
}

fn decode_error(err: tar_codec::DecodeError) -> ExtractError {
    ExtractError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, err))
}

/// Async equivalent of [`crate::seek::get_file_from_archive`].
///
/// Iterates the members of a tar stream, returning the contents of the first
/// regular file whose path matches `file_name`. Because the reader is
/// streaming, only the bytes up to (and including) the target member are
/// consumed.
async fn get_file_from_tar_archive<R: AsyncRead + Unpin>(
    reader: R,
    file_name: &Path,
) -> Result<Option<Vec<u8>>, ExtractError> {
    let target = normalize(file_name)?;
    let mut members = TarArchive::new(reader)
        .with_policy(conda_decode_policy())
        .members();
    while let Some(member) = members.next().await.map_err(decode_error)? {
        // Normalized comparison, matching the sparse path in `crate::archive`.
        if normalize(Path::new(&member.metadata().path))? != target {
            continue;
        }
        match member {
            Member::File {
                size, mut payload, ..
            } => {
                let mut contents = Vec::with_capacity(size.min(MAX_PREALLOC) as usize);
                let mut chunk = Vec::new();
                while payload
                    .next_chunk(&mut chunk, PAYLOAD_CHUNK_SIZE)
                    .await
                    .map_err(decode_error)?
                {
                    contents.extend_from_slice(&chunk);
                }
                return Ok(Some(contents));
            }
            Member::SymbolicLink {
                target: link_target,
                ..
            }
            | Member::HardLink {
                target: link_target,
                ..
            } => {
                return Err(ExtractError::LinksNotFollowed(vec![format!(
                    "'{}' (links to '{link_target}')",
                    target.display()
                )]));
            }
            Member::Directory { .. } | Member::Special { .. } => {}
        }
    }
    Ok(None)
}
