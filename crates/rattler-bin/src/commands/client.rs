use std::{collections::HashMap, sync::Arc};

use miette::{Context, IntoDiagnostic};
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage};
use reqwest::Client;

/// Creates an HTTP client with the middleware stack used by the CLI for remote fetches.
pub fn create_client_with_middleware() -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    let download_client = Client::builder()
        .no_gzip()
        .build()
        .into_diagnostic()
        .context("failed to create HTTP client")?;

    let authentication_storage =
        AuthenticationStorage::from_env_and_defaults().into_diagnostic()?;

    let client = reqwest_middleware::ClientBuilder::new(download_client.clone())
        .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
            authentication_storage.clone(),
        )))
        .with(rattler_networking::OciMiddleware::new(download_client));
    #[cfg(feature = "s3")]
    let client = client.with(rattler_networking::S3Middleware::new(
        HashMap::new(),
        authentication_storage,
    ));
    #[cfg(feature = "gcs")]
    let client = client.with(rattler_networking::GCSMiddleware::default());

    Ok(client.build())
}
