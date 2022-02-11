use crate::{Channel, Platform, RepoData};
use async_compression::futures::bufread::GzipDecoder;
use futures::{AsyncBufRead, AsyncRead, AsyncReadExt, TryStreamExt};
use itertools::Itertools;
use regex::internal::Input;
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::mpsc::channel;
use std::task::{Context, Poll};
use thiserror::Error;

#[derive(Copy, Clone, Serialize, Deserialize)]
pub struct RepoDataCache {}

pub struct RepoDataLoader {
    channel: Channel,
    platform: Platform,
    cache_file_path: PathBuf,
}

pub enum LoadRepoDataProgress {
    Downloading {
        progress: usize,
        total: Option<usize>,
    },
    Decoding,
}

#[derive(Error, Debug)]
pub enum LoadRepoDataError {
    #[error("error deserializing repository data: {0}")]
    DeserializeError(#[from] serde_json::Error),

    #[error("error downloading data: {0}")]
    TransportError(#[from] reqwest::Error),

    #[error("error downloading data: {0}")]
    IoError(#[from] std::io::Error),
}

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

impl RepoDataLoader {
    pub fn new(channel: Channel, platform: Platform, cache_dir: impl AsRef<Path>) -> Self {
        let cache_file_path =
            cache_dir
                .as_ref()
                .join(format!("{}/{}.json", channel.canonical_name(), platform));

        Self {
            channel,
            platform,
            cache_file_path,
        }
    }

    pub async fn load<F>(
        self,
        client: &Client,
        mut callback: F,
    ) -> Result<RepoData, LoadRepoDataError>
    where
        F: FnMut(LoadRepoDataProgress) -> () + Send,
    {
        // Notify the callback
        callback(LoadRepoDataProgress::Downloading {
            progress: 0,
            total: None,
        });

        // Create the request and wait for a response
        let response = client
            .get(
                self.channel
                    .platform_url(self.platform)
                    .join("repodata.json")
                    .unwrap(),
            )
            .header(reqwest::header::ACCEPT_ENCODING, "gzip")
            .send()
            .await
            .and_then(|r| r.error_for_status())?;

        // Notify that we are downloading
        let total_size = response.content_length().map(|v| v as usize);
        callback(LoadRepoDataProgress::Downloading {
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
                callback(LoadRepoDataProgress::Downloading {
                    progress: downloaded,
                    total: total_size,
                });
            })
            .map_err(|e| std::io::Error::new(ErrorKind::Other, e))
            .into_async_read();

        let mut bytes = if let Some(length) = total_size {
            Vec::with_capacity(length)
        } else {
            Vec::new()
        };
        if is_gzip_encoded {
            GzipDecoder::new(byte_stream)
                .read_to_end(&mut bytes)
                .await?;
        } else {
            byte_stream.read_to_end(&mut bytes).await?;
        }

        callback(LoadRepoDataProgress::Decoding);
        Ok(serde_json::from_slice(&bytes)?)
    }
}
