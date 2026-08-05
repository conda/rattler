use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::Mutex,
};

use futures::{StreamExt, TryStreamExt};
use miette::IntoDiagnostic;
use opendal::{Configurator, ErrorKind, Operator};
use rattler_azure::{AzureChannelUrl, AzureCredentials, AzureEndpointOptions};

use crate::upload::{
    object_store::{BlobUploadTarget, PACKAGE_CONCURRENCY, stream_package_to_object_store},
    opt::ForceOverwrite,
    package::ExtractedPackage,
};

/// SAS permissions requested when minting a user-delegation SAS for uploads.
/// Creating and writing blobs needs `c` + `w`; `r` is required on top of those
/// because the overwrite guard `stat`s each blob before writing it, and a
/// `stat` (HEAD Blob) is a read. The SAS stays container-scoped and short-lived.
pub(crate) const AZURE_UPLOAD_SAS_PERMISSIONS: &str = "rcw";

/// What became of one package in a concurrent upload run.
enum PackageOutcome {
    Uploaded,
    Failed(String),
}

/// Uploads packages to a channel in an Azure Blob Storage container.
///
/// The account name, endpoint, container and root prefix are all derived from the
/// channel URL together with `options` (see `azblob_config`): `options.addressing`
/// decides whether the account is the first host label or the first path segment,
/// and `options.scheme` decides what `az://` is sent over. A path-style entry is
/// therefore what makes an IP, single-label or emulator (Azurite) endpoint
/// uploadable. The [`AzureCredentials`] supply only the account key or SAS token.
pub async fn upload_package_to_azure(
    channel: AzureChannelUrl,
    credentials: AzureCredentials,
    options: AzureEndpointOptions,
    package_files: &[PathBuf],
    force: ForceOverwrite,
) -> miette::Result<()> {
    let config = rattler_azure::azblob_config(&credentials, &channel, options).into_diagnostic()?;

    let builder = config.into_builder();
    let op = Operator::new(builder).into_diagnostic()?.finish();

    // Upload multiple packages concurrently. Each package is written to its own
    // key, so the individual uploads are independent. The first failure aborts
    // the remaining uploads rather than letting them run to completion, so the
    // outcomes are recorded as they land and summarised below — otherwise a run
    // that failed halfway would report one package and stay silent about the
    // rest.
    let outcomes = Mutex::new(Vec::new());
    let result = futures::stream::iter(package_files.iter())
        .map(Ok)
        .try_for_each_concurrent(PACKAGE_CONCURRENCY, |package_file| {
            let op = op.clone();
            let channel = &channel;
            let outcomes = &outcomes;
            async move {
                let result = upload_single_package(&op, channel, package_file, force).await;
                let outcome = match &result {
                    Ok(()) => PackageOutcome::Uploaded,
                    Err(e) => PackageOutcome::Failed(e.to_string()),
                };
                outcomes
                    .lock()
                    .expect("upload outcome mutex poisoned")
                    .push((package_file.clone(), outcome));
                result
            }
        })
        .await;

    let outcomes = outcomes
        .into_inner()
        .expect("upload outcome mutex poisoned");
    let summary = summarize(&outcomes, package_files.len());
    match result {
        Ok(()) => {
            tracing::info!("{summary}");
            Ok(())
        }
        // Logged rather than attached to the error, so the counts are visible
        // next to the failure that stopped the run without repeating it.
        Err(e) => {
            tracing::error!("{summary}");
            Err(e)
        }
    }
}

/// Renders the per-package outcomes of a run. Packages without an outcome were
/// dropped mid-upload by the fail-fast stream or never started.
fn summarize(outcomes: &[(PathBuf, PackageOutcome)], total: usize) -> String {
    let failed: Vec<_> = outcomes
        .iter()
        .filter_map(|(path, outcome)| match outcome {
            PackageOutcome::Failed(message) => Some((path, message)),
            PackageOutcome::Uploaded => None,
        })
        .collect();
    let uploaded = outcomes.len() - failed.len();
    let not_attempted = total - outcomes.len();

    let mut summary = format!(
        "Azure upload summary: uploaded {uploaded} / failed {} / not attempted {not_attempted}",
        failed.len()
    );
    for (path, message) in failed {
        let _ = write!(summary, "\n  failed: {}: {message}", path.display());
    }
    if not_attempted > 0 {
        summary.push_str("\n  not attempted: cancelled mid-upload or never started");
    }
    summary
}

/// Uploads a single package file to the Azure Blob container via the given operator.
async fn upload_single_package(
    op: &Operator,
    channel: &AzureChannelUrl,
    package_file: &Path,
    force: ForceOverwrite,
) -> miette::Result<()> {
    let package = ExtractedPackage::from_package_file(package_file)?;
    let target = BlobUploadTarget::from_package(&package)?;

    // The blob's address as the user wrote the channel, used only for
    // diagnostics. The canonical spelling already carries `/<container>/<prefix>`,
    // so the key is appended to it; do not prepend the container again. Query and
    // fragment go first, or an inline SAS would land in the middle of the path.
    let blob_url = {
        let mut channel_url = channel.canonical();
        channel_url.set_query(None);
        channel_url.set_fragment(None);
        format!("{channel_url}/{}", target.key())
    };

    // Guard against overwriting an existing blob when `--force` was not passed.
    // opendal 0.57 only honours `if_not_exists` on the single-shot Put Blob path,
    // never the multi-block Put Block List path used for packages larger than a
    // single block, so the writer-level `if_not_exists` silently does nothing for
    // large uploads. An explicit `stat` closes that gap for a blob that already
    // exists, but not for two writers racing to create one: above the chunk size
    // both stat as absent and the second commit wins silently. Concurrent uploads
    // of the same package are unsafe until opendal carries `if_none_match` onto
    // the Put Block List path.
    //
    // TODO: delete this guard and its TOCTOU window once
    // <https://github.com/apache/opendal/pull/7990> merges — it carries changes
    // in this area.
    if !force.is_enabled() {
        match op.stat(target.key()).await {
            Ok(_) => {
                miette::bail!("Package {blob_url} already exists. Use --force to overwrite.");
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e).into_diagnostic(),
        }
    }

    stream_package_to_object_store(op, &target, package_file, &blob_url, force).await
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use opendal::{Operator, services::Memory};
    use rattler_azure::AzureChannelUrl;

    use super::{PackageOutcome, summarize, upload_single_package};
    use crate::upload::opt::ForceOverwrite;
    use crate::upload::package::ExtractedPackage;
    use crate::upload::test_utils::test_package_path;

    fn memory_operator() -> Operator {
        Operator::new(Memory::default()).unwrap().finish()
    }

    fn test_channel() -> AzureChannelUrl {
        AzureChannelUrl::parse("az://account.blob.core.windows.net/container/prefix").unwrap()
    }

    fn package_key() -> String {
        let path = test_package_path();
        let package = ExtractedPackage::from_package_file(&path).unwrap();
        format!(
            "{}/{}",
            package.subdir().unwrap(),
            package.filename().unwrap()
        )
    }

    /// C2: without `--force`, uploading over an existing blob must error rather
    /// than silently overwrite it. The memory backend honours `if_not_exists` on
    /// every path and the fixture is a single block, so this covers the small-blob
    /// path only — for the multi-block behaviour see
    /// `rattler_index/tests/azure_azurite.rs::azurite_if_not_exists_is_dropped_on_the_multi_block_path`.
    #[tokio::test]
    async fn test_existing_blob_without_force_errors() {
        let op = memory_operator();
        let channel = test_channel();
        let package = test_package_path();

        // Seed the target blob so the next upload finds it already present.
        upload_single_package(&op, &channel, &package, ForceOverwrite(true))
            .await
            .expect("initial force upload should succeed");

        let err = upload_single_package(&op, &channel, &package, ForceOverwrite(false))
            .await
            .expect_err("upload over an existing blob without --force must fail");
        assert!(
            err.to_string().contains("already exists"),
            "unexpected error: {err}"
        );
    }

    /// A non-forced upload into an empty container succeeds.
    #[tokio::test]
    async fn test_upload_into_empty_container_succeeds() {
        let op = memory_operator();
        upload_single_package(
            &op,
            &test_channel(),
            &test_package_path(),
            ForceOverwrite(false),
        )
        .await
        .expect("upload into an empty container should succeed");

        let meta = op.stat(&package_key()).await.unwrap();
        let expected_size = std::fs::metadata(test_package_path()).unwrap().len();
        assert_eq!(meta.content_length(), expected_size);
    }

    /// The upload carries the same download metadata as its S3 twin. Only
    /// the content disposition can be asserted here — the memory backend ignores
    /// user metadata, as azblob does above a single block.
    #[tokio::test]
    async fn test_upload_sets_content_disposition() {
        let op = memory_operator();
        upload_single_package(
            &op,
            &test_channel(),
            &test_package_path(),
            ForceOverwrite(false),
        )
        .await
        .expect("upload should succeed");

        let path = test_package_path();
        let package = ExtractedPackage::from_package_file(&path).unwrap();
        let meta = op.stat(&package_key()).await.unwrap();
        assert_eq!(
            meta.content_disposition(),
            Some(format!("attachment; filename={}", package.filename().unwrap()).as_str())
        );
    }

    /// A run that stops early must still say what landed, what failed and
    /// how many uploads never got a verdict.
    #[test]
    fn test_summary_counts_and_names_outcomes() {
        let outcomes = vec![
            (PathBuf::from("a.conda"), PackageOutcome::Uploaded),
            (
                PathBuf::from("b.conda"),
                PackageOutcome::Failed("Package b already exists".to_string()),
            ),
        ];

        let summary = summarize(&outcomes, 4);
        assert!(
            summary.contains("uploaded 1 / failed 1 / not attempted 2"),
            "unexpected summary: {summary}"
        );
        assert!(summary.contains("failed: b.conda: Package b already exists"));
        assert!(summary.contains("not attempted: cancelled mid-upload or never started"));
    }
}
