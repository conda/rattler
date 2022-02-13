use crate::{Channel, Platform, RepoData};
use async_compression::futures::bufread::GzipDecoder;
use futures::{AsyncReadExt, TryStreamExt};
use reqwest::Response;
use reqwest_middleware::ClientWithMiddleware;
use std::io::ErrorKind;
use thiserror::Error;

/// An error that might happen when fetching repo data from a channel
#[derive(Error, Debug)]
pub enum FetchRepoDataError {
    #[error("error deserializing repository data: {0}")]
    DeserializeError(#[from] serde_json::Error),

    #[error("error downloading data: {0}")]
    TransportError(#[from] reqwest::Error),

    #[error("error in middleware: {0}")]
    MiddlewareError(#[from] anyhow::Error),

    #[error("error downloading data: {0}")]
    IoError(#[from] std::io::Error),
}

impl From<reqwest_middleware::Error> for FetchRepoDataError {
    fn from(e: reqwest_middleware::Error) -> Self {
        match e {
            reqwest_middleware::Error::Middleware(e) => e.into(),
            reqwest_middleware::Error::Reqwest(e) => e.into(),
        }
    }
}

/// Enum with progress information
pub enum FetchRepoDataProgress {
    Downloading {
        progress: usize,
        total: Option<usize>,
    },
    Decoding,
}

impl Channel {
    /// Fetches the [`RepoData`] of the specified platform from this channel.
    pub async fn fetch_repo_data<CB>(
        &self,
        client: &ClientWithMiddleware,
        platform: Platform,
        mut callback: CB,
    ) -> Result<RepoData, FetchRepoDataError>
    where
        CB: FnMut(FetchRepoDataProgress) -> () + Send,
    {
        // Notify the callback
        callback(FetchRepoDataProgress::Downloading {
            progress: 0,
            total: None,
        });

        // Create the request and wait for a response
        let response = client
            .get(self.platform_url(platform).join("repodata.json").unwrap())
            .header(reqwest::header::ACCEPT_ENCODING, "gzip")
            .send()
            .await?
            .error_for_status()?;

        // Try to get the total size of the download
        let total_size = response.content_length().map(|v| v as usize);
        callback(FetchRepoDataProgress::Downloading {
            progress: 0,
            total: total_size,
        });

        let is_gzip_encoded = is_response_encoded_with(&response, "gzip");

        // Read all the contents of the stream
        let mut downloaded = 0;
        let mut byte_stream = response
            .bytes_stream()
            .inspect_ok(|bytes| {
                downloaded += bytes.len();
                callback(FetchRepoDataProgress::Downloading {
                    progress: downloaded,
                    total: total_size,
                });
            })
            .map_err(|e| std::io::Error::new(ErrorKind::Other, e))
            .into_async_read();

        // Allocate bytes to hold the data of the stream so we can parse it.
        let mut bytes = if let Some(length) = total_size {
            Vec::with_capacity(length)
        } else {
            Vec::new()
        };

        // Decode the stream if the stream is compressed
        if is_gzip_encoded {
            GzipDecoder::new(byte_stream)
                .read_to_end(&mut bytes)
                .await?;
        } else {
            byte_stream.read_to_end(&mut bytes).await?;
        }

        // Decode the JSON into actual repo data
        callback(FetchRepoDataProgress::Decoding);
        let repo_data = serde_json::from_slice(&bytes)?;

        Ok(repo_data)
    }
}

/// Returns true if the response is encoded as the specified encoding.
fn is_response_encoded_with(response: &Response, encoding_str: &str) -> bool {
    let headers = response.headers();
    headers
        .get_all(reqwest::header::CONTENT_ENCODING)
        .iter()
        .any(|enc| enc == encoding_str)
        || headers
            .get_all(reqwest::header::TRANSFER_ENCODING)
            .iter()
            .any(|enc| enc == encoding_str)
}
