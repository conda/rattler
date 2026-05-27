//! State and behavior for following CEP-42 `channel_relations` during
//! a [`RepoDataQuery`](super::query::RepoDataQuery).

use std::{collections::HashMap, sync::Arc};

use rattler_conda_types::{Channel, ChannelRelations, ChannelUrl, Platform};

use super::{
    channel_relations::{ChannelRegistry, Resolution, resolve_channel_priority},
    subdir::Subdir,
};

/// How a query should treat [CEP-42] `channel_relations` metadata
/// encountered while resolving channels.
///
/// [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelRelationsMode {
    /// Ignore declared relations; use only the user-supplied channels.
    Disabled,

    /// Follow relations recursively; cycles and failed discovery fetches
    /// log `tracing::warn!` but never fail the query.
    #[default]
    Warn,

    /// Follow relations recursively; cycles surface as
    /// [`GatewayError::ChannelRelationsError`](super::GatewayError::ChannelRelationsError).
    /// Edges overridden by the user's explicit ordering are not treated
    /// as errors (CEP says user ordering wins).
    Strict,
}

/// Tracks CEP-42 state across a query: discovered channels, the
/// declared relations gathered as subdirs resolve, and the user's
/// supplied channel order. The query executor calls
/// [`ChannelExpander::observe`] on each freshly resolved subdir to get
/// the (channel, platform) pairs it must schedule next, and
/// [`ChannelExpander::finalize`] at the end to learn the final
/// priority order.
pub(super) struct ChannelExpander {
    mode: ChannelRelationsMode,
    max_depth: usize,
    platforms: Vec<Platform>,
    user_channels: Vec<ChannelUrl>,
    discovered: HashMap<ChannelUrl, Arc<Channel>>,
    depth_of: HashMap<ChannelUrl, usize>,
    relations: HashMap<(ChannelUrl, Platform), ChannelRelations>,
    /// Descriptions of any `base`/`overrides` references that failed to
    /// parse. Surfaces as a hard error in `Strict` mode.
    parse_errors: Vec<String>,
}

impl ChannelExpander {
    pub fn new(mode: ChannelRelationsMode, max_depth: usize, platforms: Vec<Platform>) -> Self {
        Self {
            mode,
            max_depth,
            platforms,
            user_channels: Vec::new(),
            discovered: HashMap::new(),
            depth_of: HashMap::new(),
            relations: HashMap::new(),
            parse_errors: Vec::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        !matches!(self.mode, ChannelRelationsMode::Disabled)
    }

    pub fn strict(&self) -> bool {
        matches!(self.mode, ChannelRelationsMode::Strict)
    }

    pub fn platforms(&self) -> &[Platform] {
        &self.platforms
    }

    /// `true` once any subdir has contributed `channel_relations`.
    /// Callers use this to decide whether to reorder the result Vec.
    pub fn has_observed_relations(&self) -> bool {
        !self.relations.is_empty()
    }

    /// Register a user-supplied channel at depth 0. Returns the
    /// canonical URL and a shared `Arc<Channel>`. Subsequent calls for
    /// the same URL return the existing Arc and do not duplicate the
    /// channel in the priority input.
    pub fn register_user_channel(&mut self, channel: Channel) -> (ChannelUrl, Arc<Channel>) {
        let url = channel.base_url.clone();
        if let Some(existing) = self.discovered.get(&url) {
            return (url, existing.clone());
        }
        let arc = Arc::new(channel);
        self.discovered.insert(url.clone(), arc.clone());
        self.depth_of.insert(url.clone(), 0);
        self.user_channels.push(url.clone());
        (url, arc)
    }

    /// Process the relations declared by `subdir` and return any
    /// newly-discovered (url, channel, platform) triples the caller
    /// must schedule. Each new channel is fanned out over every
    /// platform the expander was configured with.
    ///
    /// In `Strict` mode this also runs an incremental check after
    /// recording the new relations: if the partial graph now contains
    /// a cycle, or any `base`/`overrides` reference fails to parse,
    /// returns `Err(ChannelRelationsError)` so the executor can abort
    /// the remaining in-flight fetches.
    pub fn observe(
        &mut self,
        channel_url: &ChannelUrl,
        platform: Platform,
        subdir: &Subdir,
    ) -> Result<Vec<(ChannelUrl, Arc<Channel>, Platform)>, super::GatewayError> {
        if !self.enabled() {
            return Ok(Vec::new());
        }
        let Some(relations) = subdir.channel_relations() else {
            return Ok(Vec::new());
        };
        if relations.is_empty() {
            // `{"channel_relations": {}}` carries no information; treating
            // it as "observed" would silently re-tier the result.
            return Ok(Vec::new());
        }
        self.relations
            .insert((channel_url.clone(), platform), relations.clone());

        let current_depth = self.depth_of.get(channel_url).copied().unwrap_or(0);
        let new_depth = current_depth + 1;
        let within_depth = current_depth < self.max_depth;

        let mut newly_discovered: Vec<(ChannelUrl, Arc<Channel>)> = Vec::new();
        for relative in [&relations.base, &relations.overrides]
            .into_iter()
            .flatten()
        {
            match resolve_channel_reference(channel_url, relative) {
                Ok(target_url) => {
                    // Take the minimum depth seen. Network completion
                    // order can otherwise record a channel at a higher
                    // depth than its shortest path, truncating expansion
                    // nondeterministically near `max_depth`.
                    let recorded = self.depth_of.get(&target_url).copied();
                    if recorded.is_none_or(|d| new_depth < d) {
                        self.depth_of.insert(target_url.clone(), new_depth);
                    }
                    // Only fan out a new fetch if we're within depth and
                    // haven't already discovered this channel.
                    if !within_depth || self.discovered.contains_key(&target_url) {
                        continue;
                    }
                    let target_channel = Arc::new(Channel::from_url(target_url.clone()));
                    self.discovered
                        .insert(target_url.clone(), target_channel.clone());
                    newly_discovered.push((target_url, target_channel));
                }
                Err(err) => {
                    let msg = format!(
                        "failed to resolve CEP-42 channel reference `{relative}` \
                         against `{channel_url}`: {err}"
                    );
                    tracing::warn!("{msg}");
                    self.parse_errors.push(msg);
                }
            }
        }

        // Incremental strict check: abort early if the partial graph
        // already contains a cycle or a parse error. Cycles in the
        // observed relations grow monotonically, so once one exists it
        // exists forever; detecting it here is just an early-exit
        // optimization over the finalize-time check.
        if let Some(msg) = self.strict_check() {
            return Err(super::GatewayError::ChannelRelationsError(msg));
        }

        let mut pairs = Vec::with_capacity(newly_discovered.len() * self.platforms.len());
        for (url, channel) in newly_discovered {
            for plat in &self.platforms {
                pairs.push((url.clone(), channel.clone(), *plat));
            }
        }
        Ok(pairs)
    }

    /// Compute the final channel priority resolution from collected
    /// relations. Idempotent across calls.
    pub fn finalize(&self) -> Resolution<ChannelUrl> {
        let registry = self.merged_relations();
        resolve_channel_priority(&self.user_channels, &registry, self.max_depth)
    }

    /// In `Strict` mode, returns `Some(message)` describing the first
    /// reason `resolution` reveals the declared relations to be
    /// malformed: an unparseable `base`/`overrides` reference or a
    /// cycle. Returns `None` in `Disabled`/`Warn` modes or when
    /// nothing is wrong.
    pub fn strict_error(&self, resolution: &Resolution<ChannelUrl>) -> Option<String> {
        if !self.strict() {
            return None;
        }
        if let Some(first) = self.parse_errors.first() {
            return Some(first.clone());
        }
        if resolution.broken_cycle_edges.is_empty() {
            return None;
        }
        let edges = resolution
            .broken_cycle_edges
            .iter()
            .map(|e| format!("`{}` -> `{}`", e.from, e.to))
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "cycle detected in CEP-42 channel relations; would need to drop: {edges}"
        ))
    }

    /// Convenience for incremental checks: computes a snapshot
    /// resolution and runs [`strict_error`](Self::strict_error)
    /// against it. Used by [`observe`](Self::observe) so callers don't
    /// have to compute the snapshot themselves.
    pub fn strict_check(&self) -> Option<String> {
        if !self.strict() {
            return None;
        }
        // Fast path: a parse error alone is enough to fail; skip the
        // graph snapshot.
        if let Some(first) = self.parse_errors.first() {
            return Some(first.clone());
        }
        let resolution = resolve_channel_priority(
            &self.user_channels,
            &self.merged_relations(),
            self.max_depth,
        );
        self.strict_error(&resolution)
    }

    /// Fold per-(channel, platform) relations into a per-channel
    /// registry. For each channel takes the first non-`None`
    /// `base`/`overrides` seen; divergent declarations across platforms
    /// log a warning. Parse errors were already caught and recorded
    /// by [`observe`](Self::observe); references that fail to parse
    /// here are silently skipped (caller surfaces them via
    /// [`strict_check`](Self::strict_check)).
    fn merged_relations(&self) -> ChannelRegistry<ChannelUrl> {
        let mut registry: ChannelRegistry<ChannelUrl> = ChannelRegistry::new();
        for ((channel_url, _platform), relations) in &self.relations {
            let entry = registry.entry(channel_url.clone()).or_insert_with(|| {
                super::channel_relations::ChannelRelations {
                    base: None,
                    overrides: None,
                }
            });
            fold_relation(
                &mut entry.base,
                relations.base.as_deref(),
                channel_url,
                "base",
            );
            fold_relation(
                &mut entry.overrides,
                relations.overrides.as_deref(),
                channel_url,
                "overrides",
            );
        }
        registry
    }
}

/// Resolve a CEP-42 channel reference against a declaring channel's
/// base URL via `Url::join`. Typical references look like
/// `../conda-forge`, `../..`, or absolute URLs. The output is
/// normalized via [`ChannelUrl`] so relative-path joins resolve at the
/// channel's parent rather than at its last path component.
fn resolve_channel_reference(
    declaring: &ChannelUrl,
    reference: &str,
) -> Result<ChannelUrl, url::ParseError> {
    let joined = declaring.url().join(reference)?;
    Ok(ChannelUrl::from(joined))
}

fn fold_relation(
    slot: &mut Option<ChannelUrl>,
    candidate: Option<&str>,
    declaring: &ChannelUrl,
    field: &str,
) {
    let Some(candidate) = candidate else { return };
    // `observe` already pushed a description to `parse_errors` for any
    // unparseable reference, so we silently skip here. That keeps this
    // method `&self`.
    let Ok(resolved) = resolve_channel_reference(declaring, candidate) else {
        return;
    };
    match slot {
        None => *slot = Some(resolved),
        Some(existing) if *existing == resolved => {}
        Some(existing) => {
            tracing::warn!(
                "CEP-42 `{field}` differs across subdirs of `{declaring}`: \
                 keeping `{existing}`, ignoring `{resolved}`"
            );
        }
    }
}
