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
    warning::GatewayWarning,
};
use crate::Reporter;

/// How a query treats [CEP-42] `channel_relations` metadata.
///
/// `Strict` follows the CEP to the letter. The default `Warn` mode
/// deliberately deviates: problems degrade the result instead of
/// failing the query and are returned as [`ChannelRelationsWarning`]s.
///
/// [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelRelationsMode {
    /// Ignore declared relations. Same as `channel_relations_max_depth(0)`.
    Disabled,

    /// Follow relations; report cycles, malformed metadata, and failed
    /// discovery fetches as warnings instead of aborting.
    #[default]
    Warn,

    /// Follow relations; abort with
    /// [`GatewayError::ChannelRelationsError`](super::GatewayError::ChannelRelationsError)
    /// on any violation.
    Strict,
}

/// A non-fatal CEP-42 problem, collected on the query output in
/// [`Warn`](ChannelRelationsMode::Warn) mode. In
/// [`Strict`](ChannelRelationsMode::Strict) mode every variant except
/// [`UserOrderConflict`](Self::UserOrderConflict) becomes a
/// [`GatewayError::ChannelRelationsError`](super::GatewayError::ChannelRelationsError).
#[derive(Debug, Clone, thiserror::Error)]
pub enum ChannelRelationsWarning {
    /// The reference is not a relative path starting with `../`; it is
    /// dropped.
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

    /// The reference fails to resolve against the declaring channel's
    /// URL; it is dropped.
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

    /// `base` and `overrides` resolve to the same channel; both
    /// references are dropped.
    #[error(
        "channel `{declaring}` declares the same target `{target}` as both `base` and `overrides`"
    )]
    BaseAndOverridesSameTarget {
        /// Channel declaring the contradiction.
        declaring: ChannelUrl,
        /// The doubly referenced channel.
        target: ChannelUrl,
    },

    /// The channel references itself; the reference is dropped.
    #[error("channel `{declaring}` declares itself as `{field}`")]
    SelfRelation {
        /// Channel referencing itself.
        declaring: ChannelUrl,
        /// Which field self-referenced: `"base"` or `"overrides"`.
        field: &'static str,
    },

    /// The reference lies beyond `channel_relations_max_depth` and was
    /// not followed. Reported at finalize, when depths are final.
    #[error(
        "CEP-42 relation chain exceeded `channel_relations_max_depth` ({max_depth}) at `{declaring}`; \
         the reference `{reference}` was not followed"
    )]
    MaxDepthExceeded {
        /// Channel declaring the reference.
        declaring: ChannelUrl,
        /// The unfollowed reference.
        reference: String,
        /// The configured limit.
        max_depth: usize,
    },

    /// A discovered channel failed to fetch; in `Warn` mode its subdir
    /// is treated as empty.
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

    /// The relation contradicts the explicit channel order and was
    /// ignored. Never fatal: the user's order wins.
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

    /// Relation edges dropped to break a cycle in the declared
    /// relations.
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

/// CEP-42 state for a single query.
///
/// The executor feeds each resolved subdir to [`observe`](Self::observe)
/// and schedules the pairs it returns; [`finalize`](Self::finalize)
/// yields the priority order and [`take_warnings`](Self::take_warnings)
/// the collected warnings. The final state never depends on fetch
/// completion order: declarations are kept raw and re-derived when a
/// shorter path lowers a channel's depth, and depth refusals are only
/// reported at finalize.
pub(super) struct ChannelExpander {
    mode: ChannelRelationsMode,
    max_depth: usize,
    platforms: Vec<Platform>,
    user_channels: Vec<ChannelUrl>,
    discovered: HashMap<ChannelUrl, Arc<Channel>>,
    /// Shortest known hop distance from any user channel (user = 0).
    depth_of: HashMap<ChannelUrl, usize>,
    /// Raw declarations per channel, kept so relaxation can re-derive
    /// references a deeper pass refused.
    relations_of: HashMap<ChannelUrl, Vec<ChannelRelations>>,
    /// Deduplicated relation edges (tiny, so linear dedup on the Vec).
    edges: Vec<PriorityEdge<ChannelUrl>>,
    warnings: Vec<ChannelRelationsWarning>,
    /// Dedup keys of recorded warnings.
    emitted: HashSet<String>,
    /// Notified once per unique warning as it is recorded.
    reporter: Option<Arc<dyn Reporter>>,
}

impl ChannelExpander {
    pub fn new(
        mode: ChannelRelationsMode,
        max_depth: usize,
        platforms: Vec<Platform>,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Self {
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
            reporter,
        }
    }

    /// `max_depth == 0` is equivalent to [`ChannelRelationsMode::Disabled`].
    pub fn enabled(&self) -> bool {
        !matches!(self.mode, ChannelRelationsMode::Disabled) && self.max_depth > 0
    }

    pub fn strict(&self) -> bool {
        matches!(self.mode, ChannelRelationsMode::Strict)
    }

    pub fn platforms(&self) -> &[Platform] {
        &self.platforms
    }

    /// `true` once any subdir contributed an edge; gates reordering.
    pub fn has_observed_relations(&self) -> bool {
        !self.edges.is_empty()
    }

    /// Record a warning unless an identical one exists, notifying the
    /// reporter on acceptance.
    pub fn push_warning(&mut self, warning: ChannelRelationsWarning) {
        if self.emitted.insert(warning.to_string()) {
            if let Some(reporter) = &self.reporter {
                reporter.on_gateway_warning(&GatewayWarning::from(warning.clone()));
            }
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

    /// Drain the accumulated warnings.
    pub fn take_warnings(&mut self) -> Vec<ChannelRelationsWarning> {
        std::mem::take(&mut self.warnings)
    }

    /// Register a user channel at depth 0, deduplicating repeats.
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

    /// Process one resolved subdir's relations and return the newly
    /// discovered (url, channel, platform) pairs to schedule.
    ///
    /// `Strict` mode returns `Err` on malformed metadata or a cycle so
    /// the executor can abort. Depth violations are deferred to
    /// [`finalize`](Self::finalize), where they no longer depend on
    /// fetch completion order.
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

    /// Derive edges starting from `start`. When a target's depth
    /// improves, its stored declarations are processed again so
    /// references a deeper pass refused are picked up.
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

    /// Derive edges from one declaration of `declaring` at `depth`.
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

        // Malformed per CEP-42; drop both references.
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

            // Silent here; finalize reports it once depths are final.
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
                    // Shorter path: re-derive the target's refusals.
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

    /// Resolve one reference, reporting invalid or unparsable ones.
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

    /// Report deferred depth refusals, resolve the priority order from
    /// canonically sorted inputs (identical run to run), and surface
    /// ignored and broken edges.
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

    /// Report every reference left unfollowed by the depth limit.
    /// Depths only decrease and relaxation re-derives anything that
    /// comes within range, so at finalize a refusal is definitive.
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

    /// Fail fast on a cycle in the partial graph. Node order does not
    /// matter for cycle detection, so the user list stands in for the
    /// node list; edge endpoints are indexed automatically.
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

    /// Map each discovered channel to the first user channel in
    /// `user_priority` that reaches it. Derived from the final edge
    /// set, so independent of fetch completion order.
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
                    // User channels anchor to themselves; their own
                    // pass claims their descendants.
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

/// Validate `reference` as a CEP-42 relative path and resolve it
/// against `declaring`. Strictness keeps malicious metadata from
/// pointing at attacker-controlled URLs.
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

/// Only `../`-rooted relative paths: no scheme, query, fragment,
/// backslash, or empty path segment (one trailing `/` is allowed).
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
    // WHATWG URL parsing maps `\` to `/` for http(s), smuggling in
    // separators the segment checks below never see.
    if reference.contains('\\') {
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

    /// WHATWG URL parsing maps `\` to `/` for http(s); accepting it
    /// would smuggle path separators past the segment checks.
    #[test]
    fn rejects_backslash_reference() {
        let declaring = chan("https://example.com/bioconda/");
        assert!(matches!(
            validate_and_resolve(&declaring, "../\\evil.example").unwrap_err(),
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
                ChannelExpander::new(ChannelRelationsMode::Warn, 2, vec![Platform::Linux64], None);
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

        let mut ex =
            ChannelExpander::new(ChannelRelationsMode::Warn, 2, vec![Platform::Linux64], None);
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

/// Property tests for the invariant the expander exists to uphold:
/// the final state (edges, depths, discovered set, resolution order,
/// anchors, warnings) is a pure function of the declared relations
/// and never depends on the order in which subdir fetches complete.
#[cfg(test)]
mod proptests {
    use std::collections::HashMap;

    use proptest::prelude::*;
    use rattler_conda_types::{Channel, ChannelRelations, ChannelUrl, Platform};
    use url::Url;

    use super::{
        ChannelExpander, ChannelRelationsMode, PriorityEdge, Resolution, validate_and_resolve,
    };

    const NAMES: [&str; 6] = ["a", "b", "c", "d", "e", "f"];

    fn chan(name: &str) -> ChannelUrl {
        ChannelUrl::from(Url::parse(&format!("https://example.com/{name}/")).unwrap())
    }

    /// One randomly generated channel graph: per channel an optional
    /// `base` / `overrides` target index (self-references allowed, so
    /// the malformed paths are exercised too), how many channels the
    /// user listed, and the depth limit.
    #[derive(Debug, Clone)]
    struct Scenario {
        relations: Vec<(Option<usize>, Option<usize>)>,
        user_count: usize,
        max_depth: usize,
    }

    fn scenario() -> impl Strategy<Value = Scenario> {
        (2..=NAMES.len(), 1..=3usize, 1..=4usize).prop_flat_map(|(n, user_count, max_depth)| {
            proptest::collection::vec((proptest::option::of(0..n), proptest::option::of(0..n)), n)
                .prop_map(move |relations| Scenario {
                    user_count: user_count.min(relations.len()),
                    relations,
                    max_depth,
                })
        })
    }

    #[derive(Debug, PartialEq)]
    struct Outcome {
        edges: Vec<PriorityEdge<ChannelUrl>>,
        depths: Vec<(ChannelUrl, usize)>,
        discovered: Vec<ChannelUrl>,
        anchors: Vec<(ChannelUrl, ChannelUrl)>,
        warnings: Vec<String>,
        resolution: Resolution<ChannelUrl>,
    }

    /// Drive the expander like the executor does. `priority[i]` says
    /// how quickly channel `i`'s subdir fetch completes: at every
    /// step the discovered-but-unobserved channel with the lowest
    /// priority is observed next.
    fn run(sc: &Scenario, priority: &[u32]) -> Outcome {
        let n = sc.relations.len();
        let urls: Vec<ChannelUrl> = NAMES[..n].iter().map(|name| chan(name)).collect();

        let mut ex = ChannelExpander::new(
            ChannelRelationsMode::Warn,
            sc.max_depth,
            vec![Platform::Linux64],
            None,
        );
        let user_urls: Vec<ChannelUrl> = urls[..sc.user_count].to_vec();
        for url in &user_urls {
            ex.register_user_channel(Channel::from_url(url.clone()));
        }

        let mut observed = vec![false; n];
        loop {
            let next = (0..n)
                .filter(|&i| !observed[i] && ex.discovered.contains_key(&urls[i]))
                .min_by_key(|&i| (priority[i], i));
            let Some(i) = next else { break };
            observed[i] = true;

            let (base, overrides) = sc.relations[i];
            let relations = ChannelRelations {
                base: base.map(|t| format!("../{}", NAMES[t])),
                overrides: overrides.map(|t| format!("../{}", NAMES[t])),
            };
            if relations.is_empty() {
                continue;
            }
            // Mirrors `observe` after its subdir gating.
            let entries = ex.relations_of.entry(urls[i].clone()).or_default();
            if entries.contains(&relations) {
                continue;
            }
            entries.push(relations);
            let mut newly = Vec::new();
            ex.relax(urls[i].clone(), &mut newly).unwrap();
        }

        let resolution = ex.finalize().unwrap();
        let mut edges = ex.edges.clone();
        edges.sort();
        let mut depths: Vec<_> = ex.depth_of.iter().map(|(u, d)| (u.clone(), *d)).collect();
        depths.sort();
        let mut discovered: Vec<_> = ex.discovered.keys().cloned().collect();
        discovered.sort();
        let mut anchors: Vec<_> = ex.anchors(&user_urls).into_iter().collect();
        anchors.sort();
        let mut warnings: Vec<_> = ex.take_warnings().iter().map(ToString::to_string).collect();
        warnings.sort();
        Outcome {
            edges,
            depths,
            discovered,
            anchors,
            warnings,
            resolution,
        }
    }

    proptest! {
        /// The observable outcome must be identical for every fetch
        /// completion order, and the resolution must satisfy its
        /// structural invariants.
        #[test]
        fn outcome_is_independent_of_fetch_completion_order(
            sc in scenario(),
            priority in proptest::collection::vec(any::<u32>(), NAMES.len()),
        ) {
            let canonical: Vec<u32> = (0..NAMES.len() as u32).collect();
            let reference = run(&sc, &canonical);
            let shuffled = run(&sc, &priority);
            prop_assert_eq!(&shuffled, &reference);

            // Structural invariants of the final resolution.
            let order = &reference.resolution.order;
            let mut sorted_order = order.clone();
            sorted_order.sort();
            prop_assert_eq!(
                &sorted_order, &reference.discovered,
                "order must be a permutation of the discovered channels"
            );
            let position: HashMap<&ChannelUrl, usize> =
                order.iter().enumerate().map(|(i, u)| (u, i)).collect();
            for edge in &reference.resolution.edges {
                prop_assert_ne!(&edge.from, &edge.to, "kept edges must not be self-loops");
                prop_assert!(
                    position[&edge.from] < position[&edge.to],
                    "kept edge {:?} -> {:?} not respected by {:?}",
                    edge.from, edge.to, order
                );
            }
            // Every discovered channel sits within the depth limit.
            for (url, depth) in &reference.depths {
                prop_assert!(
                    *depth <= sc.max_depth,
                    "{url} discovered at depth {depth} > max {}",
                    sc.max_depth
                );
            }
        }
    }

    /// The security contract of the reference validator: anything it
    /// accepts resolves to the same origin as the declaring channel.
    fn reference() -> impl Strategy<Value = String> {
        let segment = prop_oneof![
            Just("..".to_string()),
            Just(".".to_string()),
            Just(String::new()),
            Just("conda-forge".to_string()),
            Just("%2e%2e".to_string()),
            Just("http:".to_string()),
            Just("\\evil.example".to_string()),
            Just("?x=1".to_string()),
            Just("#frag".to_string()),
            "[a-z]{1,4}".prop_map(|s| s),
        ];
        prop_oneof![
            (
                proptest::collection::vec(segment, 0..5),
                any::<bool>(),
                any::<bool>()
            )
                .prop_map(|(segments, rooted, trailing)| {
                    let mut reference = if rooted {
                        "../".to_string()
                    } else {
                        String::new()
                    };
                    reference.push_str(&segments.join("/"));
                    if trailing {
                        reference.push('/');
                    }
                    reference
                }),
            any::<String>(),
        ]
    }

    proptest! {
        #[test]
        fn accepted_references_never_change_origin(
            reference in reference(),
            base_idx in 0..3usize,
        ) {
            let bases = [
                "https://example.com/scope/chan/",
                "http://host.example:8080/a/b/",
                "file:///tmp/repo/chan/",
            ];
            let base = ChannelUrl::from(Url::parse(bases[base_idx]).unwrap());
            if let Ok(resolved) = validate_and_resolve(&base, &reference) {
                let base_url = base.url();
                let url = resolved.url();
                prop_assert_eq!(url.scheme(), base_url.scheme());
                prop_assert_eq!(url.host_str(), base_url.host_str());
                prop_assert_eq!(
                    url.port_or_known_default(),
                    base_url.port_or_known_default()
                );
                // No empty path segments survive validation (a single
                // trailing slash yields one trailing empty segment).
                let segments: Vec<&str> = url.path().split('/').collect();
                for (i, segment) in segments.iter().enumerate() {
                    prop_assert!(
                        !segment.is_empty() || i == 0 || i == segments.len() - 1,
                        "empty path segment in {url}"
                    );
                }
            }
        }
    }
}
