use reqwest::{
    header,
    header::{HeaderMap, HeaderValue},
    Response,
};
use serde::{Deserialize, Serialize};

/// Extracted HTTP response headers that enable caching the repodata.json files.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CacheHeaders {
    /// The `ETag` HTTP cache header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,

    /// The `Last-Modified` HTTP cache header
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "mod")]
    pub last_modified: Option<String>,

    /// The cache control configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<String>,
}

impl From<&Response> for CacheHeaders {
    fn from(response: &Response) -> Self {
        // Get the ETag from the response (if any). This can be used to cache the result during a
        // next request.
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|header| header.to_str().ok())
            .map(ToOwned::to_owned);

        // Get the last modified time. This can also be used to cache the result during a next
        // request.
        let last_modified = response
            .headers()
            .get(header::LAST_MODIFIED)
            .and_then(|header| header.to_str().ok())
            .map(ToOwned::to_owned);

        // Get the cache-control headers so we possibly perform local caching.
        let cache_control = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|header| header.to_str().ok())
            .map(ToOwned::to_owned);

        Self {
            etag,
            last_modified,
            cache_control,
        }
    }
}

impl CacheHeaders {
    /// Adds the headers to the specified request to short-circuit if the content is still up to
    /// date.
    pub fn add_to_request(&self, headers: &mut HeaderMap) {
        // If previously there was an etag header, add the If-None-Match header so the server only sends
        // us new data if the etag is not longer valid.
        if let Some(etag) = self
            .etag
            .as_deref()
            .and_then(|etag| HeaderValue::from_str(etag).ok())
        {
            headers.insert(header::IF_NONE_MATCH, etag);
        }
        // If a previous request contains a Last-Modified header, add the If-Modified-Since header to let
        // the server send us new data if the contents has been modified since that date.
        if let Some(last_modified) = self
            .last_modified
            .as_deref()
            .and_then(|last_modified| HeaderValue::from_str(last_modified).ok())
        {
            headers.insert(header::IF_MODIFIED_SINCE, last_modified);
        }
    }
}
