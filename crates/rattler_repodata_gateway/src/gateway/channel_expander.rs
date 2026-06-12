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

    /// Follow relations recursively. Cycles, malformed metadata, and
    /// failed discovery fetches surface as
    /// [`ChannelRelationsWarning`]s on the query output; nothing fails
    /// the query.
    #[default]
    Warn,

    /// Follow relations recursively; cycles surface as
    /// [`GatewayError::ChannelRelationsError`](super::GatewayError::ChannelRelationsError).
    /// Edges overridden by the user's explicit ordering are not treated
    /// as errors (CEP says user ordering wins).
    Strict,
}

/// One non-fatal issue surfaced while resolving CEP-42
/// `channel_relations`. In [`ChannelRelationsMode::Warn`] these accumulate
/// on the query output instead of failing the query; the caller decides
/// whether to log them, surface them to the user, or ignore them.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ChannelRelationsWarning {
    /// A `base`/`overrides` reference failed to resolve against its
    /// declaring channel's URL.
    #[error(
        "failed to resolve CEP-42 channel reference `{reference}` against `{declaring}`: {error}"
    )]
    UnparsableReference {
        /// Channel that declared the offending reference.
        declaring: ChannelUrl,
        /// The raw reference string from the channel's metadata.
        reference: String,
        /// `url::ParseError` message.
        error: String,
    },

    /// Different subdirs of the same channel declared conflicting
    /// `base`/`overrides` values; the first one seen was kept.
    #[error(
        "CEP-42 `{field}` differs across subdirs of `{declaring}`: \
         keeping `{kept}`, ignoring `{ignored}`"
    )]
    DivergentDeclaration {
        /// Which field disagreed: `"base"` or `"overrides"`.
        field: &'static str,
        /// Channel whose subdirs disagreed.
        declaring: ChannelUrl,
        /// Value kept by the resolver.
        kept: ChannelUrl,
        /// Value seen in another subdir and ignored.
        ignored: ChannelUrl,
    },

    /// A transitively discovered channel could not be fetched. In
    /// [`ChannelRelationsMode::Warn`] the subdir is treated as empty.
    #[error(
        "failed to fetch transitively discovered channel `{url}` \
         for platform `{platform}`: {error}"
    )]
    DiscoveryFetchFailed {
        /// Channel that failed to fetch.
        url: ChannelUrl,
        /// Platform whose subdir failed to fetch.
        platform: Platform,
        /// Display-formatted [`GatewayError`](super::GatewayError).
        error: String,
    },

    /// One or more channel-relation edges were dropped to break a
    /// cycle in the declared relations.
    #[error(
        "dropped {} CEP-42 relation edge(s) to break a cycle: {}",
        broken_edges.len(),
        format_broken_edges(broken_edges),
    )]
    CycleBroken {
        /// Each dropped edge as a `(from, to)` pair.
        broken_edges: Vec<(ChannelUrl, ChannelUrl)>,
    },
}

fn format_broken_edges(edges: &[(ChannelUrl, ChannelUrl)]) -> String {
    edges
        .iter()
        .map(|(from, to)| format!("`{from}` -> `{to}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Tracks CEP-42 state across a query: discovered channels, the
/// declared relations gathered as subdirs resolve, the user's
/// supplied channel order, and any non-fatal warnings observed along
/// the way. The query executor calls
/// [`ChannelExpander::observe`] on each freshly resolved subdir to get
/// the (channel, platform) pairs it must schedule next,
/// [`ChannelExpander::finalize`] at the end to learn the final
/// priority order, and [`ChannelExpander::take_warnings`] to attach
/// the accumulated warnings to the query output.
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
    warnings: Vec<ChannelRelationsWarning>,
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
            warnings: Vec::new(),
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

    /// Push an externally-observed warning (used by the executor to
    /// surface fetch failures gathered from spawned futures).
    pub fn push_warning(&mut self, warning: ChannelRelationsWarning) {
        self.warnings.push(warning);
    }

    /// Drain the accumulated warnings. Idempotent: subsequent calls
    /// return an empty vec.
    pub fn take_warnings(&mut self) -> Vec<ChannelRelationsWarning> {
        std::mem::take(&mut self.warnings)
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
                    self.parse_errors.push(msg);
                    self.warnings
                        .push(ChannelRelationsWarning::UnparsableReference {
                            declaring: channel_url.clone(),
                            reference: relative.clone(),
                            error: err.to_string(),
                        });
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
    /// relations. Records a [`ChannelRelationsWarning::CycleBroken`]
    /// when the resolver had to drop back-edges, and one
    /// [`ChannelRelationsWarning::DivergentDeclaration`] per channel
    /// whose subdirs disagreed.
    pub fn finalize(&mut self) -> Resolution<ChannelUrl> {
        let registry = self.merged_relations_recording_divergences();
        let resolution = resolve_channel_priority(&self.user_channels, &registry, self.max_depth);
        if !resolution.broken_cycle_edges.is_empty() {
            let broken_edges = resolution
                .broken_cycle_edges
                .iter()
                .map(|e| (e.from.clone(), e.to.clone()))
                .collect();
            self.warnings
                .push(ChannelRelationsWarning::CycleBroken { broken_edges });
        }
        resolution
    }

    /// In `Strict` mode, returns `Some(message)` describing the first
    /// reason `resolution` reveals the declared relations to be
    /// malformed: an unparsable `base`/`overrides` reference or a
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
    /// `base`/`overrides` seen; divergent declarations are silently
    /// dropped here so this method can be called from `&self` contexts
    /// (e.g. the incremental strict check) without recording duplicate
    /// warnings.
    fn merged_relations(&self) -> ChannelRegistry<ChannelUrl> {
        let mut registry: ChannelRegistry<ChannelUrl> = ChannelRegistry::new();
        for ((channel_url, _platform), relations) in &self.relations {
            let entry = registry.entry(channel_url.clone()).or_insert_with(|| {
                super::channel_relations::ChannelRelations {
                    base: None,
                    overrides: None,
                }
            });
            fold_relation(&mut entry.base, relations.base.as_deref(), channel_url);
            fold_relation(
                &mut entry.overrides,
                relations.overrides.as_deref(),
                channel_url,
            );
        }
        registry
    }

    /// Same as [`merged_relations`](Self::merged_relations) but also
    /// pushes a [`ChannelRelationsWarning::DivergentDeclaration`] for
    /// each `(channel, field)` whose subdirs disagreed. Called exactly
    /// once at finalize time.
    fn merged_relations_recording_divergences(&mut self) -> ChannelRegistry<ChannelUrl> {
        let mut registry: ChannelRegistry<ChannelUrl> = ChannelRegistry::new();
        let mut new_warnings: Vec<ChannelRelationsWarning> = Vec::new();
        for ((channel_url, _platform), relations) in &self.relations {
            let entry = registry.entry(channel_url.clone()).or_insert_with(|| {
                super::channel_relations::ChannelRelations {
                    base: None,
                    overrides: None,
                }
            });
            fold_relation_recording(
                &mut entry.base,
                relations.base.as_deref(),
                channel_url,
                "base",
                &mut new_warnings,
            );
            fold_relation_recording(
                &mut entry.overrides,
                relations.overrides.as_deref(),
                channel_url,
                "overrides",
                &mut new_warnings,
            );
        }
        self.warnings.append(&mut new_warnings);
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

fn fold_relation(slot: &mut Option<ChannelUrl>, candidate: Option<&str>, declaring: &ChannelUrl) {
    let Some(candidate) = candidate else { return };
    // `observe` already pushed a description to `parse_errors` for any
    // unparsable reference, so we silently skip here.
    let Ok(resolved) = resolve_channel_reference(declaring, candidate) else {
        return;
    };
    if slot.is_none() {
        *slot = Some(resolved);
    }
}

fn fold_relation_recording(
    slot: &mut Option<ChannelUrl>,
    candidate: Option<&str>,
    declaring: &ChannelUrl,
    field: &'static str,
    warnings: &mut Vec<ChannelRelationsWarning>,
) {
    let Some(candidate) = candidate else { return };
    let Ok(resolved) = resolve_channel_reference(declaring, candidate) else {
        return;
    };
    match slot {
        None => *slot = Some(resolved),
        Some(existing) if *existing == resolved => {}
        Some(existing) => {
            warnings.push(ChannelRelationsWarning::DivergentDeclaration {
                field,
                declaring: declaring.clone(),
                kept: existing.clone(),
                ignored: resolved,
            });
        }
    }
}
