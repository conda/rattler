//! Fetching small files (manifests, repodata) completely.

use bytes::Bytes;
use reqwest_middleware::ClientWithMiddleware;

use crate::{Location, LookupError};

/// Fetches a file completely. Returns `None` if it does not exist.
pub(crate) async fn fetch_optional(
    location: &Location,
    client: &ClientWithMiddleware,
) -> Result<Option<Bytes>, LookupError> {
    match location {
        Location::Url(url) => {
            let http_error = |source| LookupError::Http {
                url: url.clone(),
                source: std::sync::Arc::new(source),
            };
            let response = client.get(url.clone()).send().await.map_err(http_error)?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            let response = response
                .error_for_status()
                .map_err(|e| http_error(e.into()))?;
            let bytes = response.bytes().await.map_err(|e| http_error(e.into()))?;
            Ok(Some(bytes))
        }
        Location::Path(path) => match tokio::fs::read(path).await {
            Ok(bytes) => Ok(Some(bytes.into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(LookupError::io(location, e)),
        },
    }
}
