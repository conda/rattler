//! State and behavior for following CEP-42 `channel_relations` during
//! a [`RepoDataQuery`](super::query::RepoDataQuery).

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use rattler_conda_types::{Channel, ChannelRelations, ChannelUrl, Platform};

use super::{
    channel_relations::{EdgeSource, PriorityEdge, Resolution, resolve_channel_priority},
    subdir::Subdir,
};

/// How a query should treat [CEP-42] `channel_relations` metadata
/// encountered while resolving channels.
///
/// [CEP-42] requires that cycles and malformed metadata abort
/// resolution. `Strict` matches that requirement. The default `Warn`
/// mode is a deliberate non-compliant lenient fallback: cycles,
/// malformed references, and failed discovery fetches degrade the
/// result rather than failing the whole query, and the caller
/// receives them as [`ChannelRelationsWarning`]s on the query output.
///
/// [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelRelationsMode {
    /// Ignore declared relations; use only the user-supplied channels.
    /// Equivalent to setting `channel_relations_max_depth(0)`.
    Disabled,

    /// Follow relations recursively, but tolerate problems. Cycles,
    /// malformed metadata, depth-exceeded chains, and failed
    /// discovery fetches surface as [`ChannelRelationsWarning`]s on
    /// the query output instead of aborting. Default. Deviates from
    /// CEP-42 in that the latter mandates aborting on cycles and
    /// malformed metadata.
    #[default]
    Warn,

    /// Follow relations recursively and abort on any violation
    /// (cycles, malformed references, depth exceeded,
    /// `base == overrides`, self-relations). CEP-42 compliant.
    /// Surfaces violations as
    /// [`GatewayError::ChannelRelationsError`](super::GatewayError::ChannelRelationsError)
    /// so the executor can cancel the in-flight fetches.
    Strict,
}

/// One non-fatal issue surfaced while resolving CEP-42
/// `channel_relations`. In [`ChannelRelationsMode::Warn`] these
/// accumulate on the query output instead of failing the query; the
/// caller decides whether to log them, surface them to the user, or
/// ignore them. In [`ChannelRelationsMode::Strict`] each one (except
/// [`UserOrderConflict`](ChannelRelationsWarning::UserOrderConflict),
/// which the CEP sanctions) is instead translated into a
/// [`GatewayError::ChannelRelationsError`](super::GatewayError::ChannelRelationsError).
#[derive(Debug, Clone, thiserror::Error)]
pub enum ChannelRelationsWarning {
    /// A `base`/`overrides` reference is not a valid CEP-42 relative
    /// path (it must start with `../`). The reference is dropped.
    #[error(
        "malformed CEP-42 reference `{reference}` declared by `{declaring}`: \
         must be a relative path starting with `../`"
    )]
    InvalidReferenceSyntax {
        /// Channel that declared the offending reference.
        declaring: ChannelUrl,
        /// The raw reference string from the channel's metadata.
        reference: String,
    },

    /// A CEP-42 reference is shaped like a valid relative path but
    /// fails to resolve against the declaring channel's URL.
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

    /// A channel declared `base` and `overrides` resolving to the
    /// same channel URL. CEP-42 forbids this; both references are
    /// dropped.
    #[error(
        "channel `{declaring}` declares the same target `{target}` as both `base` and `overrides`"
    )]
    BaseAndOverridesSameTarget {
        /// Channel that declared the contradiction.
        declaring: ChannelUrl,
        /// Channel both `base` and `overrides` resolved to.
        target: ChannelUrl,
    },

    /// A channel declared a `base` or `overrides` that resolves to
    /// itself. CEP-42 forbids this; the reference is dropped.
    #[error("channel `{declaring}` declares itself as `{field}`")]
    SelfRelation {
        /// Channel that declared the self-relation.
        declaring: ChannelUrl,
        /// Which field self-referenced: `"base"` or `"overrides"`.
        field: &'static str,
    },

    /// A relation chain reached the configured depth limit and was
    /// truncated. CEP-42 says this should abort resolution; `Warn`
    /// mode tolerates it. Reported at finalize time, when the depth
    /// of every channel is final regardless of fetch completion
    /// order.
    #[error(
        "CEP-42 relation chain exceeded `channel_relations_max_depth` ({max_depth}) at `{declaring}`; \
         the reference `{reference}` was not followed"
    )]
    MaxDepthExceeded {
        /// Channel whose relation would have crossed the depth limit.
        declaring: ChannelUrl,
        /// The reference that was not followed.
        reference: String,
        /// The configured depth limit.
        max_depth: usize,
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

    /// A relation edge was dropped because it contradicts the
    /// explicit user-supplied channel order. Never fatal: CEP-42
    /// says the user's ordering wins.
    #[error(
        "CEP-42 relation `{from}` -> `{to}` contradicts the explicit \
         channel order and was ignored"
    )]
    UserOrderConflict {
        /// Channel the dropped edge ranked higher.
        from: ChannelUrl,
        /// Channel the dropped edge ranked lower.
        to: ChannelUrl,
    },

    /// One or more channel-relation edges were dropped to break a
    /// cycle in the declared relations. CEP-42 says cycles must abort
    /// resolution; `Warn` mode tolerates them by dropping back-edges.
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

/// Tracks CEP-42 state across a query: discovered channels, the raw
/// relations gathered as subdirs resolve, the user's supplied channel
/// order, and any non-fatal warnings observed along the way.
///
/// The query executor calls [`ChannelExpander::observe`] on each
/// freshly resolved subdir to get the (channel, platform) pairs it
/// must schedule next, [`ChannelExpander::finalize`] at the end to
/// learn the final priority order, and
/// [`ChannelExpander::take_warnings`] to attach the accumulated
/// warnings to the query output.
///
/// The final state is independent of the order in which subdir
/// fetches complete: raw relations are kept per channel and
/// re-derived whenever a shorter path lowers a channel's depth
/// (relaxation), and depth-limit refusals are only reported at
/// finalize time when every depth is final.
pub(super) struct ChannelExpander {
    mode: ChannelRelationsMode,
    max_depth: usize,
    platforms: Vec<Platform>,
    user_channels: Vec<ChannelUrl>,
    discovered: HashMap<ChannelUrl, Arc<Channel>>,
    /// Shortest known relation-hop distance from any user channel.
    /// User channels are at depth 0.
    depth_of: HashMap<ChannelUrl, usize>,
    /// Raw relations observed per declaring channel, deduplicated.
    /// Distinct subdirs may declare distinct relations; all of them
    /// contribute edges. Kept raw so relaxation can re-derive
    /// references that an earlier, deeper depth refused.
    relations_of: HashMap<ChannelUrl, Vec<ChannelRelations>>,
    /// Deduplicated relation edges in insertion order. Edge counts
    /// are tiny (a couple per declaring channel), so linear dedup on
    /// the Vec beats maintaining a mirror set.
    edges: Vec<PriorityEdge<ChannelUrl>>,
    warnings: Vec<ChannelRelationsWarning>,
    /// Rendered messages of already-recorded warnings; suppresses
    /// duplicates from multi-platform observation and relaxation
    /// re-processing.
    emitted: HashSet<String>,
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
            relations_of: HashMap::new(),
            edges: Vec::new(),
            warnings: Vec::new(),
            emitted: HashSet::new(),
        }
    }

    /// `true` when relations should be followed. `max_depth == 0` is
    /// equivalent to [`ChannelRelationsMode::Disabled`] regardless of
    /// the mode the caller selected.
    pub fn enabled(&self) -> bool {
        !matches!(self.mode, ChannelRelationsMode::Disabled) && self.max_depth > 0
    }

    pub fn strict(&self) -> bool {
        matches!(self.mode, ChannelRelationsMode::Strict)
    }

    pub fn platforms(&self) -> &[Platform] {
        &self.platforms
    }

    /// `true` once any subdir has contributed a valid relation edge.
    /// Callers use this to decide whether to reorder the result Vec.
    pub fn has_observed_relations(&self) -> bool {
        !self.edges.is_empty()
    }

    /// Record a warning unless an identical one was already recorded
    /// (multi-platform subdirs of one channel would otherwise report
    /// every problem once per platform).
    pub fn push_warning(&mut self, warning: ChannelRelationsWarning) {
        if self.emitted.insert(warning.to_string()) {
            self.warnings.push(warning);
        }
    }

    /// Route a violation according to the mode: fatal in `Strict`,
    /// deduplicated warning otherwise.
    fn report(&mut self, warning: ChannelRelationsWarning) -> Result<(), super::GatewayError> {
        if self.strict() {
            return Err(super::GatewayError::ChannelRelationsError(
                warning.to_string(),
            ));
        }
        self.push_warning(warning);
        Ok(())
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

    /// Process the relations declared by `subdir` for one
    /// `(channel, platform)` and return any newly-discovered
    /// `(url, channel, platform)` triples the caller must schedule.
    /// Each new channel is fanned out over every platform the
    /// expander was configured with.
    ///
    /// In `Strict` mode a violation (invalid reference, self-relation,
    /// `base == overrides`, cycle) returns `Err(ChannelRelationsError)`
    /// so the executor can abort the remaining in-flight fetches.
    /// Depth-limit violations are deferred to [`finalize`](Self::finalize),
    /// where depths no longer depend on fetch completion order.
    pub fn observe(
        &mut self,
        channel_url: &ChannelUrl,
        _platform: Platform,
        subdir: &Subdir,
    ) -> Result<Vec<(ChannelUrl, Arc<Channel>, Platform)>, super::GatewayError> {
        if !self.enabled() {
            return Ok(Vec::new());
        }
        let Some(relations) = subdir.channel_relations() else {
            return Ok(Vec::new());
        };
        if relations.is_empty() {
            // `{"channel_relations": {}}` carries no information.
            return Ok(Vec::new());
        }

        // Store the raw declaration; identical declarations from other
        // platforms of the same channel are processed only once.
        let entries = self.relations_of.entry(channel_url.clone()).or_default();
        if entries.contains(relations) {
            return Ok(Vec::new());
        }
        entries.push(relations.clone());

        let edges_before = self.edges.len();
        let mut newly_discovered: Vec<(ChannelUrl, Arc<Channel>)> = Vec::new();
        self.relax(channel_url.clone(), &mut newly_discovered)?;

        // Incremental strict cycle check; edges grow monotonically, so
        // a cycle now is a cycle at finalize. Skipped when this
        // observation added no edge.
        if self.strict() && self.edges.len() > edges_before {
            self.strict_cycle_check()?;
        }

        let mut pairs = Vec::with_capacity(newly_discovered.len() * self.platforms.len());
        for (url, channel) in newly_discovered {
            for plat in &self.platforms {
                pairs.push((url.clone(), channel.clone(), *plat));
            }
        }
        Ok(pairs)
    }

    /// Re-derive edges starting from `start`, relaxing depths: when a
    /// target's known depth decreases, its stored relations are
    /// re-processed so references an earlier (deeper) pass refused
    /// are picked up. This makes the final edge set and depths
    /// independent of subdir fetch completion order.
    fn relax(
        &mut self,
        start: ChannelUrl,
        newly_discovered: &mut Vec<(ChannelUrl, Arc<Channel>)>,
    ) -> Result<(), super::GatewayError> {
        let mut worklist = vec![start];
        while let Some(declaring) = worklist.pop() {
            let Some(entries) = self.relations_of.get(&declaring) else {
                continue;
            };
            let entries = entries.clone();
            let depth = self.depth_of.get(&declaring).copied().unwrap_or(0);
            for relations in &entries {
                self.derive_edges(
                    &declaring,
                    depth,
                    relations,
                    newly_discovered,
                    &mut worklist,
                )?;
            }
        }
        Ok(())
    }

    /// Derive priority edges from one raw declaration of `declaring`
    /// at `depth`, discovering targets and scheduling relaxation of
    /// channels whose depth improves.
    fn derive_edges(
        &mut self,
        declaring: &ChannelUrl,
        depth: usize,
        relations: &ChannelRelations,
        newly_discovered: &mut Vec<(ChannelUrl, Arc<Channel>)>,
        worklist: &mut Vec<ChannelUrl>,
    ) -> Result<(), super::GatewayError> {
        let base = self.resolve_field(declaring, relations.base.as_deref())?;
        let overrides = self.resolve_field(declaring, relations.overrides.as_deref())?;

        // base == overrides target is malformed; drop both references
        // so the contradictory declaration cannot influence priority.
        if let (Some(b), Some(o)) = (&base, &overrides)
            && b == o
        {
            self.report(ChannelRelationsWarning::BaseAndOverridesSameTarget {
                declaring: declaring.clone(),
                target: b.clone(),
            })?;
            return Ok(());
        }

        for (source, target) in [(EdgeSource::Base, base), (EdgeSource::Override, overrides)] {
            let Some(target) = target else { continue };

            if &target == declaring {
                self.report(ChannelRelationsWarning::SelfRelation {
                    declaring: declaring.clone(),
                    field: field_name(source),
                })?;
                continue;
            }

            // Depth-limit refusal is silent here; finalize reports it
            // once depths are final.
            if depth + 1 > self.max_depth {
                continue;
            }

            let edge = match source {
                EdgeSource::Base => PriorityEdge {
                    from: target.clone(),
                    to: declaring.clone(),
                    source,
                },
                EdgeSource::Override => PriorityEdge {
                    from: declaring.clone(),
                    to: target.clone(),
                    source,
                },
                EdgeSource::User => unreachable!("relations never produce user edges"),
            };
            if !self.edges.contains(&edge) {
                self.edges.push(edge);
            }

            let new_depth = depth + 1;
            match self.depth_of.get(&target).copied() {
                Some(known) if new_depth < known => {
                    // Shorter path found: relax the target so refs its
                    // earlier, deeper pass refused get re-derived.
                    self.depth_of.insert(target.clone(), new_depth);
                    worklist.push(target.clone());
                }
                Some(_) => {}
                None => {
                    self.depth_of.insert(target.clone(), new_depth);
                    let channel = Arc::new(Channel::from_url(target.clone()));
                    self.discovered.insert(target.clone(), channel.clone());
                    newly_discovered.push((target, channel));
                }
            }
        }
        Ok(())
    }

    /// Resolve one `base` or `overrides` reference, surfacing
    /// invalid-syntax / unparsable-target failures per the mode.
    fn resolve_field(
        &mut self,
        declaring: &ChannelUrl,
        reference: Option<&str>,
    ) -> Result<Option<ChannelUrl>, super::GatewayError> {
        let Some(reference) = reference else {
            return Ok(None);
        };
        match validate_and_resolve(declaring, reference) {
            Ok(url) => Ok(Some(url)),
            Err(ResolveError::InvalidSyntax) => {
                self.report(ChannelRelationsWarning::InvalidReferenceSyntax {
                    declaring: declaring.clone(),
                    reference: reference.to_string(),
                })?;
                Ok(None)
            }
            Err(ResolveError::Unparsable(err)) => {
                self.report(ChannelRelationsWarning::UnparsableReference {
                    declaring: declaring.clone(),
                    reference: reference.to_string(),
                    error: err.to_string(),
                })?;
                Ok(None)
            }
        }
    }

    /// Compute the final channel priority resolution.
    ///
    /// Reports depth-limit refusals (now that every depth is final),
    /// user-order conflicts, and broken cycles; in `Strict` mode the
    /// fatal ones among these return `Err`. The resolution inputs are
    /// canonically ordered so the result is identical run to run.
    pub fn finalize(&mut self) -> Result<Resolution<ChannelUrl>, super::GatewayError> {
        self.report_depth_refusals()?;

        let mut nodes = self.user_channels.clone();
        let mut rest: Vec<ChannelUrl> = self
            .discovered
            .keys()
            .filter(|url| !self.user_channels.contains(url))
            .cloned()
            .collect();
        rest.sort();
        nodes.extend(rest);

        let mut edges = self.edges.clone();
        edges.sort();

        let resolution = resolve_channel_priority(&self.user_channels, &nodes, &edges);

        for edge in &resolution.ignored_edges {
            // Never fatal: CEP-42 says the explicit user order wins.
            self.push_warning(ChannelRelationsWarning::UserOrderConflict {
                from: edge.from.clone(),
                to: edge.to.clone(),
            });
        }
        if !resolution.broken_cycle_edges.is_empty() {
            let broken_edges = resolution
                .broken_cycle_edges
                .iter()
                .map(|e| (e.from.clone(), e.to.clone()))
                .collect();
            self.report(ChannelRelationsWarning::CycleBroken { broken_edges })?;
        }
        Ok(resolution)
    }

    /// Report every reference that stayed unfollowed because its
    /// declaring channel sits at the depth limit. Runs at finalize so
    /// the outcome does not depend on fetch completion order: a
    /// channel's depth can only have decreased since the reference
    /// was first seen, and relaxation already re-derived references
    /// whose depth improved.
    fn report_depth_refusals(&mut self) -> Result<(), super::GatewayError> {
        let mut refused: Vec<(ChannelUrl, Vec<ChannelRelations>)> = self
            .relations_of
            .iter()
            .filter(|(url, _)| {
                let depth = self.depth_of.get(*url).copied().unwrap_or(0);
                depth + 1 > self.max_depth
            })
            .map(|(url, entries)| (url.clone(), entries.clone()))
            .collect();
        refused.sort_by(|a, b| a.0.cmp(&b.0));

        for (declaring, entries) in refused {
            for relations in entries {
                let base = relations
                    .base
                    .as_deref()
                    .and_then(|r| validate_and_resolve(&declaring, r).ok());
                let overrides = relations
                    .overrides
                    .as_deref()
                    .and_then(|r| validate_and_resolve(&declaring, r).ok());
                // Skip references already diagnosed as malformed.
                if let (Some(b), Some(o)) = (&base, &overrides)
                    && b == o
                {
                    continue;
                }
                for (reference, target) in [
                    (relations.base.as_deref(), base),
                    (relations.overrides.as_deref(), overrides),
                ] {
                    let (Some(reference), Some(target)) = (reference, target) else {
                        continue;
                    };
                    if target == declaring {
                        continue;
                    }
                    self.report(ChannelRelationsWarning::MaxDepthExceeded {
                        declaring: declaring.clone(),
                        reference: reference.to_string(),
                        max_depth: self.max_depth,
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Fail fast on a cycle in the partial graph. Node order is
    /// irrelevant for cycle detection, so the (cheap) user-channel
    /// list stands in for the full node list; edge endpoints are
    /// indexed automatically.
    fn strict_cycle_check(&self) -> Result<(), super::GatewayError> {
        let resolution =
            resolve_channel_priority(&self.user_channels, &self.user_channels, &self.edges);
        if resolution.broken_cycle_edges.is_empty() {
            return Ok(());
        }
        let edges: Vec<(ChannelUrl, ChannelUrl)> = resolution
            .broken_cycle_edges
            .iter()
            .map(|e| (e.from.clone(), e.to.clone()))
            .collect();
        Err(super::GatewayError::ChannelRelationsError(format!(
            "cycle detected in CEP-42 channel relations; would need to drop: {}",
            format_broken_edges(&edges)
        )))
    }

    /// Map every transitively discovered channel to the first user
    /// channel in `user_priority` that reaches it via declared
    /// relations. Deterministic given the final edge set; the
    /// executor uses it to slot discovered channels next to the
    /// user channel that introduced them.
    pub fn anchors(&self, user_priority: &[ChannelUrl]) -> HashMap<ChannelUrl, ChannelUrl> {
        // Discovery arcs run declaring -> target: a Base edge stores
        // (from: target, to: declaring), an Override edge stores
        // (from: declaring, to: target).
        let mut adjacency: HashMap<&ChannelUrl, Vec<&ChannelUrl>> = HashMap::new();
        for edge in &self.edges {
            match edge.source {
                EdgeSource::Base => adjacency.entry(&edge.to).or_default().push(&edge.from),
                EdgeSource::Override => adjacency.entry(&edge.from).or_default().push(&edge.to),
                EdgeSource::User => {}
            }
        }

        let user_set: HashSet<&ChannelUrl> = self.user_channels.iter().collect();
        let mut anchors: HashMap<ChannelUrl, ChannelUrl> = HashMap::new();
        for user in user_priority {
            let mut stack: Vec<&ChannelUrl> = vec![user];
            while let Some(current) = stack.pop() {
                let Some(targets) = adjacency.get(current) else {
                    continue;
                };
                for &target in targets {
                    // User channels anchor to themselves; do not
                    // traverse through them (their own BFS pass
                    // claims their descendants).
                    if user_set.contains(target) {
                        continue;
                    }
                    if !anchors.contains_key(target) {
                        anchors.insert(target.clone(), user.clone());
                        stack.push(target);
                    }
                }
            }
        }
        anchors
    }
}

fn field_name(source: EdgeSource) -> &'static str {
    match source {
        EdgeSource::Base => "base",
        EdgeSource::Override => "overrides",
        EdgeSource::User => unreachable!("relations never produce user edges"),
    }
}

#[derive(Debug)]
enum ResolveError {
    /// Reference is not a valid CEP-42 relative path (does not start
    /// with `../`).
    InvalidSyntax,
    /// Reference shape looks valid but `Url::join` failed.
    Unparsable(url::ParseError),
}

/// Validate that `reference` is a CEP-42-compliant relative path and
/// resolve it against `declaring`. CEP-42 mandates that references be
/// relative paths starting with `../`; absolute URLs, `./foo`, `foo`,
/// `/foo`, `?x`, etc. are all rejected to prevent malicious metadata
/// from pointing at attacker-controlled URLs.
fn validate_and_resolve(
    declaring: &ChannelUrl,
    reference: &str,
) -> Result<ChannelUrl, ResolveError> {
    let trimmed = reference.trim();
    if !is_valid_cep42_reference(trimmed) {
        return Err(ResolveError::InvalidSyntax);
    }
    let joined = declaring
        .url()
        .join(trimmed)
        .map_err(ResolveError::Unparsable)?;
    Ok(ChannelUrl::from(joined))
}

/// CEP-42 requires that `base` and `overrides` be relative paths
/// starting with `../`. No scheme, no leading `/`, no query, no
/// fragment, and no empty path segments (one trailing `/` is
/// allowed).
fn is_valid_cep42_reference(reference: &str) -> bool {
    if reference.is_empty() {
        return false;
    }
    if !reference.starts_with("../") && reference != ".." {
        return false;
    }
    if reference.contains('?') || reference.contains('#') {
        return false;
    }
    if reference.contains("://") {
        return false;
    }
    // Every segment must be non-empty; only the final segment may be
    // empty (a single trailing slash).
    let segments: Vec<&str> = reference.split('/').collect();
    segments
        .iter()
        .enumerate()
        .all(|(i, segment)| !segment.is_empty() || i == segments.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn chan(s: &str) -> ChannelUrl {
        ChannelUrl::from(Url::parse(s).unwrap())
    }

    #[test]
    fn rejects_absolute_url_reference() {
        let declaring = chan("https://example.com/bioconda/");
        let err = validate_and_resolve(&declaring, "https://evil.example/channel").unwrap_err();
        assert!(matches!(err, ResolveError::InvalidSyntax), "{err:?}");
    }

    #[test]
    fn rejects_plain_name_reference() {
        let declaring = chan("https://example.com/bioconda/");
        assert!(matches!(
            validate_and_resolve(&declaring, "conda-forge").unwrap_err(),
            ResolveError::InvalidSyntax
        ));
    }

    #[test]
    fn rejects_dot_slash_reference() {
        let declaring = chan("https://example.com/bioconda/");
        assert!(matches!(
            validate_and_resolve(&declaring, "./foo").unwrap_err(),
            ResolveError::InvalidSyntax
        ));
    }

    #[test]
    fn rejects_absolute_path_reference() {
        let declaring = chan("https://example.com/bioconda/");
        assert!(matches!(
            validate_and_resolve(&declaring, "/foo").unwrap_err(),
            ResolveError::InvalidSyntax
        ));
    }

    #[test]
    fn rejects_empty_reference() {
        let declaring = chan("https://example.com/bioconda/");
        assert!(matches!(
            validate_and_resolve(&declaring, "").unwrap_err(),
            ResolveError::InvalidSyntax
        ));
    }

    #[test]
    fn rejects_query_only_reference() {
        let declaring = chan("https://example.com/bioconda/");
        assert!(matches!(
            validate_and_resolve(&declaring, "?x=1").unwrap_err(),
            ResolveError::InvalidSyntax
        ));
    }

    #[test]
    fn rejects_double_slash_in_reference() {
        let declaring = chan("https://example.com/bioconda/");
        assert!(matches!(
            validate_and_resolve(&declaring, "../..//conda-forge").unwrap_err(),
            ResolveError::InvalidSyntax
        ));
    }

    /// Trailing `//` produces an empty path segment and must be
    /// rejected like an internal one; only a single trailing slash is
    /// allowed.
    #[test]
    fn rejects_trailing_double_slash() {
        let declaring = chan("https://example.com/bioconda/");
        for bad in ["..//", "../..//", "..///", "../a//"] {
            assert!(
                matches!(
                    validate_and_resolve(&declaring, bad).unwrap_err(),
                    ResolveError::InvalidSyntax
                ),
                "`{bad}` must be rejected"
            );
        }
    }

    #[test]
    fn accepts_dotdot_slash_relative() {
        let declaring = chan("https://example.com/bioconda/");
        let resolved = validate_and_resolve(&declaring, "../conda-forge").unwrap();
        assert_eq!(resolved.url().as_str(), "https://example.com/conda-forge/");
    }

    #[test]
    fn accepts_trailing_single_slash() {
        let declaring = chan("https://example.com/bioconda/");
        let resolved = validate_and_resolve(&declaring, "../conda-forge/").unwrap();
        assert_eq!(resolved.url().as_str(), "https://example.com/conda-forge/");
    }

    #[test]
    fn accepts_dotdot_only() {
        let declaring = chan("https://example.com/scope/bioconda/");
        let resolved = validate_and_resolve(&declaring, "..").unwrap();
        assert_eq!(resolved.url().as_str(), "https://example.com/scope/");
    }

    #[test]
    fn accepts_nested_dotdot() {
        let declaring = chan("https://example.com/a/b/c/");
        let resolved = validate_and_resolve(&declaring, "../../x").unwrap();
        assert_eq!(resolved.url().as_str(), "https://example.com/a/x/");
    }

    #[test]
    fn accepts_file_url_reference() {
        let declaring = chan("file:///tmp/repo/bioconda/");
        let resolved = validate_and_resolve(&declaring, "../conda-forge").unwrap();
        assert_eq!(resolved.url().as_str(), "file:///tmp/repo/conda-forge/");
    }

    /// The final expander state must not depend on the order in which
    /// subdirs were observed. Exercises the relaxation path: a channel
    /// first reached at the depth limit refuses its outgoing
    /// reference; a later shorter path must re-derive it.
    #[test]
    fn relaxation_makes_depth_refusals_order_independent() {
        let a = chan("https://example.com/a/");
        let b = chan("https://example.com/b/");
        let c = chan("https://example.com/c/");
        let d = chan("https://example.com/d/");

        // a -> m -> c (c at depth 2), b -> c (c at depth 1),
        // c -> d. max_depth = 2, so d is only reachable when the
        // shorter path through b is taken into account.
        let m = chan("https://example.com/m/");
        let rel = |base: Option<&str>| ChannelRelations {
            base: base.map(str::to_owned),
            overrides: None,
        };

        // Both observation orders must produce identical edges,
        // depths, and discovered sets.
        let run = |order: &[(&ChannelUrl, ChannelRelations)]| {
            let mut ex =
                ChannelExpander::new(ChannelRelationsMode::Warn, 2, vec![Platform::Linux64]);
            ex.register_user_channel(Channel::from_url(a.clone()));
            ex.register_user_channel(Channel::from_url(b.clone()));
            for (url, relations) in order {
                let entries = ex.relations_of.entry((*url).clone()).or_default();
                if !entries.contains(relations) {
                    entries.push(relations.clone());
                }
                let mut newly = Vec::new();
                ex.relax((*url).clone(), &mut newly).unwrap();
            }
            let mut edges = ex.edges.clone();
            edges.sort();
            let mut discovered: Vec<ChannelUrl> = ex.discovered.keys().cloned().collect();
            discovered.sort();
            (edges, discovered, ex.depth_of.clone())
        };

        let a_declares = rel(Some("../m"));
        let m_declares = rel(Some("../c"));
        let b_declares = rel(Some("../c"));
        let c_declares = rel(Some("../d"));

        // Order 1: the deep path resolves first; c is observed at
        // depth 2 and refuses d, then b's shorter path relaxes c.
        let one = run(&[
            (&a, a_declares.clone()),
            (&m, m_declares.clone()),
            (&c, c_declares.clone()),
            (&b, b_declares.clone()),
        ]);
        // Order 2: the shortcut resolves first.
        let two = run(&[
            (&b, b_declares),
            (&a, a_declares),
            (&m, m_declares),
            (&c, c_declares),
        ]);

        assert_eq!(one.0, two.0, "edge sets must match");
        assert_eq!(one.1, two.1, "discovered sets must match");
        assert_eq!(one.2, two.2, "depths must match");
        assert!(
            one.1.contains(&d),
            "d must be discovered via the shorter path regardless of order"
        );
    }

    /// Anchors derive from the final edge set and the caller's user
    /// order, not from fetch completion order.
    #[test]
    fn anchors_prefer_earliest_user_channel() {
        let a = chan("https://example.com/a/");
        let b = chan("https://example.com/b/");
        let cf = chan("https://example.com/conda-forge/");

        let mut ex = ChannelExpander::new(ChannelRelationsMode::Warn, 2, vec![Platform::Linux64]);
        ex.register_user_channel(Channel::from_url(a.clone()));
        ex.register_user_channel(Channel::from_url(b.clone()));

        // Both a and b declare cf as base; simulate b's subdir
        // arriving first.
        let declares_cf = ChannelRelations {
            base: Some("../conda-forge".to_owned()),
            overrides: None,
        };
        for url in [&b, &a] {
            let entries = ex.relations_of.entry(url.clone()).or_default();
            entries.push(declares_cf.clone());
            let mut newly = Vec::new();
            ex.relax(url.clone(), &mut newly).unwrap();
        }

        let anchors = ex.anchors(&[a.clone(), b.clone()]);
        assert_eq!(
            anchors.get(&cf),
            Some(&a),
            "cf must anchor to the earliest user channel that references it"
        );
    }
}
