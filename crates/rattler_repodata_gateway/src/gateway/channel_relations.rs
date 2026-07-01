//! [CEP-42] channel-relations resolution.
//!
//! Given a set of user-specified channels, the channels reachable from
//! them via declared `base`/`overrides` relations, and the relation
//! edges themselves, produces the final channel priority order.
//!
//! The function is intentionally edge-oriented rather than
//! channel-oriented: CEP-42 lets a single channel declare DIFFERENT
//! relations per subdir, so collapsing them into one `base` and one
//! `overrides` per channel is wrong (the discarded edge silently
//! disappears from the priority graph). The caller (the
//! [`ChannelExpander`](super::channel_expander::ChannelExpander))
//! observes per-`(channel, platform)` relations, resolves them to
//! edges, deduplicates exact duplicates, and passes the deduplicated
//! edge list in.
//!
//! Steps performed here:
//! 1. Build user edges from consecutive user-listed channels (user
//!    wins; left-to-right means strictly higher to lower priority).
//! 2. Drop any relation edge that directly contradicts the user's
//!    explicit ordering. (Edges between two user channels where the
//!    user listed `to` at or before `from` lose to the user. Self-loop
//!    relation edges DO fall through to cycle detection; they are
//!    malformed, not user conflicts.)
//! 3. Topologically sort. User edges are inserted first, so a cycle
//!    that mixes user and relation edges always breaks by dropping a
//!    relation edge. Broken edges land in
//!    [`Resolution::broken_cycle_edges`].
//!
//! [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md

use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
};

/// Default CEP-42 relation-traversal depth. Each hop through a `base`
/// or `overrides` relation costs one. Setting this to `0` disables
/// relation following entirely.
pub const DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH: usize = 10;

/// Where a [`PriorityEdge`] originated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EdgeSource {
    /// Implied by the user's channel ordering.
    User,
    /// Declared via the `to` channel's `base`.
    Base,
    /// Declared via the `from` channel's `overrides`.
    Override,
}

/// Directed priority edge: `from` outranks `to`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PriorityEdge<K> {
    pub from: K,
    pub to: K,
    pub source: EdgeSource,
}

/// Outcome of a channel priority resolution. Always succeeds; the
/// caller surfaces `ignored_edges` / `broken_cycle_edges` as warnings
/// or errors per its mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution<K> {
    /// Final priority order, highest first.
    pub order: Vec<K>,
    /// Edges respected by `order`.
    pub edges: Vec<PriorityEdge<K>>,
    /// Relation edges dropped because they contradicted the user's
    /// explicit ordering. Surfaced by the expander as
    /// `UserOrderConflict` warnings.
    pub ignored_edges: Vec<PriorityEdge<K>>,
    /// Relation edges dropped to break a cycle.
    pub broken_cycle_edges: Vec<PriorityEdge<K>>,
}

/// Resolve channel priority from an explicit list of relation edges.
///
/// `user_channels` is the caller-supplied channel order;
/// `discovered_channels` lists every node that should appear in the
/// resolution (user + transitively discovered); `relation_edges`
/// carries the deduplicated `Base` / `Override` edges. The output is
/// fully determined by the inputs, so callers that need run-to-run
/// determinism must pass canonically ordered slices.
pub fn resolve_channel_priority<K>(
    user_channels: &[K],
    discovered_channels: &[K],
    relation_edges: &[PriorityEdge<K>],
) -> Resolution<K>
where
    K: Hash + Eq + Clone + std::fmt::Debug,
{
    if user_channels.is_empty() {
        return Resolution {
            order: Vec::new(),
            edges: Vec::new(),
            ignored_edges: Vec::new(),
            broken_cycle_edges: Vec::new(),
        };
    }

    let Graph {
        edges,
        ignored_edges,
    } = Graph::build(user_channels, relation_edges);

    let TopoResult {
        order,
        broken_cycle_edges,
        edges: kept_edges,
    } = TopoResult::new(discovered_channels, edges);

    Resolution {
        order,
        edges: kept_edges,
        ignored_edges,
        broken_cycle_edges,
    }
}

struct Graph<K> {
    edges: Vec<PriorityEdge<K>>,
    ignored_edges: Vec<PriorityEdge<K>>,
}

impl<K> Graph<K>
where
    K: Hash + Eq + Clone,
{
    fn build(user_channels: &[K], relation_edges: &[PriorityEdge<K>]) -> Self {
        // User edges form a linear chain `u0 -> u1 -> ... -> un`, so
        // `user_pos(from) >= user_pos(to)` is equivalent to a reachability
        // check and runs in O(1).
        let user_positions: HashMap<&K, usize> = user_channels
            .iter()
            .enumerate()
            .map(|(i, c)| (c, i))
            .collect();

        let mut edges: Vec<PriorityEdge<K>> =
            Vec::with_capacity(user_channels.len().saturating_sub(1) + relation_edges.len());
        for pair in user_channels.windows(2) {
            edges.push(PriorityEdge {
                from: pair[0].clone(),
                to: pair[1].clone(),
                source: EdgeSource::User,
            });
        }

        let mut ignored_edges: Vec<PriorityEdge<K>> = Vec::new();
        for edge in relation_edges {
            dispatch_edge(
                edge.clone(),
                &user_positions,
                &mut edges,
                &mut ignored_edges,
            );
        }

        Self {
            edges,
            ignored_edges,
        }
    }
}

/// Route `edge` to `accepted` or `ignored`. A relation edge is ignored
/// when BOTH endpoints are user-listed AND the user placed `to` strictly
/// before `from` (i.e. the user wants `to` to outrank `from`). Self-loop
/// relation edges fall through to cycle detection; they are malformed
/// metadata, not user conflicts.
fn dispatch_edge<K>(
    edge: PriorityEdge<K>,
    user_positions: &HashMap<&K, usize>,
    accepted: &mut Vec<PriorityEdge<K>>,
    ignored: &mut Vec<PriorityEdge<K>>,
) where
    K: Hash + Eq,
{
    if edge.from == edge.to {
        accepted.push(edge);
        return;
    }
    let conflict = matches!(
        (
            user_positions.get(&edge.from),
            user_positions.get(&edge.to),
        ),
        (Some(from_pos), Some(to_pos)) if to_pos < from_pos,
    );

    if conflict {
        ignored.push(edge);
    } else {
        accepted.push(edge);
    }
}

struct TopoResult<K> {
    order: Vec<K>,
    /// Edges actually respected by `order`.
    edges: Vec<PriorityEdge<K>>,
    /// Back-edges that were dropped to break a cycle.
    broken_cycle_edges: Vec<PriorityEdge<K>>,
}

impl<K> TopoResult<K>
where
    K: Hash + Eq + Clone,
{
    /// Topological sort with user-edge-preserving cycle breaking.
    /// Inserts edges greedily, user edges first; rejects any edge that
    /// would close a cycle with the ones already accepted. After the
    /// greedy pass the adjacency is acyclic and a plain post-order DFS
    /// yields the order.
    fn new(channels: &[K], edges: Vec<PriorityEdge<K>>) -> Self {
        // Index every node, including any edge endpoints that aren't in
        // `channels`, so no edge is silently dropped.
        let mut index: HashMap<K, usize> = HashMap::with_capacity(channels.len());
        let mut nodes: Vec<K> = Vec::with_capacity(channels.len());
        for ch in channels {
            if !index.contains_key(ch) {
                index.insert(ch.clone(), nodes.len());
                nodes.push(ch.clone());
            }
        }
        for edge in &edges {
            for endpoint in [&edge.from, &edge.to] {
                if !index.contains_key(endpoint) {
                    index.insert(endpoint.clone(), nodes.len());
                    nodes.push(endpoint.clone());
                }
            }
        }

        let n = nodes.len();
        let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];

        // Partition rather than relying on input order so the user-first
        // preference is robust.
        let (user_edges, relation_edges): (Vec<_>, Vec<_>) = edges
            .into_iter()
            .partition(|e| matches!(e.source, EdgeSource::User));

        let mut kept: Vec<PriorityEdge<K>> =
            Vec::with_capacity(user_edges.len() + relation_edges.len());
        let mut broken: Vec<PriorityEdge<K>> = Vec::new();

        // User edges form a linear chain and are acyclic with unique
        // user channels; only `[a, b, a]`-style duplicates can introduce
        // a cycle, and `[a, a]`-style self-edges are dropped (no
        // topological order respects a node coming before itself).
        let mut seen_user: HashSet<(usize, usize)> = HashSet::new();
        for edge in user_edges {
            let from = index[&edge.from];
            let to = index[&edge.to];
            if from == to {
                broken.push(edge);
                continue;
            }
            if !seen_user.insert((from, to)) {
                // Exact duplicate: redundant with the already-accepted
                // copy, but the order does respect it.
                kept.push(edge);
                continue;
            }
            if seen_user.contains(&(to, from)) {
                broken.push(edge);
            } else {
                adjacency[from].push(to);
                kept.push(edge);
            }
        }

        // Generation counter avoids per-edge `vec![false; n]` zeroing.
        let mut visited_gen: Vec<u32> = vec![0; n];
        let mut generation: u32 = 0;

        for edge in relation_edges {
            let from = index[&edge.from];
            let to = index[&edge.to];
            if can_reach(to, from, &adjacency, &mut visited_gen, &mut generation) {
                broken.push(edge);
            } else {
                adjacency[from].push(to);
                kept.push(edge);
            }
        }

        let mut post_order: Vec<usize> = Vec::with_capacity(n);
        let mut visited = vec![false; n];
        for start in 0..n {
            if visited[start] {
                continue;
            }
            dfs_post_order(start, &adjacency, &mut visited, &mut post_order);
        }

        post_order.reverse();
        let order: Vec<K> = post_order.into_iter().map(|i| nodes[i].clone()).collect();

        Self {
            order,
            edges: kept,
            broken_cycle_edges: broken,
        }
    }
}

/// Returns `true` if `target` is reachable from `start`. `start ==
/// target` counts as reachable so self-loops register as cycles.
/// `visited_gen`/`generation` is reusable scratch: a node is marked visited
/// when `visited_gen[v] == *generation`, avoiding per-call buffer zeroing.
fn can_reach(
    start: usize,
    target: usize,
    adjacency: &[Vec<usize>],
    visited_gen: &mut [u32],
    generation: &mut u32,
) -> bool {
    if start == target {
        return true;
    }

    // Zero the buffer on wrap so stale marks can't collide.
    *generation = generation.wrapping_add(1);
    if *generation == 0 {
        visited_gen.fill(0);
        *generation = 1;
    }
    let current = *generation;

    let mut stack: Vec<usize> = vec![start];
    while let Some(node) = stack.pop() {
        if node == target {
            return true;
        }
        if visited_gen[node] == current {
            continue;
        }
        visited_gen[node] = current;
        for &next in &adjacency[node] {
            if visited_gen[next] != current {
                stack.push(next);
            }
        }
    }
    false
}

/// Iterative post-order DFS. Each stack frame carries a cursor into
/// the node's adjacency list so we resume at the right neighbor after
/// descending. Assumes `adjacency` is acyclic.
fn dfs_post_order(
    start: usize,
    adjacency: &[Vec<usize>],
    visited: &mut [bool],
    post_order: &mut Vec<usize>,
) {
    visited[start] = true;
    let mut stack: Vec<(usize, usize)> = vec![(start, 0)];

    while let Some(&(node, next_idx)) = stack.last() {
        let neighbors = &adjacency[node];
        if next_idx >= neighbors.len() {
            stack.pop();
            post_order.push(node);
            continue;
        }

        let top = stack.len() - 1;
        stack[top].1 = next_idx + 1;

        let neighbor = neighbors[next_idx];
        if !visited[neighbor] {
            visited[neighbor] = true;
            stack.push((neighbor, 0));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};

    use super::*;

    /// Test-only registry of declared relations, used by `resolve`
    /// below to set up algorithm inputs concisely. NOT the runtime
    /// representation: production code passes the deduplicated
    /// per-`(channel, platform)` edge list directly.
    type ChannelRegistry<'a> = HashMap<&'a str, ChannelRelations<'a>>;

    #[derive(Debug, Clone, Copy, Default)]
    struct ChannelRelations<'a> {
        base: Option<&'a str>,
        overrides: Option<&'a str>,
    }

    /// Build a [`ChannelRegistry`] from an iterator of (name, base,
    /// overrides) triples for concise test setup.
    fn registry<'a, I>(entries: I) -> ChannelRegistry<'a>
    where
        I: IntoIterator<Item = (&'a str, Option<&'a str>, Option<&'a str>)>,
    {
        entries
            .into_iter()
            .map(|(name, base, overrides)| (name, ChannelRelations { base, overrides }))
            .collect()
    }

    /// Walk `registry` from `user_channels`, gather the BFS discovery
    /// order, and build the deduplicated edge list. Mirrors what
    /// [`ChannelExpander`](super::super::channel_expander::ChannelExpander)
    /// does at runtime. Kept private here so the tests can exercise
    /// the algorithm with concise inputs.
    fn build_inputs<'a>(
        user: &[&'a str],
        registry: &ChannelRegistry<'a>,
        max_depth: usize,
    ) -> (Vec<&'a str>, Vec<PriorityEdge<&'a str>>) {
        let mut discovered_set: std::collections::HashSet<&'a str> =
            std::collections::HashSet::new();
        let mut discovered: Vec<&'a str> = Vec::new();
        let mut queue: VecDeque<(&'a str, usize)> = VecDeque::new();
        for ch in user {
            if discovered_set.insert(ch) {
                discovered.push(ch);
                queue.push_back((ch, 0));
            }
        }
        while let Some((ch, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            if let Some(rels) = registry.get(ch) {
                for related in [rels.base, rels.overrides].into_iter().flatten() {
                    if discovered_set.insert(related) {
                        discovered.push(related);
                        queue.push_back((related, depth + 1));
                    }
                }
            }
        }

        let mut seen_edges: std::collections::HashSet<(EdgeSource, &'a str, &'a str)> =
            std::collections::HashSet::new();
        let mut edges: Vec<PriorityEdge<&'a str>> = Vec::new();
        for ch in &discovered {
            let Some(rels) = registry.get(ch) else {
                continue;
            };
            if let Some(base) = rels.base
                && discovered_set.contains(base)
                && seen_edges.insert((EdgeSource::Base, base, ch))
            {
                edges.push(PriorityEdge {
                    from: base,
                    to: ch,
                    source: EdgeSource::Base,
                });
            }
            if let Some(overridden) = rels.overrides
                && discovered_set.contains(overridden)
                && seen_edges.insert((EdgeSource::Override, ch, overridden))
            {
                edges.push(PriorityEdge {
                    from: ch,
                    to: overridden,
                    source: EdgeSource::Override,
                });
            }
        }
        (discovered, edges)
    }

    fn resolve<'a>(user: &[&'a str], reg: &ChannelRegistry<'a>) -> Resolution<&'a str> {
        let (discovered, edges) = build_inputs(user, reg, DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH);
        resolve_channel_priority(user, &discovered, &edges)
    }

    fn resolve_with_depth<'a>(
        user: &[&'a str],
        reg: &ChannelRegistry<'a>,
        max_depth: usize,
    ) -> Resolution<&'a str> {
        let (discovered, edges) = build_inputs(user, reg, max_depth);
        resolve_channel_priority(user, &discovered, &edges)
    }

    #[test]
    fn empty_user_channels_returns_empty_resolution() {
        let reg: ChannelRegistry<'_> = ChannelRegistry::new();
        let r = resolve(&[], &reg);
        assert!(r.order.is_empty());
        assert!(r.edges.is_empty());
        assert!(r.ignored_edges.is_empty());
        assert!(r.broken_cycle_edges.is_empty());
    }

    #[test]
    fn single_channel_with_no_relations() {
        let reg = registry([("conda-forge", None, None)]);
        let r = resolve(&["conda-forge"], &reg);
        assert_eq!(r.order, vec!["conda-forge"]);
        assert!(r.edges.is_empty());
        assert!(r.ignored_edges.is_empty());
        assert!(r.broken_cycle_edges.is_empty());
    }

    #[test]
    fn channel_not_in_registry_is_treated_as_having_no_relations() {
        let reg: ChannelRegistry<'_> = ChannelRegistry::new();
        let r = resolve(&["a", "b"], &reg);
        assert_eq!(r.order, vec!["a", "b"]);
    }

    #[test]
    fn base_relation_gives_base_higher_priority() {
        let reg = registry([
            ("bioconda", Some("conda-forge"), None),
            ("conda-forge", None, None),
        ]);
        let r = resolve(&["bioconda"], &reg);
        assert_eq!(r.order, vec!["conda-forge", "bioconda"]);
        assert!(r.ignored_edges.is_empty());
        assert!(r.broken_cycle_edges.is_empty());
    }

    #[test]
    fn user_order_consistent_with_base_is_preserved() {
        let reg = registry([
            ("bioconda", Some("conda-forge"), None),
            ("conda-forge", None, None),
        ]);
        let r = resolve(&["conda-forge", "bioconda"], &reg);
        assert_eq!(r.order, vec!["conda-forge", "bioconda"]);
        assert!(r.ignored_edges.is_empty());
    }

    #[test]
    fn overrides_relation_gives_declaring_channel_higher_priority() {
        let reg = registry([
            ("conda-forge/label/rc", None, Some("conda-forge")),
            ("conda-forge", None, None),
        ]);
        let r = resolve(&["conda-forge/label/rc"], &reg);
        assert_eq!(r.order, vec!["conda-forge/label/rc", "conda-forge"]);
    }

    #[test]
    fn transitive_base_chain_is_resolved() {
        let reg = registry([
            ("my-channel", Some("bioconda"), None),
            ("bioconda", Some("conda-forge"), None),
            ("conda-forge", None, None),
        ]);
        let r = resolve(&["my-channel"], &reg);
        assert_eq!(r.order, vec!["conda-forge", "bioconda", "my-channel"]);
    }

    #[test]
    fn base_and_overrides_combine_on_the_same_channel() {
        let reg = registry([
            ("my-channel", Some("conda-forge"), Some("my-hotfixes")),
            ("conda-forge", None, None),
            ("my-hotfixes", None, None),
        ]);
        let r = resolve(&["my-channel"], &reg);
        assert_eq!(r.order, vec!["conda-forge", "my-channel", "my-hotfixes"]);
    }

    #[test]
    fn user_order_wins_over_conflicting_base_relation() {
        let reg = registry([
            ("bioconda", Some("conda-forge"), None),
            ("conda-forge", None, None),
        ]);
        let r = resolve(&["bioconda", "conda-forge"], &reg);
        assert_eq!(r.order, vec!["bioconda", "conda-forge"]);
        assert_eq!(r.ignored_edges.len(), 1);
        let ignored = &r.ignored_edges[0];
        assert_eq!(ignored.source, EdgeSource::Base);
        assert_eq!(ignored.from, "conda-forge");
        assert_eq!(ignored.to, "bioconda");
    }

    #[test]
    fn user_order_wins_over_conflicting_overrides_relation() {
        let reg = registry([("a", None, Some("b")), ("b", None, None)]);
        let r = resolve(&["b", "a"], &reg);
        assert_eq!(r.order, vec!["b", "a"]);
        assert_eq!(r.ignored_edges.len(), 1);
        assert_eq!(r.ignored_edges[0].source, EdgeSource::Override);
    }

    #[test]
    fn non_adjacent_user_channels_still_impose_ordering_and_filter_relations() {
        let reg = registry([("a", None, None), ("b", Some("a"), None), ("x", None, None)]);
        let r = resolve(&["b", "x", "a"], &reg);
        assert_eq!(r.order, vec!["b", "x", "a"]);
        assert_eq!(r.ignored_edges.len(), 1);
        assert_eq!(r.ignored_edges[0].source, EdgeSource::Base);
    }

    #[test]
    fn relation_consistent_with_non_adjacent_user_order_is_kept() {
        let reg = registry([("a", None, None), ("b", Some("a"), None), ("x", None, None)]);
        let r = resolve(&["a", "x", "b"], &reg);
        assert_eq!(r.order, vec!["a", "x", "b"]);
        assert!(r.ignored_edges.is_empty());
    }

    #[test]
    fn simple_cycle_is_broken_rather_than_failing() {
        let reg = registry([("a", Some("b"), None), ("b", Some("a"), None)]);
        let r = resolve(&["a"], &reg);
        assert_eq!(r.order.len(), 2);
        assert!(r.order.contains(&"a"));
        assert!(r.order.contains(&"b"));
        assert_eq!(r.broken_cycle_edges.len(), 1);
        let dropped = &r.broken_cycle_edges[0];
        assert_eq!(dropped.source, EdgeSource::Base);
        assert!(!r.edges.contains(dropped));
        for edge in &r.edges {
            let from_pos = r.order.iter().position(|c| c == &edge.from).unwrap();
            let to_pos = r.order.iter().position(|c| c == &edge.to).unwrap();
            assert!(from_pos < to_pos);
        }
    }

    #[test]
    fn transitive_cycle_is_broken() {
        let reg = registry([
            ("a", Some("b"), None),
            ("b", Some("c"), None),
            ("c", Some("a"), None),
        ]);
        let r = resolve(&["a"], &reg);
        assert_eq!(r.order.len(), 3);
        assert_eq!(r.broken_cycle_edges.len(), 1);
        for edge in &r.edges {
            let from_pos = r.order.iter().position(|c| c == &edge.from).unwrap();
            let to_pos = r.order.iter().position(|c| c == &edge.to).unwrap();
            assert!(from_pos < to_pos);
        }
    }

    #[test]
    fn self_loop_is_broken() {
        let reg = registry([("a", Some("b"), None), ("b", Some("b"), None)]);
        let r = resolve(&["a"], &reg);
        assert_eq!(r.order, vec!["b", "a"]);
        assert_eq!(r.broken_cycle_edges.len(), 1);
        let dropped = &r.broken_cycle_edges[0];
        assert_eq!(dropped.from, "b");
        assert_eq!(dropped.to, "b");
    }

    /// A self-loop on a user-listed channel is no longer routed to
    /// `ignored_edges` via the user-conflict check; self-relations
    /// are malformed metadata. The algorithm reports them via
    /// `broken_cycle_edges`; the caller (the expander) translates
    /// that into a warning/error per its mode.
    #[test]
    fn self_loop_on_a_user_channel_is_treated_as_a_cycle() {
        let reg = registry([("a", Some("a"), None)]);
        let r = resolve(&["a"], &reg);
        assert_eq!(r.order, vec!["a"]);
        assert!(r.ignored_edges.is_empty());
        assert_eq!(r.broken_cycle_edges.len(), 1);
        let dropped = &r.broken_cycle_edges[0];
        assert_eq!(dropped.from, "a");
        assert_eq!(dropped.to, "a");
    }

    #[test]
    fn cycle_mixing_user_and_relation_edges_drops_the_relation_edge() {
        let reg = registry([
            ("a", None, None),
            ("b", None, Some("c")),
            ("c", None, Some("a")),
        ]);
        let r = resolve(&["a", "b"], &reg);
        assert_eq!(r.broken_cycle_edges.len(), 1);
        assert_ne!(r.broken_cycle_edges[0].source, EdgeSource::User);
        assert!(
            r.edges
                .iter()
                .any(|e| e.source == EdgeSource::User && e.from == "a" && e.to == "b")
        );
        let pos = |ch: &str| r.order.iter().position(|c| *c == ch).unwrap();
        assert!(pos("a") < pos("b"));
    }

    #[test]
    fn only_one_edge_is_dropped_per_cycle() {
        let reg = registry([
            ("a", Some("b"), None),
            ("b", Some("c"), None),
            ("c", Some("d"), None),
            ("d", Some("a"), None),
        ]);
        let r = resolve(&["a"], &reg);
        assert_eq!(r.order.len(), 4);
        assert_eq!(r.broken_cycle_edges.len(), 1);
        assert_eq!(r.edges.len(), 3);
    }

    #[test]
    fn a_channel_referenced_multiple_times_appears_only_once() {
        let reg = registry([
            ("a", Some("conda-forge"), None),
            ("b", Some("conda-forge"), None),
            ("conda-forge", None, None),
        ]);
        let r = resolve(&["a", "b"], &reg);
        assert_eq!(r.order.iter().filter(|c| **c == "conda-forge").count(), 1);
    }

    #[test]
    fn max_depth_limits_traversal() {
        let reg = registry([
            ("a", Some("b"), None),
            ("b", Some("c"), None),
            ("c", Some("d"), None),
            ("d", None, None),
        ]);
        let r = resolve_with_depth(&["a"], &reg, 1);
        let nodes: HashSet<&str> = r.order.iter().copied().collect();
        assert!(nodes.contains("a"));
        assert!(nodes.contains("b"));
        assert!(!nodes.contains("c"));
        assert!(!nodes.contains("d"));
    }

    #[test]
    fn max_depth_zero_only_keeps_user_channels() {
        let reg = registry([("a", Some("b"), None), ("b", None, None)]);
        let r = resolve_with_depth(&["a"], &reg, 0);
        assert_eq!(r.order, vec!["a"]);
    }

    #[test]
    fn diamond_without_cycle_is_resolved() {
        let reg = registry([
            ("bottom", Some("left"), None),
            ("left", Some("top"), None),
            ("right", Some("top"), None),
            ("top", None, None),
        ]);
        let r = resolve(&["bottom", "right"], &reg);
        let pos = |ch: &str| r.order.iter().position(|c| *c == ch).unwrap();
        assert!(pos("top") < pos("left"));
        assert!(pos("top") < pos("right"));
        assert!(pos("left") < pos("bottom"));
        assert!(pos("bottom") < pos("right"));
        assert!(r.broken_cycle_edges.is_empty());
    }

    #[test]
    fn all_three_edge_sources_coexist() {
        let reg = registry([
            ("mine", Some("conda-forge"), Some("legacy")),
            ("conda-forge", None, None),
            ("legacy", None, None),
            ("other", None, None),
        ]);
        let r = resolve(&["mine", "other"], &reg);
        let sources: HashSet<_> = r.edges.iter().map(|e| e.source).collect();
        assert!(sources.contains(&EdgeSource::User));
        assert!(sources.contains(&EdgeSource::Base));
        assert!(sources.contains(&EdgeSource::Override));
    }

    #[test]
    fn user_self_edge_is_not_kept() {
        let reg: ChannelRegistry<'_> = ChannelRegistry::new();
        let r = resolve(&["a", "a"], &reg);
        for edge in &r.edges {
            assert_ne!(
                edge.from, edge.to,
                "Resolution::edges must not contain a self-edge"
            );
        }
    }

    #[test]
    fn order_is_consistent_with_every_accepted_edge() {
        let reg = registry([
            ("alpha", Some("conda-forge"), Some("alpha-archive")),
            ("beta", Some("alpha"), None),
            ("conda-forge", None, None),
            ("alpha-archive", None, None),
        ]);
        let r = resolve(&["beta"], &reg);
        for edge in &r.edges {
            let from_pos = r.order.iter().position(|c| c == &edge.from).unwrap();
            let to_pos = r.order.iter().position(|c| c == &edge.to).unwrap();
            assert!(
                from_pos < to_pos,
                "edge {:?} -> {:?} not respected in order {:?}",
                edge.from,
                edge.to,
                r.order
            );
        }
    }

    #[test]
    fn broken_cycle_edges_are_reported_in_the_resolution() {
        let reg = registry([("a", Some("b"), None), ("b", Some("a"), None)]);
        let r = resolve(&["a"], &reg);
        assert!(!r.broken_cycle_edges.is_empty());
    }

    #[test]
    fn no_broken_edges_on_a_cycle_free_graph() {
        let reg = registry([
            ("bioconda", Some("conda-forge"), None),
            ("conda-forge", None, None),
        ]);
        let r = resolve(&["bioconda"], &reg);
        assert!(r.broken_cycle_edges.is_empty());
    }

    /// Two platforms of the same channel declaring DIFFERENT bases is
    /// valid per CEP-42. Both edges must be respected in the final
    /// order, with neither silently dropped.
    #[test]
    fn divergent_per_platform_relations_are_both_respected() {
        // Channel `app` has base `cf-linux` (one platform) and base
        // `cf-osx` (another platform). Both should appear above `app`
        // in the final order.
        let user = ["app"];
        let discovered = vec!["app", "cf-linux", "cf-osx"];
        let edges = vec![
            PriorityEdge {
                from: "cf-linux",
                to: "app",
                source: EdgeSource::Base,
            },
            PriorityEdge {
                from: "cf-osx",
                to: "app",
                source: EdgeSource::Base,
            },
        ];
        let r = resolve_channel_priority(&user, &discovered, &edges);
        let pos = |ch: &str| r.order.iter().position(|c| *c == ch).unwrap();
        assert!(pos("cf-linux") < pos("app"));
        assert!(pos("cf-osx") < pos("app"));
        assert!(r.broken_cycle_edges.is_empty());
    }

    /// Exact duplicate relation edges (same from/to/source) must not
    /// produce a cycle on their own; they're redundant, not
    /// contradictory.
    #[test]
    fn exact_duplicate_relation_edges_are_not_cycles() {
        let user = ["app"];
        let discovered = vec!["app", "cf"];
        let edges = vec![
            PriorityEdge {
                from: "cf",
                to: "app",
                source: EdgeSource::Base,
            },
            PriorityEdge {
                from: "cf",
                to: "app",
                source: EdgeSource::Base,
            },
        ];
        let r = resolve_channel_priority(&user, &discovered, &edges);
        assert!(r.broken_cycle_edges.is_empty());
    }
}
