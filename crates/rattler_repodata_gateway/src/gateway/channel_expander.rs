//! State and behavior for following CEP-42 `channel_relations` during
//! a [`RepoDataQuery`](super::query::RepoDataQuery).

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use rattler_conda_types::{Channel, ChannelUrl, Platform};

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
/// Bindings forward these warnings to their host environment (Python
/// `warnings.warn`, JS `console.warn`); they cannot be silently lost.
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
    /// CEP-42 in that the latter mandates aborting on cycles /
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
/// ignore them. In [`ChannelRelationsMode::Strict`] each one is
/// instead translated into a
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
    /// same channel URL. CEP-42 forbids this.
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
    /// itself. CEP-42 forbids this.
    #[error("channel `{declaring}` declares itself as `{field}`")]
    SelfRelation {
        /// Channel that declared the self-relation.
        declaring: ChannelUrl,
        /// Which field self-referenced: `"base"` or `"overrides"`.
        field: &'static str,
    },

    /// A relation chain reached the configured depth limit and was
    /// truncated. CEP-42 says this should abort resolution; `Warn`
    /// mode tolerates it.
    #[error(
        "CEP-42 relation chain exceeded `channel_relations_max_depth` ({max_depth}) at `{declaring}`; \
         the reference `{reference}` was not followed"
    )]
    MaxDepthExceeded {
        /// Channel whose relation would have crossed the depth limit.
        declaring: ChannelUrl,
        /// The reference that would have been followed.
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

impl ChannelRelationsWarning {
    /// Render the warning as the message used to construct a
    /// [`GatewayError::ChannelRelationsError`](super::GatewayError::ChannelRelationsError)
    /// in `Strict` mode.
    fn into_strict_message(self) -> String {
        self.to_string()
    }
}

/// Tracks CEP-42 state across a query: discovered channels, the
/// declared relations gathered as subdirs resolve, the user's
/// supplied channel order, and any non-fatal warnings observed along
/// the way. The query executor calls
/// [`ChannelExpander::observe`] on each freshly resolved subdir to
/// get the (channel, platform) pairs it must schedule next,
/// [`ChannelExpander::finalize`] at the end to learn the final
/// priority order, and [`ChannelExpander::take_warnings`] to attach
/// the accumulated warnings to the query output.
pub(super) struct ChannelExpander {
    mode: ChannelRelationsMode,
    max_depth: usize,
    platforms: Vec<Platform>,
    user_channels: Vec<ChannelUrl>,
    /// User channels are at depth 0; every other discovered channel
    /// is at depth >= 1.
    discovered: HashMap<ChannelUrl, Arc<Channel>>,
    /// Discovery order (user channels first, then BFS-discovered).
    /// Used to feed the algorithm a deterministic node list.
    discovered_order: Vec<ChannelUrl>,
    depth_of: HashMap<ChannelUrl, usize>,
    /// Maps each discovered transitive channel to the user-channel
    /// slot it was first reached from. Used by the executor to
    /// preserve caller-specified custom-source positions while still
    /// placing discovered channels adjacent to the user channel that
    /// introduced them.
    introducer_of: HashMap<ChannelUrl, ChannelUrl>,
    /// Per-(channel, platform) deduplicated edge set.
    edges: HashSet<PriorityEdge<ChannelUrl>>,
    /// Insertion-ordered edge list mirroring `edges` (a `HashSet`
    /// alone would make the algorithm output depend on hash
    /// randomization).
    edges_in_order: Vec<PriorityEdge<ChannelUrl>>,
    /// `true` once any subdir contributed at least one valid edge.
    /// Drives the executor's decision to reorder the result.
    observed_relations: bool,
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
            discovered_order: Vec::new(),
            depth_of: HashMap::new(),
            introducer_of: HashMap::new(),
            edges: HashSet::new(),
            edges_in_order: Vec::new(),
            observed_relations: false,
            warnings: Vec::new(),
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
        self.observed_relations
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
        self.discovered_order.push(url.clone());
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
    /// `base == overrides`, depth exceeded, cycle) returns
    /// `Err(ChannelRelationsError)` so the executor can abort the
    /// remaining in-flight fetches.
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

        let current_depth = self.depth_of.get(channel_url).copied().unwrap_or(0);

        // Resolve base / overrides separately and stash the resolved
        // targets so we can run the cross-field consistency check after.
        let base_resolved = self.resolve_field(channel_url, relations.base.as_deref())?;
        let overrides_resolved = self.resolve_field(channel_url, relations.overrides.as_deref())?;

        // base == overrides target → malformed.
        if let (Some(b), Some(o)) = (&base_resolved, &overrides_resolved)
            && b == o
        {
            let w = ChannelRelationsWarning::BaseAndOverridesSameTarget {
                declaring: channel_url.clone(),
                target: b.clone(),
            };
            if self.strict() {
                return Err(super::GatewayError::ChannelRelationsError(
                    w.into_strict_message(),
                ));
            }
            self.warnings.push(w);
        }

        let mut newly_discovered: Vec<(ChannelUrl, Arc<Channel>)> = Vec::new();

        for (field, target_url) in [("base", base_resolved), ("overrides", overrides_resolved)] {
            let Some(target) = target_url else { continue };

            // Self-relation → malformed (per CEP-42).
            if &target == channel_url {
                let w = ChannelRelationsWarning::SelfRelation {
                    declaring: channel_url.clone(),
                    field,
                };
                if self.strict() {
                    return Err(super::GatewayError::ChannelRelationsError(
                        w.into_strict_message(),
                    ));
                }
                self.warnings.push(w);
                continue;
            }

            // Depth check. Following this reference would land the
            // target at depth `current_depth + 1`. Refuse if that
            // exceeds `max_depth`.
            if current_depth + 1 > self.max_depth {
                let reference = match field {
                    "base" => relations.base.clone().unwrap_or_default(),
                    "overrides" => relations.overrides.clone().unwrap_or_default(),
                    _ => String::new(),
                };
                let w = ChannelRelationsWarning::MaxDepthExceeded {
                    declaring: channel_url.clone(),
                    reference,
                    max_depth: self.max_depth,
                };
                if self.strict() {
                    return Err(super::GatewayError::ChannelRelationsError(
                        w.into_strict_message(),
                    ));
                }
                self.warnings.push(w);
                continue;
            }

            // Record the edge (deduplicated). `base` means target is
            // higher priority than declaring; `overrides` means
            // declaring is higher priority than target.
            let edge = match field {
                "base" => PriorityEdge {
                    from: target.clone(),
                    to: channel_url.clone(),
                    source: EdgeSource::Base,
                },
                "overrides" => PriorityEdge {
                    from: channel_url.clone(),
                    to: target.clone(),
                    source: EdgeSource::Override,
                },
                _ => unreachable!(),
            };
            if self.edges.insert(edge.clone()) {
                self.edges_in_order.push(edge);
            }
            self.observed_relations = true;

            // Track introducer of newly discovered targets so the
            // executor can group transitively discovered channels
            // adjacent to the user channel they were reached from.
            // Use the introducer of `channel_url` if it's not a user
            // channel, else `channel_url` itself.
            let introducer = self
                .introducer_of
                .get(channel_url)
                .cloned()
                .unwrap_or_else(|| channel_url.clone());

            // Take the minimum depth seen.
            let new_depth = current_depth + 1;
            let recorded = self.depth_of.get(&target).copied();
            if recorded.is_none_or(|d| new_depth < d) {
                self.depth_of.insert(target.clone(), new_depth);
            }

            if self.discovered.contains_key(&target) {
                continue;
            }
            let target_channel = Arc::new(Channel::from_url(target.clone()));
            self.discovered
                .insert(target.clone(), target_channel.clone());
            self.discovered_order.push(target.clone());
            self.introducer_of.insert(target.clone(), introducer);
            newly_discovered.push((target, target_channel));
        }

        // Strict incremental cycle check. The acyclic invariant grows
        // monotonically with edges, so a partial cycle now is a cycle
        // at finalize; detecting it here lets the executor abort
        // in-flight fetches early.
        if self.strict()
            && let Some(msg) = self.strict_cycle_check()
        {
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

    /// Resolve one `base` or `overrides` reference, surfacing
    /// invalid-syntax / unparsable-target failures as warnings (or
    /// errors in `Strict`).
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
                let w = ChannelRelationsWarning::InvalidReferenceSyntax {
                    declaring: declaring.clone(),
                    reference: reference.to_string(),
                };
                if self.strict() {
                    return Err(super::GatewayError::ChannelRelationsError(
                        w.into_strict_message(),
                    ));
                }
                self.warnings.push(w);
                Ok(None)
            }
            Err(ResolveError::Unparsable(err)) => {
                let w = ChannelRelationsWarning::UnparsableReference {
                    declaring: declaring.clone(),
                    reference: reference.to_string(),
                    error: err.to_string(),
                };
                if self.strict() {
                    return Err(super::GatewayError::ChannelRelationsError(
                        w.into_strict_message(),
                    ));
                }
                self.warnings.push(w);
                Ok(None)
            }
        }
    }

    /// Compute the final channel priority resolution from collected
    /// relations. Records a [`ChannelRelationsWarning::CycleBroken`]
    /// when the resolver had to drop back-edges (warn mode); strict
    /// mode catches the cycle earlier via `strict_cycle_check`.
    pub fn finalize(&mut self) -> Resolution<ChannelUrl> {
        let resolution = resolve_channel_priority(
            &self.user_channels,
            &self.discovered_order,
            self.edges_in_order.clone(),
        );
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

    /// In `Strict` mode, returns `Some(message)` describing the
    /// reason `resolution` reveals a cycle. Returns `None` in
    /// `Disabled`/`Warn` modes or when nothing is wrong.
    pub fn strict_error(&self, resolution: &Resolution<ChannelUrl>) -> Option<String> {
        if !self.strict() {
            return None;
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

    /// Incremental strict cycle check. Run after every `observe`.
    fn strict_cycle_check(&self) -> Option<String> {
        let resolution = resolve_channel_priority(
            &self.user_channels,
            &self.discovered_order,
            self.edges_in_order.clone(),
        );
        self.strict_error(&resolution)
    }

    /// Return the introducing user channel for a transitively
    /// discovered channel, if any. Used by the executor to slot
    /// discovered channels next to the user channel they came from.
    pub fn introducer_of(&self, url: &ChannelUrl) -> Option<&ChannelUrl> {
        self.introducer_of.get(url)
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

/// CEP-42 §"references" requires that `base` and `overrides` be
/// relative paths starting with `../`. Each segment must be
/// non-empty; no scheme, no leading `/`, no query, no fragment.
fn is_valid_cep42_reference(reference: &str) -> bool {
    if reference.is_empty() {
        return false;
    }
    if !reference.starts_with("../") && reference != ".." {
        return false;
    }
    // Disallow query / fragment.
    if reference.contains('?') || reference.contains('#') {
        return false;
    }
    // Disallow embedded scheme (e.g. `../http://evil`).
    if reference.contains("://") {
        return false;
    }
    // Reject internal `//` empty segments. Trailing `/` is allowed.
    let core = reference.trim_end_matches('/');
    !core.contains("//")
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

    #[test]
    fn accepts_dotdot_slash_relative() {
        let declaring = chan("https://example.com/bioconda/");
        let resolved = validate_and_resolve(&declaring, "../conda-forge").unwrap();
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
}
