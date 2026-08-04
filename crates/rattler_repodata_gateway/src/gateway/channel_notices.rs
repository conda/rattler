//! Fetching and caching of CEP-6 channel notices.

use std::{collections::HashSet, sync::Arc, time::Duration};

use rattler_conda_types::{Channel, ChannelNotice, ChannelNotices, ChannelUrl};
use rattler_redaction::Redact;
use reqwest::StatusCode;

use crate::{Reporter, reporter::ResponseReporterExt};

use super::GatewayInner;

/// A channel notice together with the channel that published it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelNoticeResult {
    /// The channel that published the notice.
    pub channel: ChannelUrl,
    /// The published CEP-6 notice.
    pub notice: ChannelNotice,
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
            let notices = if let Some(notices) = self.notices.get(&channel.base_url) {
                notices.clone()
            } else {
                let notices = Arc::new(self.fetch_channel_notices(channel, reporter).await);
                self.notices
                    .entry(channel.base_url.clone())
                    .or_insert_with(|| notices.clone())
                    .clone()
            };
            (channel.base_url.clone(), notices)
        }))
        .await;

        let notices: Vec<_> = results
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
            .collect();

        if let Some(reporter) = reporter {
            for notice in &notices {
                reporter.on_channel_notice(notice);
            }
        }
        notices
    }

    async fn fetch_channel_notices(
        &self,
        channel: &Channel,
        reporter: Option<&dyn Reporter>,
    ) -> Vec<ChannelNotice> {
        let Ok(url) = channel.base_url.url().join("notices.json") else {
            return Vec::new();
        };

        let result = if url.scheme() == "file" {
            let Ok(path) = url.to_file_path() else {
                return Vec::new();
            };
            fs_err::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ChannelNotices>(&bytes).ok())
        } else if matches!(url.scheme(), "http" | "https") {
            let response = match self
                .client
                .client()
                .get(url.clone())
                .timeout(Duration::from_secs(5))
                .send()
                .await
            {
                Ok(response) if response.status() == StatusCode::NOT_FOUND => return Vec::new(),
                Ok(response) => match response.error_for_status() {
                    Ok(response) => response,
                    Err(err) => {
                        tracing::debug!(url = %url.clone().redact(), "failed to fetch channel notices: {err}");
                        return Vec::new();
                    }
                },
                Err(err) => {
                    tracing::debug!(url = %url.clone().redact(), "failed to fetch channel notices: {err}");
                    return Vec::new();
                }
            };

            let download = reporter
                .and_then(Reporter::download_reporter)
                .map(|download| (download, download.on_download_start(&url)));
            let bytes = response.bytes_with_progress(download).await;
            if let Some((download, index)) = download {
                download.on_download_complete(&url, index);
            }
            bytes
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ChannelNotices>(&bytes).ok())
        } else {
            None
        };

        result.map_or_else(Vec::new, |notices| notices.notices)
    }
}
