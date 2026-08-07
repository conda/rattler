//! The upload module provides the package upload functionality.

use self::opt::ArtifactoryAuthentication;
use crate::{
    AnacondaData, ArtifactoryData, CloudsmithData, QuetzData, tool_configuration::APP_USER_AGENT,
};
use fs_err::tokio as fs;
use futures::TryStreamExt;
use indicatif::{HumanBytes, ProgressState, style::TemplateError};
use reqwest_retry::{RetryDecision, RetryPolicy, policies::ExponentialBackoff};
use std::{
    fmt::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio_util::io::ReaderStream;

use miette::{Context, IntoDiagnostic};
use rattler_networking::{Authentication, AuthenticationStorage};
use rattler_redaction::Redact;
use reqwest::{Method, StatusCode};
use tracing::{info, warn};
use url::Url;

use crate::upload::package::{ExtractedPackage, sha256_sum};

#[cfg(test)]
pub(crate) mod test_utils;

mod anaconda;
#[cfg(feature = "azure")]
mod azure;
#[cfg(feature = "azure")]
pub(crate) use azure::AZURE_UPLOAD_SAS_PERMISSIONS;
#[cfg(feature = "azure")]
pub use azure::upload_package_to_azure;
#[cfg(feature = "sigstore-sign")]
pub mod attestation;
mod cloudsmith;
pub mod conda_forge;
pub mod opt;
mod package;
mod prefix;
#[cfg(feature = "s3")]
mod s3;
#[cfg(feature = "s3")]
pub use s3::upload_package_to_s3;

pub use anaconda::AnacondaError;
pub use cloudsmith::CloudsmithError;
pub use prefix::{PrefixUploadError, upload_package_to_prefix};

/// The streaming upload shared by the object-store backends (S3 and Azure Blob),
/// which both drive an opendal writer.
#[cfg(any(feature = "s3", feature = "azure"))]
pub(crate) mod object_store {
    use std::{collections::HashMap, path::Path};

    use miette::IntoDiagnostic;
    use opendal::{ErrorKind, Operator, options::WriteOptions};
    use rattler_digest::{HashingReader, Md5, Md5Hash, Sha256, Sha256Hash};
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    use tokio_util::bytes::BytesMut;

    use crate::upload::{opt::ForceOverwrite, package::ExtractedPackage};

    /// An object store whose errors cannot carry a credential.
    ///
    /// opendal stamps the request URL into the context of every HTTP error it
    /// builds, and prints that context from both `Display` and `Debug`. For Azure
    /// the SAS *is* in the URL, so any opendal error that escapes unmasked is a
    /// leaked credential — into a log, a `miette` report, or a CI transcript.
    ///
    /// The inner [`Operator`] is private and the only error type out is
    /// [`BlobStoreError`], which is built by masking. Reaching for an opendal
    /// method this does not have means adding it here, where leaving the masking
    /// out is a visible omission rather than a silent leak.
    #[derive(Clone)]
    pub(crate) struct BlobStore(Operator);

    /// An opendal error with any pre-signed signature masked out of its text.
    ///
    /// Carries the [`ErrorKind`] separately because callers branch on it — a
    /// `NotFound` from the overwrite guard, a `ConditionNotMatch` from a write that
    /// lost a race — and must not have to read the message to do so.
    #[derive(Debug, thiserror::Error)]
    #[error("{message}")]
    pub(crate) struct BlobStoreError {
        kind: ErrorKind,
        message: String,
    }

    impl BlobStoreError {
        fn new(err: opendal::Error) -> Self {
            Self {
                kind: err.kind(),
                // `Debug` rather than `Display`: it is the spelling that keeps the
                // source chain and the operation, and it is also the one the leak
                // was found in.
                message: rattler_redaction::redact_signatures_in_text(
                    &format!("{err:?}"),
                    rattler_redaction::DEFAULT_REDACTION_STR,
                )
                .into_owned(),
            }
        }

        pub(crate) fn kind(&self) -> ErrorKind {
            self.kind
        }
    }

    impl BlobStore {
        pub(crate) fn new(builder: impl opendal::Builder) -> Result<Self, BlobStoreError> {
            Ok(Self(
                Operator::new(builder)
                    .map_err(BlobStoreError::new)?
                    .finish(),
            ))
        }

        /// Metadata for one blob, used by the callers' overwrite guards.
        #[cfg(feature = "azure")]
        pub(crate) async fn stat(&self, path: &str) -> Result<opendal::Metadata, BlobStoreError> {
            self.0.stat(path).await.map_err(BlobStoreError::new)
        }

        async fn writer(
            &self,
            path: &str,
            options: WriteOptions,
        ) -> Result<BlobWriter, BlobStoreError> {
            self.0
                .writer_options(path, options)
                .await
                .map(BlobWriter)
                .map_err(BlobStoreError::new)
        }
    }

    /// A writer that masks its errors, for the same reason [`BlobStore`] does: a
    /// failed block upload reports the URL it was sent to.
    struct BlobWriter(opendal::Writer);

    impl BlobWriter {
        async fn write(&mut self, chunk: tokio_util::bytes::Bytes) -> Result<(), BlobStoreError> {
            self.0.write(chunk).await.map_err(BlobStoreError::new)
        }

        async fn close(&mut self) -> Result<(), BlobStoreError> {
            self.0
                .close()
                .await
                .map(|_| ())
                .map_err(BlobStoreError::new)
        }

        async fn abort(&mut self) -> Result<(), BlobStoreError> {
            self.0.abort().await.map_err(BlobStoreError::new)
        }
    }

    /// Size of a single chunk handed to the writer. S3 rejects every multipart
    /// part but the last below 5 MiB, and Azure Blob bills per block, so both
    /// backends prefer few large chunks.
    ///
    /// Peak buffered bytes across a run are `PACKAGE_CONCURRENCY *
    /// PART_CONCURRENCY * DESIRED_CHUNK_SIZE` = 160 MiB.
    const DESIRED_CHUNK_SIZE: usize = 1024 * 1024 * 10;

    /// Number of chunks of a single package that are uploaded concurrently.
    const PART_CONCURRENCY: usize = 4;

    /// Number of packages that are uploaded concurrently.
    pub(crate) const PACKAGE_CONCURRENCY: usize = 4;

    /// A package resolved to the channel-relative key it is stored under. Holding
    /// the key and the filename together keeps the two from disagreeing.
    pub(crate) struct BlobUploadTarget {
        key: String,
        filename: String,
    }

    impl BlobUploadTarget {
        /// Resolves `<subdir>/<filename>` from the package's own `index.json`.
        pub(crate) fn from_package(package: &ExtractedPackage<'_>) -> miette::Result<Self> {
            let subdir = package
                .subdir()
                .ok_or_else(|| miette::miette!("Failed to get subdir"))?;
            let filename = package
                .filename()
                .ok_or_else(|| miette::miette!("Failed to get filename"))?;
            Ok(Self {
                key: format!("{subdir}/{filename}"),
                filename: filename.to_string(),
            })
        }

        /// The channel-relative key the package is written to.
        pub(crate) fn key(&self) -> &str {
            &self.key
        }
    }

    /// A file measured and hashed by one pass over a single handle, rewound and
    /// ready to be read again. Size and hashes describe the same bytes, so the
    /// upload cannot publish a length a concurrent writer changed after a `stat`.
    struct HashedFile<R> {
        reader: R,
        size: u64,
        sha256: Sha256Hash,
        md5: Md5Hash,
    }

    async fn hash_file(
        path: &Path,
    ) -> miette::Result<HashedFile<impl AsyncReadExt + AsyncSeekExt + Unpin>> {
        let file =
            tokio::io::BufReader::new(fs_err::tokio::File::open(path).await.into_diagnostic()?);
        let sha256_reader = HashingReader::<_, Sha256>::new(file);
        let mut md5_reader = HashingReader::<_, Md5>::new(sha256_reader);
        let size = tokio::io::copy(&mut md5_reader, &mut tokio::io::sink())
            .await
            .into_diagnostic()?;
        let (sha256_reader, md5) = md5_reader.finalize();
        let (mut reader, sha256) = sha256_reader.finalize();
        reader.rewind().await.into_diagnostic()?;
        Ok(HashedFile {
            reader,
            size,
            sha256,
            md5,
        })
    }

    /// Streams `package_file` to `target`'s key through `op`.
    ///
    /// `destination` is the blob as the user addressed it and appears in the
    /// success log and in the "already exists" error. `if_not_exists` is asked of
    /// the backend, which is free to drop it — the caller is responsible for any
    /// guard it needs on top (see `azure::upload_single_package`).
    pub(crate) async fn stream_package_to_object_store(
        store: &BlobStore,
        target: &BlobUploadTarget,
        package_file: &Path,
        destination: &str,
        force: ForceOverwrite,
    ) -> miette::Result<()> {
        let HashedFile {
            mut reader,
            size,
            sha256,
            md5,
        } = hash_file(package_file).await?;

        // S3 honours both. azblob never sends content-disposition, and drops user
        // metadata on its Put Block List commit, so a package above
        // `DESIRED_CHUNK_SIZE` lands there with neither.
        let options = WriteOptions {
            chunk: Some(DESIRED_CHUNK_SIZE),
            concurrent: PART_CONCURRENCY,
            content_disposition: Some(format!("attachment; filename={}", target.filename)),
            user_metadata: Some(HashMap::from([
                (String::from("package-sha256"), hex::encode(sha256)),
                (String::from("package-md5"), hex::encode(md5)),
            ])),
            if_not_exists: !force.is_enabled(),
            ..WriteOptions::default()
        };

        let already_exists =
            || miette::miette!("Package {destination} already exists. Use --force to overwrite.");

        // `if_not_exists` is not evaluated here on either backend: both build their
        // writer with a pure constructor and issue nothing until the first chunk, so
        // a lost race always surfaces at `close()` below. This arm covers what
        // `writer()` itself can reject — a capability opendal refuses up front — and
        // exists so that the answer cannot depend on which backend is in play.
        let mut writer = match store.writer(target.key(), options).await {
            Ok(writer) => writer,
            Err(e) if e.kind() == ErrorKind::ConditionNotMatch => return Err(already_exists()),
            Err(e) => return Err(e).into_diagnostic(),
        };

        if let Err(e) = stream_chunks(&mut writer, &mut reader, size).await {
            discard_partial_upload(&mut writer, destination).await;
            return Err(e);
        }

        match writer.close().await {
            Ok(_) => {
                tracing::info!("Uploaded package to {destination}");
                Ok(())
            }
            Err(e) => {
                discard_partial_upload(&mut writer, destination).await;
                if e.kind() == ErrorKind::ConditionNotMatch {
                    return Err(already_exists());
                }
                Err(e).into_diagnostic()
            }
        }
    }

    /// Feeds exactly `size` bytes of `reader` to `writer`. opendal buffers them
    /// into correctly sized parts/blocks and uploads `PART_CONCURRENCY` at a time.
    async fn stream_chunks(
        writer: &mut BlobWriter,
        reader: &mut (impl AsyncReadExt + Unpin),
        size: u64,
    ) -> miette::Result<()> {
        let mut remaining_size = size as usize;
        while remaining_size > 0 {
            let chunk_size = remaining_size.min(DESIRED_CHUNK_SIZE);
            let mut chunk = BytesMut::zeroed(chunk_size);

            let bytes_read = reader.read_exact(&mut chunk[..]).await.into_diagnostic()?;
            debug_assert_eq!(bytes_read, chunk.len());

            writer.write(chunk.freeze()).await.into_diagnostic()?;

            remaining_size = remaining_size.saturating_sub(bytes_read);
        }
        Ok(())
    }

    /// Uncommitted parts are billed until they are discarded. S3 discards them
    /// here; azblob's abort is a no-op, so Azure only collects its uncommitted
    /// blocks after a week without further writes to the blob.
    async fn discard_partial_upload(writer: &mut BlobWriter, destination: &str) {
        if let Err(e) = writer.abort().await {
            tracing::warn!("Failed to discard the partial upload of {destination}: {e}");
        }
    }

    #[cfg(test)]
    mod test {
        use super::{BlobStoreError, hash_file};
        use crate::upload::test_utils::test_package_path;
        use opendal::ErrorKind;
        use rattler_digest::{Md5, Sha256, compute_file_digest};

        /// The size the upload streams and the hashes it records must come
        /// from the same pass, so they always describe the same bytes.
        #[tokio::test]
        async fn test_hash_file_size_and_hashes_agree_with_the_file() {
            let path = test_package_path();
            let hashed = hash_file(&path).await.expect("hashing the package failed");

            assert_eq!(hashed.size, std::fs::metadata(&path).unwrap().len());
            assert_eq!(
                hashed.sha256,
                compute_file_digest::<Sha256>(&path).unwrap(),
                "recorded sha256 must match the file's"
            );
            assert_eq!(
                hashed.md5,
                compute_file_digest::<Md5>(&path).unwrap(),
                "recorded md5 must match the file's"
            );
        }

        /// The whole reason `BlobStore` hides its `Operator`: opendal puts the
        /// request URL in the error context, and for Azure the SAS is in that URL.
        #[test]
        fn blob_store_errors_do_not_carry_a_signature() {
            let err = BlobStoreError::new(
                opendal::Error::new(ErrorKind::NotFound, "blob not found").with_context(
                    "url",
                    "https://acct.blob.core.windows.net/c/p?sv=2025-01-05&sig=s3cr3t",
                ),
            );

            let message = err.to_string();
            assert!(!message.contains("s3cr3t"), "{message}");
            // Everything that makes the error useful survives.
            assert_eq!(err.kind(), ErrorKind::NotFound);
            assert!(
                message.contains("acct.blob.core.windows.net/c/p"),
                "{message}"
            );
        }
    }
}

/// Returns the style to use for a progress bar that is currently in progress.
fn default_bytes_style() -> Result<indicatif::ProgressStyle, TemplateError> {
    Ok(indicatif::ProgressStyle::default_bar()
            .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{bar:40!.bright.yellow/dim.white}] {bytes:>8} @ {smoothed_bytes_per_sec:8}")?
            .progress_chars("━━╾─")
            .with_key(
                "smoothed_bytes_per_sec",
                |s: &ProgressState, w: &mut dyn Write| match (s.pos(), s.elapsed().as_millis()) {
                    (pos, elapsed_ms) if elapsed_ms > 0 => {
                        // TODO: log with tracing?
                        _ = write!(w, "{}/s", HumanBytes((pos as f64 * 1000_f64 / elapsed_ms as f64) as u64));
                    }
                    _ => {
                        _ = write!(w, "-");
                    },
                },
            ))
}

fn get_default_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .no_gzip()
        .user_agent(APP_USER_AGENT)
        .build()
}

/// Returns a reqwest client with retry middleware.
fn get_client_with_retry() -> Result<reqwest_middleware::ClientWithMiddleware, reqwest::Error> {
    let client = reqwest::Client::builder()
        .no_gzip()
        .user_agent(APP_USER_AGENT)
        .build()?;

    Ok(reqwest_middleware::ClientBuilder::new(client)
        .with(reqwest_retry::RetryTransientMiddleware::new_with_policy(
            reqwest_retry::policies::ExponentialBackoff::builder().build_with_max_retries(3),
        ))
        .build())
}

/// Uploads package files to a Quetz server.
pub async fn upload_package_to_quetz(
    storage: &AuthenticationStorage,
    package_files: &Vec<PathBuf>,
    quetz_data: QuetzData,
) -> miette::Result<()> {
    let token = match quetz_data.api_key {
        Some(api_key) => api_key,
        None => match storage.get_by_url(Url::from(quetz_data.url.clone())) {
            Ok((_, Some(Authentication::CondaToken(token)))) => token,
            Ok((_, Some(_))) => {
                return Err(miette::miette!("A Conda token is required for authentication with quetz.
                        Authentication information found in the keychain / auth file, but it was not a Conda token"));
            }
            Ok((_, None)) => {
                return Err(miette::miette!(
                    "No quetz api key was given and none was found in the keychain / auth file"
                ));
            }
            Err(e) => {
                return Err(miette::miette!(
                    "Failed to get authentication information form keychain: {e}"
                ));
            }
        },
    };

    let client = get_default_client().into_diagnostic()?;

    for package_file in package_files {
        let upload_url = quetz_data
            .url
            .join(&format!(
                "api/channels/{}/upload/{}",
                quetz_data.channels,
                package_file.file_name().unwrap().to_string_lossy()
            ))
            .into_diagnostic()?;

        let hash = sha256_sum(package_file).into_diagnostic()?;

        let prepared_request = client
            .request(Method::POST, upload_url)
            .query(&[("force", "false"), ("sha256", &hash)])
            .header("X-API-Key", token.clone());

        send_request_with_retry(prepared_request, package_file).await?;
    }

    info!("Packages successfully uploaded to Quetz server");

    Ok(())
}

/// Uploads package files to an Artifactory server.
pub async fn upload_package_to_artifactory(
    storage: &AuthenticationStorage,
    package_files: &Vec<PathBuf>,
    artifactory_data: ArtifactoryData,
) -> miette::Result<()> {
    let authentication = match artifactory_data.authentication {
        Some(ArtifactoryAuthentication::Token(token)) => Authentication::BearerToken(token),
        Some(ArtifactoryAuthentication::Basic { username, password }) => {
            Authentication::BasicHTTP { username, password }
        }
        None => match storage.get_by_url(Url::from(artifactory_data.url.clone())) {
            Ok((
                _,
                Some(
                    authentication @ (Authentication::BearerToken(_)
                    | Authentication::BasicHTTP { .. }),
                ),
            )) => authentication,
            Ok((_, Some(_))) => {
                return Err(miette::miette!(
                    "Authentication information found in the keychain / auth file, but it was neither a bearer token nor HTTP basic auth credentials"
                ));
            }
            Ok((_, None)) => {
                return Err(miette::miette!(
                    "No bearer token or HTTP basic auth credentials were given or found in the keychain / auth file"
                ));
            }
            Err(e) => {
                return Err(miette::miette!(
                    "Failed to get authentication information from keychain: {e}"
                ));
            }
        },
    };

    for package_file in package_files {
        let package = ExtractedPackage::from_package_file(package_file)?;

        let subdir = package.subdir().ok_or_else(|| {
            miette::miette!(
                "index.json of package {} has no subdirectory. Cannot determine which directory to upload to",
                package_file.display()
            )
        })?;

        let package_name = package.filename().ok_or(miette::miette!(
            "Package file {} has no filename",
            package_file.display()
        ))?;

        let client = get_default_client().into_diagnostic()?;

        let upload_url = artifactory_data
            .url
            .join(&format!(
                "{}/{}/{}",
                artifactory_data.channels, subdir, package_name
            ))
            .into_diagnostic()?;

        let prepared_request = client.request(Method::PUT, upload_url);
        let prepared_request = match &authentication {
            Authentication::BearerToken(token) => prepared_request.bearer_auth(token),
            Authentication::BasicHTTP { username, password } => {
                prepared_request.basic_auth(username, Some(password))
            }
            _ => unreachable!("Artifactory authentication was validated above"),
        };

        send_request_with_retry(prepared_request, package_file).await?;
    }

    info!("Packages successfully uploaded to Artifactory server");

    Ok(())
}

/// Uploads package files to an Anaconda server.
pub async fn upload_package_to_anaconda(
    storage: &AuthenticationStorage,
    package_files: &Vec<PathBuf>,
    anaconda_data: AnacondaData,
) -> Result<(), anaconda::AnacondaError> {
    let token = match anaconda_data.api_key {
        Some(token) => token,
        None => match storage.get_by_url(Url::from(anaconda_data.url.clone())) {
            Ok((_, Some(Authentication::CondaToken(token)))) => token,
            Ok((_, Some(_))) => {
                return Err(anaconda::AnacondaError::WrongAuthenticationType);
            }
            Ok((_, None)) => {
                return Err(anaconda::AnacondaError::MissingApiKey);
            }
            Err(e) => {
                return Err(anaconda::AnacondaError::KeychainError {
                    message: e.to_string(),
                });
            }
        },
    };

    let anaconda = anaconda::Anaconda::new(token, anaconda_data.url);

    for package_file in package_files {
        loop {
            let package = package::ExtractedPackage::from_package_file(package_file)?;

            anaconda
                .create_or_update_package(&anaconda_data.owner, &package)
                .await?;

            anaconda
                .create_or_update_release(&anaconda_data.owner, &package)
                .await?;

            let successful = anaconda
                .upload_file(
                    &anaconda_data.owner,
                    &anaconda_data.channels,
                    anaconda_data.force,
                    &package,
                )
                .await?;

            // When running with --force and experiencing a conflict error, we delete the conflicting file.
            // Anaconda automatically deletes releases / packages when the deletion of a file would leave them empty.
            // Therefore, we need to ensure that the release / package still exists before trying to upload again.
            if successful {
                break;
            }
        }
    }
    Ok(())
}

/// Uploads package files to a Cloudsmith repository.
pub async fn upload_package_to_cloudsmith(
    storage: &AuthenticationStorage,
    package_files: &Vec<PathBuf>,
    cloudsmith_data: CloudsmithData,
) -> Result<(), cloudsmith::CloudsmithError> {
    let token = match cloudsmith_data.api_key {
        Some(token) => token,
        None => match storage.get_by_url(Url::from(cloudsmith_data.url.clone())) {
            Ok((
                _,
                Some(Authentication::CondaToken(token) | Authentication::BearerToken(token)),
            )) => token,
            Ok((_, Some(_))) => {
                return Err(cloudsmith::CloudsmithError::WrongAuthenticationType);
            }
            Ok((_, None)) => {
                return Err(cloudsmith::CloudsmithError::MissingApiKey);
            }
            Err(e) => {
                return Err(cloudsmith::CloudsmithError::KeychainError {
                    message: e.to_string(),
                });
            }
        },
    };

    let client = cloudsmith::Cloudsmith::new(
        token,
        cloudsmith_data.url,
        cloudsmith_data.owner,
        cloudsmith_data.repo,
    );

    for package_file in package_files {
        let package = package::ExtractedPackage::from_package_file(package_file)?;
        let filename = package.filename().ok_or_else(|| {
            miette::miette!("Package file {} has no filename", package_file.display())
        })?;

        let md5 = package.md5_hex().into_diagnostic()?;
        let file_size = package.file_size().into_diagnostic()?;
        let is_multipart = file_size >= cloudsmith::CHUNK_SIZE as u64;

        let upload_response = client.request_upload(filename, &md5, is_multipart).await?;

        if is_multipart {
            client
                .upload_file_multipart(
                    &upload_response.upload_url,
                    &upload_response.identifier,
                    package_file,
                )
                .await?;
        } else {
            client
                .upload_file_single(
                    &upload_response.upload_url,
                    &upload_response.upload_fields,
                    package_file,
                )
                .await?;
        }

        let pkg_response = client.create_package(&upload_response.identifier).await?;
        info!(
            "Package created: slug_perm={}, slug={}",
            pkg_response.slug_perm, pkg_response.slug
        );
    }

    info!("Packages successfully uploaded to Cloudsmith");
    Ok(())
}

async fn send_request_with_retry(
    prepared_request: reqwest::RequestBuilder,
    package_file: &Path,
) -> miette::Result<reqwest::Response> {
    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
    let mut current_try = 0;

    let request_start = SystemTime::now();

    loop {
        let request = prepared_request
            .try_clone()
            .expect("Could not clone request. Does it have a streaming body?");
        let response = send_request(request, package_file).await?;

        if response.status().is_success() {
            return Ok(response);
        }

        let status = response.status();
        let body = response.text().await.into_diagnostic()?;
        let err = miette::miette!(
            "Failed to upload package file: {}\nStatus: {}\nBody: {}",
            package_file.display(),
            status,
            body
        );

        // Non-retry status codes
        match status {
            // Authentication/Authorization errors
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(miette::miette!("Authentication error: {}", err));
            }
            // Resource conflicts
            StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY => {
                return Err(miette::miette!("Resource conflict: {}", err));
            }
            // Client errors
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::PAYLOAD_TOO_LARGE => {
                return Err(miette::miette!("Client error: {}", err));
            }
            _ => {}
        }

        match retry_policy.should_retry(request_start, current_try) {
            RetryDecision::DoNotRetry => {
                return Err(err);
            }
            RetryDecision::Retry { execute_after } => {
                let sleep_for = execute_after
                    .duration_since(SystemTime::now())
                    .unwrap_or(Duration::ZERO);
                warn!(
                    "Failed to upload package file: {}\nStatus: {}\nBody: {}\nRetrying in {} seconds",
                    package_file.display(),
                    status,
                    body,
                    sleep_for.as_secs()
                );
                tokio::time::sleep(sleep_for).await;
            }
        }

        current_try += 1;
    }
}

/// Note that we need to use a regular request. `reqwest_retry` does not support streaming requests.
async fn send_request(
    prepared_request: reqwest::RequestBuilder,
    package_file: &Path,
) -> miette::Result<reqwest::Response> {
    let file = fs::File::open(package_file).await.into_diagnostic()?;

    let file_size = file.metadata().await.into_diagnostic()?.len();
    info!(
        "Uploading package file: {} ({})\n",
        package_file
            .file_name()
            .expect("no filename found")
            .to_string_lossy(),
        HumanBytes(file_size)
    );
    let progress_bar = indicatif::ProgressBar::new(file_size)
        .with_prefix("Uploading")
        .with_style(default_bytes_style().into_diagnostic()?);

    let progress_bar_clone = progress_bar.clone();
    let reader_stream = ReaderStream::new(file)
        .inspect_ok(move |bytes| {
            progress_bar_clone.inc(bytes.len() as u64);
        })
        .inspect_err(|e| {
            println!("Error while uploading: {e}");
        });

    let body = reqwest::Body::wrap_stream(reader_stream);

    let response = prepared_request
        .body(body)
        .send()
        .await
        .map_err(Redact::redact)
        .into_diagnostic()?;

    response
        .error_for_status_ref()
        .map_err(Redact::redact)
        .into_diagnostic()
        .wrap_err("Server responded with error")?;

    progress_bar.finish();
    info!(
        "\nUpload complete for package file: {}",
        package_file
            .file_name()
            .expect("no filename found")
            .to_string_lossy()
    );

    Ok(response)
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use axum::{Router, http::StatusCode};
    use rattler_networking::{
        Authentication, AuthenticationStorage,
        authentication_storage::backends::memory::MemoryStorage,
    };

    use crate::upload::opt::{ArtifactoryData, QuetzData};
    use crate::upload::test_utils::{start_test_server, test_package_path};

    async fn ok_with_api_key(
        headers: axum::http::HeaderMap,
        _body: axum::body::Bytes,
    ) -> StatusCode {
        assert!(headers.get("x-api-key").is_some());
        StatusCode::OK
    }

    async fn ok_with_bearer(
        headers: axum::http::HeaderMap,
        _body: axum::body::Bytes,
    ) -> StatusCode {
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("Bearer "));
        StatusCode::OK
    }

    async fn ok_with_basic(headers: axum::http::HeaderMap, _body: axum::body::Bytes) -> StatusCode {
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        // Base64 encoding of `test-user:test-password`, as required by HTTP basic auth.
        assert_eq!(auth, "Basic dGVzdC11c2VyOnRlc3QtcGFzc3dvcmQ=");
        StatusCode::OK
    }

    async fn unauthorized(_body: axum::body::Bytes) -> StatusCode {
        StatusCode::UNAUTHORIZED
    }

    async fn conflict(_body: axum::body::Bytes) -> StatusCode {
        StatusCode::CONFLICT
    }

    #[tokio::test]
    async fn test_quetz_upload_success() {
        let router = Router::new().fallback(ok_with_api_key);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let quetz_data = QuetzData::new(
            url,
            "test-channel".to_string(),
            Some("test-api-key".to_string()),
        );
        let result =
            super::upload_package_to_quetz(&storage, &vec![test_package_path()], quetz_data).await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_quetz_upload_auth_failure() {
        let router = Router::new().fallback(unauthorized);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let quetz_data =
            QuetzData::new(url, "test-channel".to_string(), Some("bad-key".to_string()));
        let result =
            super::upload_package_to_quetz(&storage, &vec![test_package_path()], quetz_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_quetz_upload_conflict() {
        let router = Router::new().fallback(conflict);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let quetz_data = QuetzData::new(
            url,
            "test-channel".to_string(),
            Some("test-key".to_string()),
        );
        let result =
            super::upload_package_to_quetz(&storage, &vec![test_package_path()], quetz_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_artifactory_bearer_upload_success() {
        let router = Router::new().fallback(ok_with_bearer);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let artifactory_data = ArtifactoryData::new(url, "test-channel".to_string())
            .with_bearer_auth("test-token".to_string());
        let result = super::upload_package_to_artifactory(
            &storage,
            &vec![test_package_path()],
            artifactory_data,
        )
        .await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_artifactory_basic_upload_success() {
        let router = Router::new().fallback(ok_with_basic);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let artifactory_data = ArtifactoryData::new(url, "test-channel".to_string())
            .with_basic_auth("test-user".to_string(), "test-password".to_string());
        let result = super::upload_package_to_artifactory(
            &storage,
            &vec![test_package_path()],
            artifactory_data,
        )
        .await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_artifactory_basic_upload_from_auth_storage() {
        let router = Router::new().fallback(ok_with_basic);
        let url = start_test_server(router).await;
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        storage
            .store(
                url.host_str().unwrap(),
                &Authentication::BasicHTTP {
                    username: "test-user".to_string(),
                    password: "test-password".to_string(),
                },
            )
            .unwrap();
        let artifactory_data = ArtifactoryData::new(url, "test-channel".to_string());
        let result = super::upload_package_to_artifactory(
            &storage,
            &vec![test_package_path()],
            artifactory_data,
        )
        .await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_artifactory_upload_auth_failure() {
        let router = Router::new().fallback(unauthorized);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let artifactory_data = ArtifactoryData::new(url, "test-channel".to_string())
            .with_bearer_auth("bad-token".to_string());
        let result = super::upload_package_to_artifactory(
            &storage,
            &vec![test_package_path()],
            artifactory_data,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cloudsmith_upload_success() {
        use axum::routing::post;
        use std::net::SocketAddr;

        // Bind the listener first so we know the port for the upload_url response
        let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url: url::Url = format!("http://{}:{}", addr.ip(), addr.port())
            .parse()
            .unwrap();

        let upload_handler = {
            let base_url = base_url.clone();
            move |headers: axum::http::HeaderMap| {
                let base_url = base_url.clone();
                async move {
                    assert!(headers.get("X-Api-Key").is_some());
                    let upload_url = base_url.join("s3-upload").unwrap();
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "application/json")],
                        serde_json::json!({
                            "identifier": "test-file-id",
                            "upload_url": upload_url.to_string(),
                            "upload_fields": {"key": "value"}
                        })
                        .to_string(),
                    )
                }
            }
        };

        let router = Router::new()
            .route("/files/{owner}/{repo}/", post(upload_handler))
            .route("/s3-upload", post(|| async { StatusCode::OK }))
            .route(
                "/packages/{owner}/{repo}/upload/conda/",
                post(|| async {
                    (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        serde_json::json!({
                            "slug_perm": "test-slug-perm",
                            "slug": "test-slug"
                        })
                        .to_string(),
                    )
                }),
            );

        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let storage = AuthenticationStorage::empty();
        let cloudsmith_data = crate::upload::opt::CloudsmithData::new(
            "test-owner".to_string(),
            "test-repo".to_string(),
            Some("test-api-key".to_string()),
            Some(base_url),
        );
        let result = super::upload_package_to_cloudsmith(
            &storage,
            &vec![test_package_path()],
            cloudsmith_data,
        )
        .await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_cloudsmith_upload_missing_api_key() {
        let storage = AuthenticationStorage::empty();
        let cloudsmith_data = crate::upload::opt::CloudsmithData::new(
            "test-owner".to_string(),
            "test-repo".to_string(),
            None,
            Some("http://127.0.0.1:1".parse().unwrap()),
        );
        let result = super::upload_package_to_cloudsmith(
            &storage,
            &vec![test_package_path()],
            cloudsmith_data,
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            super::cloudsmith::CloudsmithError::MissingApiKey
        ),);
    }
}
