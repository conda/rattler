//! Schedules channel discovery for record, name, and detector queries.
//!
//! [`ChannelDiscovery`] drives subdir fetches incrementally. Record queries
//! wait on its per-subdir barriers while fetching packages concurrently with
//! discovery; metadata-only queries drain the same scheduler with
//! [`expand_channels`].

use std::{collections::HashMap, sync::Arc};

use futures::{StreamExt, stream::FuturesUnordered};
use rattler_conda_types::{Channel, ChannelUrl, Subdir};

use super::{
    BarrierCell, GatewayError, GatewayInner,
    boxed::{BoxFuture, box_future},
    channel_expander::{ChannelExpander, ChannelRelationsMode, ChannelRelationsWarning},
    query::{FetchErrorPolicy, apply_fetch_error_policy},
    subdir::SubdirState,
};
use crate::Reporter;

/// The result of expanding channels through their relations.
pub(super) struct ChannelExpansion {
    /// The resolved channel order, highest priority first.
    pub order: Vec<ChannelUrl>,
    pub channels: HashMap<ChannelUrl, Arc<Channel>>,
    /// Fetched subdirs, including missing subdirs and isolated fetch failures.
    pub subdirs: HashMap<(ChannelUrl, Subdir), Arc<SubdirState>>,
    pub warnings: Vec<ChannelRelationsWarning>,
}

impl ChannelExpansion {
    pub fn subdir(&self, url: &ChannelUrl, platform: Subdir) -> Option<&SubdirState> {
        self.subdirs.get(&(url.clone(), platform)).map(Arc::as_ref)
    }

    pub fn was_cut_short(&self) -> bool {
        self.warnings.iter().any(|warning| {
            matches!(
                warning,
                ChannelRelationsWarning::CycleBroken { .. }
                    | ChannelRelationsWarning::MaxDepthExceeded { .. }
            )
        })
    }

    pub fn ordered_channels(&self) -> impl Iterator<Item = &Arc<Channel>> + '_ {
        self.order.iter().filter_map(|url| self.channels.get(url))
    }
}

pub(super) struct ScheduledSubdir {
    pub url: ChannelUrl,
    pub channel: Arc<Channel>,
    pub platform: Subdir,
    pub barrier: Arc<BarrierCell<Arc<SubdirState>>>,
}

pub(super) struct DiscoveredSubdir {
    pub url: ChannelUrl,
    pub platform: Subdir,
    pub subdir: Arc<SubdirState>,
    warning: Option<ChannelRelationsWarning>,
}

/// Owns the shared fetch queue, deduplication, error policy, and incremental
/// relation discovery. Callers attach their own work to scheduled barriers.
pub(super) struct ChannelDiscovery<'a> {
    gateway: Arc<GatewayInner>,
    reporter: Option<Arc<dyn Reporter>>,
    pub expander: ChannelExpander,
    pub pending: FuturesUnordered<BoxFuture<Result<DiscoveredSubdir, GatewayError>>>,
    scheduled: HashMap<(ChannelUrl, Subdir), Arc<BarrierCell<Arc<SubdirState>>>>,
    channels: HashMap<ChannelUrl, Arc<Channel>>,
    subdirs: HashMap<(ChannelUrl, Subdir), Arc<SubdirState>>,
    user_order: Vec<ChannelUrl>,
    previous: Option<&'a ChannelExpansion>,
    allow_missing: bool,
}

impl<'a> ChannelDiscovery<'a> {
    pub fn new(
        gateway: Arc<GatewayInner>,
        platforms: Vec<Subdir>,
        mode: ChannelRelationsMode,
        max_depth: usize,
        reporter: Option<Arc<dyn Reporter>>,
        previous: Option<&'a ChannelExpansion>,
        allow_missing: bool,
    ) -> Self {
        Self {
            gateway,
            expander: ChannelExpander::new(mode, max_depth, platforms, reporter.clone()),
            reporter,
            pending: FuturesUnordered::new(),
            scheduled: HashMap::new(),
            channels: HashMap::new(),
            subdirs: HashMap::new(),
            user_order: Vec::new(),
            previous,
            allow_missing,
        }
    }

    pub fn register_user_channel(&mut self, channel: Channel) -> (ChannelUrl, Arc<Channel>) {
        let (url, channel) = self.expander.register_user_channel(channel);
        if self.channels.insert(url.clone(), channel.clone()).is_none() {
            self.user_order.push(url.clone());
        }
        (url, channel)
    }

    pub fn schedule_user_subdir(
        &mut self,
        url: ChannelUrl,
        channel: Arc<Channel>,
        platform: Subdir,
    ) -> ScheduledSubdir {
        self.schedule(url, channel, platform, FetchErrorPolicy::Propagate)
    }

    fn schedule(
        &mut self,
        url: ChannelUrl,
        channel: Arc<Channel>,
        platform: Subdir,
        policy: FetchErrorPolicy,
    ) -> ScheduledSubdir {
        self.channels
            .entry(url.clone())
            .or_insert_with(|| channel.clone());
        let key = (url.clone(), platform);
        let barrier = if let Some(barrier) = self.scheduled.get(&key) {
            barrier.clone()
        } else {
            let barrier = Arc::new(BarrierCell::new());
            self.scheduled.insert(key.clone(), barrier.clone());
            let existing = self
                .previous
                .and_then(|previous| previous.subdirs.get(&key))
                .cloned();
            let gateway = self.gateway.clone();
            let reporter = self.reporter.clone();
            let allow_missing = self.allow_missing;
            let fetch_url = url.clone();
            let fetch_channel = channel.clone();
            let fetch_barrier = barrier.clone();
            self.pending.push(box_future(async move {
                let (subdir, warning) = if let Some(subdir) = existing {
                    (subdir, None)
                } else {
                    match gateway
                        .get_or_create_subdir(&fetch_channel, platform, reporter, true)
                        .await
                    {
                        Ok(subdir) => (subdir, None),
                        Err(GatewayError::SubdirNotFoundError(_)) if allow_missing => {
                            (Arc::new(SubdirState::NotFound), None)
                        }
                        Err(error) => {
                            apply_fetch_error_policy(error, &fetch_url, platform, policy)?
                        }
                    }
                };
                fetch_barrier
                    .set(subdir.clone())
                    .expect("subdir was set twice");
                Ok(DiscoveredSubdir {
                    url: fetch_url,
                    platform,
                    subdir,
                    warning,
                })
            }));
            barrier
        };
        ScheduledSubdir {
            url,
            channel,
            platform,
            barrier,
        }
    }

    /// Records one fetch outcome and schedules all channels it introduces.
    pub fn observe(
        &mut self,
        fetched: DiscoveredSubdir,
    ) -> Result<Vec<ScheduledSubdir>, GatewayError> {
        if let Some(warning) = fetched.warning {
            self.expander.push_warning(warning);
        }
        let pairs = self
            .expander
            .observe(&fetched.url, fetched.platform, &fetched.subdir)?;
        self.subdirs
            .insert((fetched.url, fetched.platform), fetched.subdir);
        let policy = if self.expander.strict() {
            FetchErrorPolicy::WrapAsChannelRelationsError
        } else {
            FetchErrorPolicy::SwallowAsWarning
        };
        Ok(pairs
            .into_iter()
            .map(|(url, channel, platform)| self.schedule(url, channel, platform, policy))
            .collect())
    }

    fn finish(mut self) -> Result<ChannelExpansion, GatewayError> {
        let order = if self.expander.enabled() && self.expander.has_observed_relations() {
            self.expander.finalize()?.order
        } else {
            self.user_order
        };
        Ok(ChannelExpansion {
            order,
            channels: self.channels,
            subdirs: self.subdirs,
            warnings: self.expander.take_warnings(),
        })
    }
}

/// Drains the shared discovery scheduler for metadata-only queries. Previously
/// observed subdirs retain their outcome when resolving a discovered origin.
pub(super) async fn expand_channels(
    gateway: &Arc<GatewayInner>,
    channels: Vec<Channel>,
    platforms: Vec<Subdir>,
    mode: ChannelRelationsMode,
    max_depth: usize,
    reporter: Option<Arc<dyn Reporter>>,
    previous: Option<&ChannelExpansion>,
) -> Result<ChannelExpansion, GatewayError> {
    let mut discovery = ChannelDiscovery::new(
        gateway.clone(),
        platforms.clone(),
        mode,
        max_depth,
        reporter,
        previous,
        true,
    );
    for channel in channels {
        let (url, channel) = discovery.register_user_channel(channel);
        for &platform in &platforms {
            discovery.schedule_user_subdir(url.clone(), channel.clone(), platform);
        }
    }
    while let Some(result) = discovery.pending.next().await {
        discovery.observe(result?)?;
    }
    discovery.finish()
}
