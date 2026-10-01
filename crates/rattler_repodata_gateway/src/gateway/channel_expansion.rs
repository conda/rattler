//! Expands a list of channels through their CEP 42 relations into the ordered
//! channel list a query resolves against, keeping every fetched subdir around
//! for the caller.
//!
//! [`RepoDataQuery`](super::RepoDataQuery) and [`NamesQuery`](super::NamesQuery)
//! use the same [`ChannelExpander`], but expand channels as they fetch records
//! and notices rather than completing channel discovery first. Queries that only
//! need the resolved order and the subdirs' metadata use [`expand_channels`] instead.

use std::{collections::HashMap, sync::Arc};

use futures::{StreamExt, stream::FuturesUnordered};
use rattler_conda_types::{Channel, ChannelUrl, Subdir};

use super::{
    GatewayError, GatewayInner,
    boxed::{BoxFuture, box_future},
    channel_expander::{ChannelExpander, ChannelRelationsMode, ChannelRelationsWarning},
    query::{FetchErrorPolicy, apply_fetch_error_policy},
    subdir::SubdirState,
};
use crate::Reporter;

/// The result of expanding channels through their relations.
pub(super) struct ChannelExpansion {
    /// The resolved channel order, highest priority first. Without observed
    /// relations this is the deduplicated input order.
    pub order: Vec<ChannelUrl>,
    /// Every channel in `order`.
    pub channels: HashMap<ChannelUrl, Arc<Channel>>,
    /// The fetched subdir of every channel. A subdir the channel does not
    /// publish is [`SubdirState::NotFound`].
    pub subdirs: HashMap<(ChannelUrl, Subdir), Arc<SubdirState>>,
    /// The non-fatal relation warnings collected during expansion.
    pub warnings: Vec<ChannelRelationsWarning>,
}

impl ChannelExpansion {
    /// The fetched subdir for `url` and `platform`, if it was part of the
    /// expansion.
    pub fn subdir(&self, url: &ChannelUrl, platform: Subdir) -> Option<&SubdirState> {
        self.subdirs.get(&(url.clone(), platform)).map(Arc::as_ref)
    }

    /// Whether a cycle or the depth limit prevented following every relation,
    /// so `order` does not reflect the complete relation graph.
    pub fn was_cut_short(&self) -> bool {
        self.warnings.iter().any(|warning| {
            matches!(
                warning,
                ChannelRelationsWarning::CycleBroken { .. }
                    | ChannelRelationsWarning::MaxDepthExceeded { .. }
            )
        })
    }

    /// The channels in resolved order.
    pub fn ordered_channels(&self) -> impl Iterator<Item = &Arc<Channel>> + '_ {
        self.order.iter().filter_map(|url| self.channels.get(url))
    }
}

type FetchResult = Result<
    (
        ChannelUrl,
        Subdir,
        Arc<SubdirState>,
        Option<ChannelRelationsWarning>,
    ),
    GatewayError,
>;

/// Fetches the subdir of one channel and platform. A subdir the channel does
/// not publish is never an error, whichever channel asked for it; other
/// failures follow `policy`.
fn spawn_fetch(
    gateway: Arc<GatewayInner>,
    channel: Arc<Channel>,
    platform: Subdir,
    url: ChannelUrl,
    reporter: Option<Arc<dyn Reporter>>,
    policy: FetchErrorPolicy,
    existing_subdir: Option<Arc<SubdirState>>,
) -> BoxFuture<FetchResult> {
    box_future(async move {
        if let Some(subdir) = existing_subdir {
            return Ok((url, platform, subdir, None));
        }
        match gateway
            .get_or_create_subdir(&channel, platform, reporter, true)
            .await
        {
            Ok(subdir) => Ok((url, platform, subdir, None)),
            Err(GatewayError::SubdirNotFoundError(_)) => {
                Ok((url, platform, Arc::new(SubdirState::NotFound), None))
            }
            Err(err) => apply_fetch_error_policy(err, &url, platform, policy)
                .map(|(subdir, warning)| (url, platform, subdir, warning)),
        }
    })
}

/// Expands `channels` through their CEP 42 relations for `platforms`.
///
/// Fetch failures of the given channels are propagated. Failures of discovered
/// channels become warnings, or errors in [`ChannelRelationsMode::Strict`].
/// Previously observed subdirs retain their original fetch outcome, including
/// failures already isolated as warnings, when resolving a discovered origin.
pub(super) async fn expand_channels(
    gateway: &Arc<GatewayInner>,
    channels: Vec<Channel>,
    platforms: Vec<Subdir>,
    mode: ChannelRelationsMode,
    max_depth: usize,
    reporter: Option<Arc<dyn Reporter>>,
    previous: Option<&ChannelExpansion>,
) -> Result<ChannelExpansion, GatewayError> {
    let mut expander = ChannelExpander::new(mode, max_depth, platforms.clone(), reporter.clone());
    let discovered_policy = if expander.strict() {
        FetchErrorPolicy::WrapAsChannelRelationsError
    } else {
        FetchErrorPolicy::SwallowAsWarning
    };

    let mut user_order = Vec::new();
    let mut known: HashMap<ChannelUrl, Arc<Channel>> = HashMap::new();
    let mut pending: FuturesUnordered<BoxFuture<FetchResult>> = FuturesUnordered::new();
    for channel in channels {
        let (url, channel) = expander.register_user_channel(channel);
        if known.insert(url.clone(), channel.clone()).is_some() {
            continue;
        }
        user_order.push(url.clone());
        for &platform in &platforms {
            pending.push(spawn_fetch(
                gateway.clone(),
                channel.clone(),
                platform,
                url.clone(),
                reporter.clone(),
                FetchErrorPolicy::Propagate,
                previous
                    .and_then(|expansion| expansion.subdirs.get(&(url.clone(), platform)).cloned()),
            ));
        }
    }

    let mut subdirs = HashMap::new();
    while let Some(result) = pending.next().await {
        let (url, platform, subdir, warning) = result?;
        if let Some(warning) = warning {
            expander.push_warning(warning);
        }
        for (new_url, new_channel, new_platform) in expander.observe(&url, platform, &subdir)? {
            known
                .entry(new_url.clone())
                .or_insert_with(|| new_channel.clone());
            let existing_subdir = previous.and_then(|expansion| {
                expansion
                    .subdirs
                    .get(&(new_url.clone(), new_platform))
                    .cloned()
            });
            pending.push(spawn_fetch(
                gateway.clone(),
                new_channel,
                new_platform,
                new_url,
                reporter.clone(),
                discovered_policy,
                existing_subdir,
            ));
        }
        subdirs.insert((url, platform), subdir);
    }

    let order = if expander.enabled() && expander.has_observed_relations() {
        expander.finalize()?.order
    } else {
        user_order
    };

    Ok(ChannelExpansion {
        order,
        channels: known,
        subdirs,
        warnings: expander.take_warnings(),
    })
}
