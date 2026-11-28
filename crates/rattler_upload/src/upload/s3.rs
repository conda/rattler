use std::path::{Path, PathBuf};

use futures::StreamExt;
use miette::IntoDiagnostic;
use opendal::{ErrorKind, Operator};
use rattler_digest::{HashingReader, Md5, Sha256};
use rattler_s3::S3CredentialSource;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::bytes::BytesMut;
use url::Url;

use super::opt::{ForceOverwrite, S3Data, SkipExisting};
use crate::upload::package::ExtractedPackage;

/// Size of a single multipart chunk. S3 requires every part except the last to
/// be at least 5 MiB; we use a larger value to keep the number of parts low.
const DESIRED_CHUNK_SIZE: usize = 1024 * 1024 * 10;

/// Number of chunks of a single package that are uploaded concurrently.
const PART_CONCURRENCY: usize = 4;

/// Number of packages that are uploaded concurrently.
const PACKAGE_CONCURRENCY: usize = 4;

/// Status of a package upload operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageUploadStatus {
    /// The package was successfully uploaded.
    Uploaded,
    /// The package already exists and was skipped.
    Skipped,
}

/// Helper to handle `ConditionNotMatch` when uploading a package.
fn handle_condition_mismatch(
    bucket: &str,
    channel: &Url,
    key: &str,
    skip_existing: SkipExisting,
) -> miette::Result<PackageUploadStatus> {
    if skip_existing.is_enabled() {
        tracing::info!(
            "Skip existing package: s3://{bucket}{}/{key}",
            channel.path().to_string()
        );
        Ok(PackageUploadStatus::Skipped)
    } else {
        miette::bail!(
            "Package s3://{bucket}{}/{key} already exists. Use --force to overwrite or --skip-existing to skip.",
            channel.path().to_string()
        );
    }
}

/// Uploads packages to a channel in an S3 bucket.
///
/// An optional attestation sidecar can be configured in [`S3Data`]. The attestation
/// must be the complete sidecar: a non-empty JSON array of Sigstore bundles. It can
/// only be supplied when uploading exactly one package. The immutable content-addressed
/// sidecar is published before the mutable discovery sidecar, and sidecars are only
/// published on an actual package upload (never when skipped).
pub async fn upload_package_to_s3(
    credentials: S3CredentialSource,
    package_files: &[PathBuf],
    s3_data: S3Data,
) -> miette::Result<()> {
    let bucket = s3_data
        .channel
        .host_str()
        .ok_or_else(|| miette::miette!("No bucket in S3 URL"))?
        .to_string();

    let op = credentials
        .opendal_operator(&bucket, s3_data.channel.path())
        .into_diagnostic()?;

    upload_package_to_s3_inner(&op, &bucket, package_files, s3_data).await
}

/// Inner upload logic shared by `upload_package_to_s3` and unit tests.
async fn upload_package_to_s3_inner(
    op: &Operator,
    bucket: &str,
    package_files: &[PathBuf],
    s3_data: S3Data,
) -> miette::Result<()> {
    if s3_data.attestation.is_some() && package_files.len() != 1 {
        miette::bail!("an attestation can only be uploaded with exactly one package");
    }

    // Validate and read the sidecar before changing any remote state.
    let attestation = match s3_data.attestation.as_deref() {
        Some(path) => {
            let bytes = fs_err::tokio::read(path).await.into_diagnostic()?;
            validate_attestation_sidecar(&bytes)?;
            Some(bytes)
        }
        None => None,
    };

    // Upload multiple packages concurrently. Each individual package upload also
    // streams its chunks concurrently (see `upload_single_package`).
    let upload_statuses = futures::stream::iter(package_files.iter())
        .map(|package_file| {
            let op = op.clone();
            let channel = &s3_data.channel;
            let force = s3_data.force;
            let skip_existing = s3_data.skip_existing;
            async move {
                upload_single_package(&op, channel, bucket, package_file, force, skip_existing)
                    .await
            }
        })
        .buffer_unordered(PACKAGE_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<miette::Result<Vec<_>>>()?;

    // Only publish the attestation sidecar if the package was actually uploaded.
    if let (Some(attestation), Some(&PackageUploadStatus::Uploaded)) =
        (attestation, upload_statuses.first())
    {
        let package = ExtractedPackage::from_package_file(&package_files[0])?;
        let subdir = package
            .subdir()
            .ok_or_else(|| miette::miette!("Failed to get subdir"))?;
        let filename = package
            .filename()
            .ok_or_else(|| miette::miette!("Failed to get filename"))?;
        let mutable_key = format!("{subdir}/{filename}.sigs");

        publish_attestation_sidecar(op, &mutable_key, attestation).await?;
    }

    Ok(())
}

/// Checks the basic container format required for a conda attestation sidecar.
fn validate_attestation_sidecar(bytes: &[u8]) -> miette::Result<()> {
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(serde_json::Value::Array(bundles)) if !bundles.is_empty() => Ok(()),
        Ok(_) => Err(miette::miette!(
            "attestation sidecar must be a non-empty JSON array of Sigstore bundles"
        )),
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
    force: ForceOverwrite,
    skip_existing: SkipExisting,
) -> miette::Result<PackageUploadStatus> {
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
        .if_not_exists(!force.is_enabled())
        .user_metadata([
            (String::from("package-sha256"), hex::encode(sha256hash)),
            (String::from("package-md5"), hex::encode(md5hash)),
        ])
        .await
    {
        Err(e) if e.kind() == ErrorKind::ConditionNotMatch => {
            return handle_condition_mismatch(bucket, channel, &key, skip_existing);
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
            handle_condition_mismatch(bucket, channel, &key, skip_existing)
        }
        Ok(_) => {
            tracing::info!(
                "Uploaded package to s3://{bucket}{}/{key}",
                channel.path().to_string()
            );
            Ok(PackageUploadStatus::Uploaded)
        }
        Err(e) => Err(e).into_diagnostic(),
    }
}

#[cfg(test)]
mod tests {
    use opendal::services::Fs;
    use url::Url;

    use super::{
        PackageUploadStatus, publish_attestation_sidecar, upload_package_to_s3_inner,
        upload_single_package, validate_attestation_sidecar,
    };
    use crate::upload::opt::{ForceOverwrite, S3Data, SkipExisting};
    use crate::upload::package::ExtractedPackage;

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
        let op =
            opendal::Operator::new(Fs::default().root(temp_dir.path().to_str().unwrap())).unwrap();
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

    #[tokio::test]
    async fn test_upload_single_package_skip_existing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let op =
            opendal::Operator::new(Fs::default().root(temp_dir.path().to_str().unwrap())).unwrap();
        let channel = Url::parse("s3://test-bucket/my-channel").unwrap();
        let bucket = "test-bucket";
        let package_path = crate::upload::test_utils::test_package_path();

        // First write succeeds
        let status = upload_single_package(
            &op,
            &channel,
            bucket,
            &package_path,
            ForceOverwrite(false),
            SkipExisting(false),
        )
        .await
        .unwrap();
        assert_eq!(status, PackageUploadStatus::Uploaded);

        // Second write fails without flag
        let err = upload_single_package(
            &op,
            &channel,
            bucket,
            &package_path,
            ForceOverwrite(false),
            SkipExisting(false),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("already exists"));
        assert!(err.to_string().contains("--skip-existing"));

        // Second write succeeds with skip_existing
        let status = upload_single_package(
            &op,
            &channel,
            bucket,
            &package_path,
            ForceOverwrite(false),
            SkipExisting(true),
        )
        .await
        .unwrap();
        assert_eq!(status, PackageUploadStatus::Skipped);
    }

    #[tokio::test]
    async fn test_s3_upload_skip_existing_with_attestation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let op =
            opendal::Operator::new(Fs::default().root(temp_dir.path().to_str().unwrap())).unwrap();
        let channel = Url::parse("s3://test-bucket/my-channel").unwrap();
        let bucket = "test-bucket";
        let package_path = crate::upload::test_utils::test_package_path();

        // Create two different valid attestation files
        let attestation_path1 = temp_dir.path().join("sidecar1.sigs");
        let attestation_path2 = temp_dir.path().join("sidecar2.sigs");
        std::fs::write(&attestation_path1, br#"[{"bundle": 1}]"#).unwrap();
        std::fs::write(&attestation_path2, br#"[{"bundle": 2}]"#).unwrap();

        let package = ExtractedPackage::from_package_file(&package_path).unwrap();
        let subdir = package.subdir().unwrap();
        let filename = package.filename().unwrap();
        let mutable_key = format!("{subdir}/{filename}.sigs");

        // 1. Initial upload with attestation 1 succeeds and publishes sidecar
        let s3_data1 = S3Data::new(
            channel.clone(),
            Some(attestation_path1),
            ForceOverwrite(false),
            SkipExisting(false),
        );
        upload_package_to_s3_inner(&op, bucket, std::slice::from_ref(&package_path), s3_data1)
            .await
            .unwrap();

        assert_eq!(
            op.read(&mutable_key).await.unwrap().to_bytes(),
            br#"[{"bundle": 1}]"#.as_slice()
        );

        // 2. Second upload with skip_existing and attestation 2: package is skipped,
        // and attestation sidecar must NOT be overwritten!
        let s3_data2 = S3Data::new(
            channel,
            Some(attestation_path2),
            ForceOverwrite(false),
            SkipExisting(true),
        );
        upload_package_to_s3_inner(&op, bucket, std::slice::from_ref(&package_path), s3_data2)
            .await
            .unwrap();

        // Verify sidecar still contains bundle 1 (not bundle 2)
        assert_eq!(
            op.read(&mutable_key).await.unwrap().to_bytes(),
            br#"[{"bundle": 1}]"#.as_slice()
        );
    }

    #[test]
    fn test_s3_opts_skip_existing() {
        use crate::upload::opt::S3Opts;
        use clap::Parser;

        let opts_default =
            S3Opts::try_parse_from(["s3", "--channel", "s3://test-bucket/test"]).unwrap();
        assert!(!opts_default.skip_existing);

        let opts_skip = S3Opts::try_parse_from([
            "s3",
            "--channel",
            "s3://test-bucket/test",
            "--skip-existing",
        ])
        .unwrap();
        assert!(opts_skip.skip_existing);

        let opts_skip_alias = S3Opts::try_parse_from([
            "s3",
            "--channel",
            "s3://test-bucket/test",
            "--skip-if-exists",
        ])
        .unwrap();
        assert!(opts_skip_alias.skip_existing);

        let opts_skip_short =
            S3Opts::try_parse_from(["s3", "--channel", "s3://test-bucket/test", "-s"]).unwrap();
        assert!(opts_skip_short.skip_existing);

        // Verify conflict with --force
        let err_force_skip = S3Opts::try_parse_from([
            "s3",
            "--channel",
            "s3://test-bucket/test",
            "--force",
            "--skip-existing",
        ])
        .unwrap_err();
        assert_eq!(
            err_force_skip.kind(),
            clap::error::ErrorKind::ArgumentConflict
        );

        let err_force_alias = S3Opts::try_parse_from([
            "s3",
            "--channel",
            "s3://test-bucket/test",
            "--force",
            "--skip-if-exists",
        ])
        .unwrap_err();
        assert_eq!(
            err_force_alias.kind(),
            clap::error::ErrorKind::ArgumentConflict
        );

        let err_force_short =
            S3Opts::try_parse_from(["s3", "--channel", "s3://test-bucket/test", "--force", "-s"])
                .unwrap_err();
        assert_eq!(
            err_force_short.kind(),
            clap::error::ErrorKind::ArgumentConflict
        );
    }
}
