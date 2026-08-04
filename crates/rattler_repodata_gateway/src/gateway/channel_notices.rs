//! Fetching and caching of CEP-6 channel notices.

use std::{collections::HashSet, sync::Arc, time::Duration};

#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use wasmtimer::std::Instant;

use futures::{TryStreamExt, future::OptionFuture};
use rattler_conda_types::{Channel, ChannelNotice, ChannelUrl};
use rattler_redaction::Redact;
use reqwest::StatusCode;
use serde::Deserialize;

use crate::{Reporter, reporter::ResponseReporterExt};

use super::GatewayInner;

const NOTICES_FILENAME: &str = "notices.json";
const MAX_NOTICES_SIZE: usize = 1024 * 1024;
const EMPTY_NOTICES_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const FAILED_NOTICES_TTL: Duration = Duration::from_secs(5 * 60);

/// A channel notice together with the channel that published it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelNoticeResult {
    /// The channel that published the notice.
    pub channel: ChannelUrl,
    /// The published CEP-6 notice.
    pub notice: ChannelNotice,
}

pub(super) struct CachedChannelNotices {
    notices: Arc<Vec<ChannelNotice>>,
    refresh_at: Instant,
}

impl CachedChannelNotices {
    fn is_fresh(&self) -> bool {
        Instant::now() < self.refresh_at
    }
}

struct NoticeFetch {
    notices: Vec<ChannelNotice>,
    ttl: Duration,
}

impl NoticeFetch {
    fn failed() -> Self {
        Self {
            notices: Vec::new(),
            ttl: FAILED_NOTICES_TTL,
        }
    }

    fn empty() -> Self {
        Self {
            notices: Vec::new(),
            ttl: EMPTY_NOTICES_TTL,
        }
    }

    fn from_notices(mut notices: Vec<ChannelNotice>) -> Self {
        let now = jiff::Timestamp::now();
        let had_expired_notice = notices
            .iter()
            .any(|notice| notice.expires_at.is_some_and(|expires| expires <= now));
        notices.retain(|notice| notice.expires_at.is_none_or(|expires| expires > now));

        let ttl = notices
            .iter()
            .filter_map(|notice| notice.expires_at)
            .filter_map(|expires| Duration::try_from(expires.duration_since(now)).ok())
            .min()
            .unwrap_or(if had_expired_notice {
                FAILED_NOTICES_TTL
            } else {
                EMPTY_NOTICES_TTL
            });

        Self { notices, ttl }
    }
}

impl GatewayInner {
    /// Fetch notices for the channels, returning an empty vector when notices
    /// are disabled. Notice failures are intentionally non-fatal: a missing or
    /// malformed `notices.json` must never prevent repodata from being used.
    pub(super) async fn get_channel_notices<'a>(
        &self,
        channels: impl IntoIterator<Item = &'a Channel>,
        reporter: Option<&dyn Reporter>,
    ) -> Vec<ChannelNoticeResult> {
        if !self.channel_notices {
            return Vec::new();
        }

        let mut seen = HashSet::new();
        let channels: Vec<_> = channels
            .into_iter()
            .filter(|channel| seen.insert(channel.base_url.clone()))
            .cloned()
            .collect();

        let results = futures::future::join_all(channels.iter().map(|channel| async move {
            let notices = self.get_one_channel_notices(channel, reporter).await;
            (channel.base_url.clone(), notices)
        }))
        .await;

        results
            .into_iter()
            .flat_map(|(channel, notices)| {
                notices
                    .as_ref()
                    .clone()
                    .into_iter()
                    .map(move |notice| ChannelNoticeResult {
                        channel: channel.clone(),
                        notice,
                    })
            })
            .collect()
    }

    async fn get_one_channel_notices(
        &self,
        channel: &Channel,
        reporter: Option<&dyn Reporter>,
    ) -> Arc<Vec<ChannelNotice>> {
        if let Some(cached) = self.notices.get(&channel.base_url)
            && cached.is_fresh()
        {
            return cached.notices.clone();
        }

        // Notice requests are much smaller than repodata requests but still
        // need to be coalesced when multiple queries start simultaneously.
        let lock = self
            .notice_fetch_locks
            .entry(channel.base_url.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Another waiter may have refreshed the entry while this task waited.
        if let Some(cached) = self.notices.get(&channel.base_url)
            && cached.is_fresh()
        {
            return cached.notices.clone();
        }

        let fetched = self.fetch_channel_notices(channel, reporter).await;
        let notices = Arc::new(fetched.notices);
        let now = Instant::now();
        self.notices.insert(
            channel.base_url.clone(),
            Arc::new(CachedChannelNotices {
                notices: notices.clone(),
                refresh_at: now
                    .checked_add(fetched.ttl)
                    .unwrap_or_else(|| now + EMPTY_NOTICES_TTL),
            }),
        );
        notices
    }

    pub(super) fn report_channel_notices(
        reporter: Option<&dyn Reporter>,
        notices: &[ChannelNoticeResult],
    ) {
        if let Some(reporter) = reporter {
            for notice in notices {
                reporter.on_channel_notice(notice);
            }
        }
    }

    async fn fetch_channel_notices(
        &self,
        channel: &Channel,
        reporter: Option<&dyn Reporter>,
    ) -> NoticeFetch {
        let Ok(url) = channel.base_url.url().join(NOTICES_FILENAME) else {
            return NoticeFetch::failed();
        };

        #[cfg(not(target_arch = "wasm32"))]
        if url.scheme() == "file" {
            let Ok(path) = url.to_file_path() else {
                return NoticeFetch::failed();
            };
            return match fs_err::read(path) {
                Ok(bytes) if bytes.len() <= MAX_NOTICES_SIZE => parse_notices(&bytes),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => NoticeFetch::empty(),
                Ok(_) | Err(_) => NoticeFetch::failed(),
            };
        }

        if !matches!(url.scheme(), "http" | "https") {
            return NoticeFetch::empty();
        }

        // Notice downloads share the gateway's request budget with repodata,
        // package, and run-export downloads.
        let _request_permit = OptionFuture::from(
            self.concurrent_requests_semaphore
                .clone()
                .map(tokio::sync::Semaphore::acquire_owned),
        )
        .await
        .transpose()
        .expect("gateway request semaphore was closed");

        let request = self.client.client().get(url.clone());
        #[cfg(not(target_arch = "wasm32"))]
        let response = request.timeout(Duration::from_secs(5)).send().await;
        #[cfg(target_arch = "wasm32")]
        let response = match wasmtimer::tokio::timeout(Duration::from_secs(5), request.send()).await
        {
            Ok(response) => response,
            Err(_) => {
                tracing::debug!(url = %url.clone().redact(), "timed out fetching channel notices");
                return NoticeFetch::failed();
            }
        };
        let response = match response {
            Ok(response) if response.status() == StatusCode::NOT_FOUND => {
                return NoticeFetch::empty();
            }
            Ok(response) => match response.error_for_status() {
                Ok(response) => response,
                Err(err) => {
                    tracing::debug!(url = %url.clone().redact(), "failed to fetch channel notices: {err}");
                    return NoticeFetch::failed();
                }
            },
            Err(err) => {
                tracing::debug!(url = %url.clone().redact(), "failed to fetch channel notices: {err}");
                return NoticeFetch::failed();
            }
        };

        if response
            .content_length()
            .is_some_and(|length| length > MAX_NOTICES_SIZE as u64)
        {
            tracing::debug!(url = %url.clone().redact(), "channel notices response is too large");
            return NoticeFetch::failed();
        }

        let download = reporter
            .and_then(Reporter::download_reporter)
            .map(|download| (download, download.on_download_start(&url)));
        let mut stream = std::pin::pin!(response.byte_stream_with_progress(download));
        let mut bytes = Vec::new();
        let result = loop {
            match stream.try_next().await {
                Ok(Some(chunk))
                    if bytes
                        .len()
                        .checked_add(chunk.len())
                        .is_some_and(|size| size <= MAX_NOTICES_SIZE) =>
                {
                    bytes.extend_from_slice(&chunk);
                }
                Ok(Some(_)) | Err(_) => break NoticeFetch::failed(),
                Ok(None) => break parse_notices(&bytes),
            }
        };
        if let Some((download, index)) = download {
            download.on_download_complete(&url, index);
        }
        result
    }
}

fn parse_notices(bytes: &[u8]) -> NoticeFetch {
    #[derive(Deserialize)]
    struct RawNotices {
        #[serde(default)]
        notices: Vec<serde_json::Value>,
    }

    let Ok(raw) = serde_json::from_slice::<RawNotices>(bytes) else {
        return NoticeFetch::failed();
    };

    // A malformed notice should not hide unrelated valid notices from the
    // same channel.
    let had_entries = !raw.notices.is_empty();
    let notices: Vec<_> = raw
        .notices
        .into_iter()
        .filter_map(|notice| serde_json::from_value(notice).ok())
        .collect();
    if had_entries && notices.is_empty() {
        NoticeFetch::failed()
    } else {
        NoticeFetch::from_notices(notices)
    }
}
