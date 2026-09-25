use std::path::{Path, PathBuf};

use futures::StreamExt;
use miette::IntoDiagnostic;
use opendal::{ErrorKind, Operator};
use rattler_digest::{HashingReader, Md5, Sha256};
use rattler_s3::S3CredentialSource;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::bytes::BytesMut;
use url::Url;

use crate::upload::package::ExtractedPackage;

/// Size of a single multipart chunk. S3 requires every part except the last to
/// be at least 5 MiB; we use a larger value to keep the number of parts low.
const DESIRED_CHUNK_SIZE: usize = 1024 * 1024 * 10;

/// Number of chunks of a single package that are uploaded concurrently.
const PART_CONCURRENCY: usize = 4;

/// Number of packages that are uploaded concurrently.
const PACKAGE_CONCURRENCY: usize = 4;

/// Uploads a package to a channel in an S3 bucket.
///
/// The credential source must already be determined by the caller (e.g. via
/// [`S3CredentialSource::resolve`] or [`S3CredentialSource::from_sdk`]).
pub async fn upload_package_to_s3(
    channel: Url,
    credentials: S3CredentialSource,
    package_files: &[PathBuf],
    force: bool,
) -> miette::Result<()> {
    upload_package_to_s3_with_attestation(channel, credentials, package_files, None, force).await
}

/// Uploads packages and an optional attestation sidecar to a channel in an S3
/// bucket.
///
/// The attestation must be the complete sidecar: a non-empty JSON array of
/// Sigstore bundles. It can only be supplied when uploading exactly one
/// package. The immutable content-addressed sidecar is published before the
/// mutable discovery sidecar.
pub async fn upload_package_to_s3_with_attestation(
    channel: Url,
    credentials: S3CredentialSource,
    package_files: &[PathBuf],
    attestation: Option<&Path>,
    force: bool,
) -> miette::Result<()> {
    if attestation.is_some() && package_files.len() != 1 {
        miette::bail!("an attestation can only be uploaded with exactly one package");
    }

    // Validate and read the sidecar before changing any remote state.
    let attestation = match attestation {
        Some(path) => {
            let bytes = fs_err::tokio::read(path).await.into_diagnostic()?;
            validate_attestation_sidecar(&bytes)?;
            Some(bytes)
        }
        None => None,
    };

    let bucket = channel
        .host_str()
        .ok_or(miette::miette!("No bucket in S3 URL"))?;

    let builder = credentials.opendal_builder(bucket, channel.path());
    let op = Operator::new(builder).into_diagnostic()?.finish();

    // Upload multiple packages concurrently. Each individual package upload also
    // streams its chunks concurrently (see `upload_single_package`).
    futures::stream::iter(package_files.iter())
        .map(|package_file| {
            let op = op.clone();
            let channel = &channel;
            async move { upload_single_package(&op, channel, bucket, package_file, force).await }
        })
        .buffer_unordered(PACKAGE_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<miette::Result<Vec<_>>>()?;

    if let Some(attestation) = attestation {
        let package = ExtractedPackage::from_package_file(&package_files[0])?;
        let subdir = package
            .subdir()
            .ok_or_else(|| miette::miette!("Failed to get subdir"))?;
        let filename = package
            .filename()
            .ok_or_else(|| miette::miette!("Failed to get filename"))?;
        let mutable_key = format!("{subdir}/{filename}.sigs");

        publish_attestation_sidecar(&op, &mutable_key, attestation).await?;
    }

    Ok(())
}

/// Checks the basic container format required for a conda attestation sidecar.
fn validate_attestation_sidecar(bytes: &[u8]) -> miette::Result<()> {
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(serde_json::Value::Array(bundles)) if !bundles.is_empty() => Ok(()),
        Ok(_) => {
            miette::bail!("attestation sidecar must be a non-empty JSON array of Sigstore bundles")
        }
        Err(err) => Err(miette::miette!(
            "attestation sidecar is not valid JSON: {err}"
        )),
    }
}

/// Publishes the immutable sidecar before updating its mutable discovery path.
async fn publish_attestation_sidecar(
    op: &Operator,
    mutable_key: &str,
    bytes: Vec<u8>,
) -> miette::Result<()> {
    let sha256 = rattler_digest::compute_bytes_digest::<Sha256>(&bytes);
    let immutable_key = format!("{mutable_key}.{}", hex::encode(sha256));

    match op
        .write_with(&immutable_key, bytes.clone())
        .content_type("application/json")
        .if_not_exists(true)
        .await
    {
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::ConditionNotMatch => {
            let existing = op.read(&immutable_key).await.into_diagnostic()?.to_bytes();
            if existing.as_ref() != bytes.as_slice() {
                miette::bail!(
                    "content-addressed attestation sidecar {immutable_key} already exists with different contents"
                );
            }
        }
        Err(err) => return Err(err).into_diagnostic(),
    }

    // Copying from the immutable object guarantees that both published paths
    // contain exactly the same bytes.
    op.copy(&immutable_key, mutable_key)
        .await
        .into_diagnostic()?;

    tracing::info!("Uploaded attestation sidecar to {mutable_key}");
    Ok(())
}

/// Uploads a single package file to the S3 bucket via the given operator.
async fn upload_single_package(
    op: &Operator,
    channel: &Url,
    bucket: &str,
    package_file: &Path,
    force: bool,
) -> miette::Result<()> {
    let package = ExtractedPackage::from_package_file(package_file)?;
    let subdir = package
        .subdir()
        .ok_or_else(|| miette::miette!("Failed to get subdir"))?;
    let filename = package
        .filename()
        .ok_or_else(|| miette::miette!("Failed to get filename"))?;
    let key = format!("{subdir}/{filename}");

    // Compute the hash of the package by streaming its content.
    let file = tokio::io::BufReader::new(
        fs_err::tokio::File::open(package_file)
            .await
            .into_diagnostic()?,
    );
    let sha256_reader = HashingReader::<_, Sha256>::new(file);
    let mut md5_reader = HashingReader::<_, Md5>::new(sha256_reader);
    let size = tokio::io::copy(&mut md5_reader, &mut tokio::io::sink())
        .await
        .into_diagnostic()?;
    let (sha256_reader, md5hash) = md5_reader.finalize();
    let (mut file, sha256hash) = sha256_reader.finalize();

    // Rewind the file to the beginning.
    file.rewind().await.into_diagnostic()?;

    // Construct a writer for the package. Setting `chunk` and `concurrent`
    // enables opendal's concurrent multipart upload: data is buffered into
    // `DESIRED_CHUNK_SIZE` parts and up to `PART_CONCURRENCY` parts are uploaded
    // in parallel.
    let mut writer = match op
        .writer_with(&key)
        .chunk(DESIRED_CHUNK_SIZE)
        .concurrent(PART_CONCURRENCY)
        .content_disposition(&format!("attachment; filename={filename}"))
        .if_not_exists(!force)
        .user_metadata([
            (String::from("package-sha256"), hex::encode(sha256hash)),
            (String::from("package-md5"), hex::encode(md5hash)),
        ])
        .await
    {
        Err(e) if e.kind() == ErrorKind::ConditionNotMatch => {
            miette::bail!(
                "Package s3://{bucket}{}/{key} already exists. Use --force to overwrite.",
                channel.path().to_string()
            );
        }
        Ok(writer) => writer,
        Err(e) => {
            return Err(e).into_diagnostic();
        }
    };

    // Stream the file to the writer in `DESIRED_CHUNK_SIZE` chunks. opendal takes
    // care of buffering these into correctly sized parts and uploading them
    // concurrently.
    let mut remaining_size = size as usize;
    while remaining_size > 0 {
        // Allocate memory for this chunk.
        let chunk_size = remaining_size.min(DESIRED_CHUNK_SIZE);
        let mut chunk = BytesMut::with_capacity(chunk_size);
        // SAFE: because we do not care about the bytes that are currently in the buffer
        unsafe { chunk.set_len(chunk_size) };

        // Fill the chunk with data. This reads exactly the number of bytes we want. No
        // more, no less.
        let bytes_read = file.read_exact(&mut chunk[..]).await.into_diagnostic()?;
        debug_assert_eq!(bytes_read, chunk.len());

        // Hand the chunk to the writer. With concurrent writes enabled this returns
        // as soon as the chunk is queued rather than fully uploaded.
        writer.write(chunk.freeze()).await.into_diagnostic()?;

        // Update the number of remaining bytes.
        remaining_size = remaining_size.saturating_sub(bytes_read);
    }

    match writer.close().await {
        Err(e) if e.kind() == ErrorKind::ConditionNotMatch => {
            miette::bail!(
                "Package s3://{bucket}{}/{key} already exists. Use --force to overwrite.",
                channel.path().to_string()
            );
        }
        Ok(_) => {
            tracing::info!(
                "Uploaded package to s3://{bucket}{}/{key}",
                channel.path().to_string()
            );
        }
        Err(e) => {
            return Err(e).into_diagnostic();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use opendal::services::Fs;

    use super::{publish_attestation_sidecar, validate_attestation_sidecar};

    #[test]
    fn validates_attestation_sidecar_container() {
        assert!(validate_attestation_sidecar(br#"[{"bundle":1}]"#).is_ok());
        assert!(validate_attestation_sidecar(br#"[]"#).is_err());
        assert!(validate_attestation_sidecar(br#"{"bundle":1}"#).is_err());
        assert!(validate_attestation_sidecar(b"not json").is_err());
    }

    #[tokio::test]
    async fn publishes_immutable_and_mutable_sidecars_with_identical_bytes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let op = opendal::Operator::new(Fs::default().root(temp_dir.path().to_str().unwrap()))
            .unwrap()
            .finish();
        let mutable_key = "noarch/test-1.0-0.conda.sigs";
        let bytes = br#"[{"bundle":1}]"#.to_vec();
        let hash =
            hex::encode(rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(&bytes));
        let immutable_key = format!("{mutable_key}.{hash}");

        publish_attestation_sidecar(&op, mutable_key, bytes.clone())
            .await
            .unwrap();

        assert_eq!(op.read(&immutable_key).await.unwrap().to_bytes(), bytes);
        assert_eq!(
            op.read(mutable_key).await.unwrap().to_bytes(),
            op.read(&immutable_key).await.unwrap().to_bytes()
        );

        // Publishing identical content is idempotent.
        publish_attestation_sidecar(&op, mutable_key, bytes)
            .await
            .unwrap();
    }
}
