//! [CEP-42] channel-relations resolution.
//!
//! Given user-specified channels and a registry mapping each channel to
//! its declared `base`/`overrides` relations, produces the final
//! channel priority order.
//!
//! Steps:
//! 1. BFS-discover transitively related channels up to [`DEFAULT_MAX_DEPTH`].
//! 2. Build a priority DAG where `from -> to` means `from` has strictly
//!    higher priority than `to`. Edges come from three sources:
//!    consecutive user-listed channels (user wins), each channel's
//!    `base` (base wins over declaring), and each channel's `overrides`
//!    (declaring wins over overridden).
//! 3. Drop any relation edge that contradicts the user's explicit
//!    ordering; user ordering always wins per CEP.
//! 4. Topologically sort, breaking any cycle by dropping back-edges
//!    (recorded in [`Resolution::broken_cycle_edges`], with a
//!    `tracing::warn!`). Resolution always succeeds so bad channel
//!    metadata can't block package resolution.
//!
//! [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md

use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::Hash,
};

/// Default CEP-42 relation-traversal depth. Each hop through a `base`
/// or `overrides` relation costs one.
pub const DEFAULT_MAX_DEPTH: usize = 10;

/// Relations declared by a single channel. Mirrors the shape of
/// [`rattler_conda_types::ChannelRelations`] but generic over the
/// identifier type so the algorithm operates on resolved URLs instead
/// of raw relative-path strings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChannelRelations<K> {
    /// Channel with strictly higher priority than the declaring channel.
    pub base: Option<K>,
    /// Channel with strictly lower priority than the declaring channel.
    pub overrides: Option<K>,
}

/// Channel identifier to its declared relations. Missing entries mean
/// no relations.
pub type ChannelRegistry<K> = HashMap<K, ChannelRelations<K>>;

/// Where a [`PriorityEdge`] originated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeSource {
    /// Implied by the user's channel ordering.
    User,
    /// Declared via the `to` channel's `base`.
    Base,
    /// Declared via the `from` channel's `overrides`.
    Override,
}

/// Directed priority edge: `from` outranks `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorityEdge<K> {
    pub from: K,
    pub to: K,
    pub source: EdgeSource,
}

/// Outcome of a channel priority resolution. Always succeeds; bad
/// metadata surfaces via `ignored_edges` and `broken_cycle_edges`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution<K> {
    /// Final priority order, highest first.
    pub order: Vec<K>,
    /// Edges respected by `order`.
    pub edges: Vec<PriorityEdge<K>>,
    /// Relation edges dropped because they contradicted the user's
    /// explicit ordering.
    pub ignored_edges: Vec<PriorityEdge<K>>,
    /// Relation edges dropped to break a cycle. When non-empty a
    /// `tracing::warn!` is also emitted.
    pub broken_cycle_edges: Vec<PriorityEdge<K>>,
    /// Channels discovered during BFS, in discovery order.
    pub channels: Vec<K>,
}

/// Resolve channel priority. See the module docs for the algorithm.
pub fn resolve_channel_priority<K>(
    user_channels: &[K],
    registry: &ChannelRegistry<K>,
    max_depth: usize,
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
            channels: Vec::new(),
        };
    }

    let Graph {
        edges,
        ignored_edges,
        channels,
    } = Graph::build(user_channels, registry, max_depth);

    let TopoResult {
        order,
        broken_cycle_edges,
        edges: kept_edges,
    } = TopoResult::new(&channels, edges);

    if !broken_cycle_edges.is_empty() {
        tracing::warn!(
            "dropped {} channel-relation edge(s) to break a cycle; \
             this indicates malformed channel metadata. broken edges: {:?}",
            broken_cycle_edges.len(),
            broken_cycle_edges,
        );
    }

    Resolution {
        order,
        edges: kept_edges,
        ignored_edges,
        broken_cycle_edges,
        channels,
    }
}

/// BFS-discover channels reachable from `user_channels` via relations,
/// stopping at `max_depth` hops.
fn discover_channels<K>(
    user_channels: &[K],
    registry: &ChannelRegistry<K>,
    max_depth: usize,
) -> Vec<K>
where
    K: Hash + Eq + Clone,
{
    let mut discovered_set: HashSet<K> = HashSet::new();
    let mut discovered_order: Vec<K> = Vec::new();
    let mut queue: VecDeque<(K, usize)> = VecDeque::new();

    for ch in user_channels {
        if discovered_set.insert(ch.clone()) {
            discovered_order.push(ch.clone());
            queue.push_back((ch.clone(), 0));
        }
    }

    while let Some((channel, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }

        let Some(relations) = registry.get(&channel) else {
            continue;
        };

        for related in [&relations.base, &relations.overrides]
            .into_iter()
            .flatten()
        {
            if discovered_set.insert(related.clone()) {
                discovered_order.push(related.clone());
                queue.push_back((related.clone(), depth + 1));
            }
        }
    }

    discovered_order
}

struct Graph<K> {
    edges: Vec<PriorityEdge<K>>,
    ignored_edges: Vec<PriorityEdge<K>>,
    channels: Vec<K>,
}

impl<K> Graph<K>
where
    K: Hash + Eq + Clone,
{
    fn build(user_channels: &[K], registry: &ChannelRegistry<K>, max_depth: usize) -> Self {
        // User edges form a linear chain `u0 -> u1 -> ... -> un`, so
        // `user_pos(from) >= user_pos(to)` is equivalent to a reachability
        // check and runs in O(1).
        let user_positions: HashMap<&K, usize> = user_channels
            .iter()
            .enumerate()
            .map(|(i, c)| (c, i))
            .collect();

        let mut user_edges: Vec<PriorityEdge<K>> =
            Vec::with_capacity(user_channels.len().saturating_sub(1));
        for pair in user_channels.windows(2) {
            user_edges.push(PriorityEdge {
                from: pair[0].clone(),
                to: pair[1].clone(),
                source: EdgeSource::User,
            });
        }

        let channels = discover_channels(user_channels, registry, max_depth);
        let discovered_set: HashSet<&K> = channels.iter().collect();

        let mut relation_edges: Vec<PriorityEdge<K>> = Vec::new();
        let mut ignored_edges: Vec<PriorityEdge<K>> = Vec::new();

        for channel in &channels {
            let Some(relations) = registry.get(channel) else {
                continue;
            };

            // Drop relations pointing past `max_depth`: we didn't fetch the
            // target so we can't trust its contribution.
            if let Some(base) = relations
                .base
                .as_ref()
                .filter(|b| discovered_set.contains(*b))
            {
                let edge = PriorityEdge {
                    from: base.clone(),
                    to: channel.clone(),
                    source: EdgeSource::Base,
                };
                dispatch_edge(
                    edge,
                    &user_positions,
                    &mut relation_edges,
                    &mut ignored_edges,
                );
            }

            if let Some(overridden) = relations
                .overrides
                .as_ref()
                .filter(|o| discovered_set.contains(*o))
            {
                let edge = PriorityEdge {
                    from: channel.clone(),
                    to: overridden.clone(),
                    source: EdgeSource::Override,
                };
                dispatch_edge(
                    edge,
                    &user_positions,
                    &mut relation_edges,
                    &mut ignored_edges,
                );
            }
        }

        let mut edges = user_edges;
        edges.extend(relation_edges);

        Self {
            edges,
            ignored_edges,
            channels,
        }
    }
}

/// Route `edge` to `accepted` or `ignored`. Conflicts when both
/// endpoints are user channels and the user placed `to` at or before
/// `from`.
fn dispatch_edge<K>(
    edge: PriorityEdge<K>,
    user_positions: &HashMap<&K, usize>,
    accepted: &mut Vec<PriorityEdge<K>>,
    ignored: &mut Vec<PriorityEdge<K>>,
) where
    K: Hash + Eq,
{
    let conflict = matches!(
        (
            user_positions.get(&edge.from),
            user_positions.get(&edge.to),
        ),
        (Some(from_pos), Some(to_pos)) if to_pos <= from_pos,
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
    use super::*;

    /// Build a [`ChannelRegistry`] from an iterator of (name, base,
    /// overrides) triples for concise test setup.
    fn registry<'a, I>(entries: I) -> ChannelRegistry<&'a str>
    where
        I: IntoIterator<Item = (&'a str, Option<&'a str>, Option<&'a str>)>,
    {
        entries
            .into_iter()
            .map(|(name, base, overrides)| (name, ChannelRelations { base, overrides }))
            .collect()
    }

    fn resolve<'a>(user: &[&'a str], reg: &ChannelRegistry<&'a str>) -> Resolution<&'a str> {
        resolve_channel_priority(user, reg, DEFAULT_MAX_DEPTH)
    }

    #[test]
    fn empty_user_channels_returns_empty_resolution() {
        let reg: ChannelRegistry<&str> = ChannelRegistry::new();
        let r = resolve(&[], &reg);
        assert!(r.order.is_empty());
        assert!(r.edges.is_empty());
        assert!(r.ignored_edges.is_empty());
        assert!(r.broken_cycle_edges.is_empty());
        assert!(r.channels.is_empty());
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
        let reg: ChannelRegistry<&str> = ChannelRegistry::new();
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
        // a declares b as base, b declares a as base - a 2-node cycle.
        let reg = registry([("a", Some("b"), None), ("b", Some("a"), None)]);
        let r = resolve(&["a"], &reg);
        // Resolution still produces a total order over both channels.
        assert_eq!(r.order.len(), 2);
        assert!(r.order.contains(&"a"));
        assert!(r.order.contains(&"b"));
        // One back-edge was dropped to break the cycle.
        assert_eq!(r.broken_cycle_edges.len(), 1);
        // The dropped edge is one of the two base relations in the cycle.
        let dropped = &r.broken_cycle_edges[0];
        assert_eq!(dropped.source, EdgeSource::Base);
        // The dropped edge does not appear in the kept edges.
        assert!(!r.edges.contains(dropped));
        // All remaining edges are respected by the order.
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
        // Exactly one back-edge is dropped.
        assert_eq!(r.broken_cycle_edges.len(), 1);
        // All kept edges are respected.
        for edge in &r.edges {
            let from_pos = r.order.iter().position(|c| c == &edge.from).unwrap();
            let to_pos = r.order.iter().position(|c| c == &edge.to).unwrap();
            assert!(from_pos < to_pos);
        }
    }

    #[test]
    fn self_loop_is_broken() {
        // `b` declares itself as base. The user only lists `a`, so `b` is
        // only transitively discovered and the self-loop is not filtered
        // out as a user-ordering conflict.
        let reg = registry([("a", Some("b"), None), ("b", Some("b"), None)]);
        let r = resolve(&["a"], &reg);
        assert_eq!(r.order, vec!["b", "a"]);
        assert_eq!(r.broken_cycle_edges.len(), 1);
        let dropped = &r.broken_cycle_edges[0];
        assert_eq!(dropped.from, "b");
        assert_eq!(dropped.to, "b");
    }

    #[test]
    fn self_loop_on_a_user_channel_is_treated_as_a_conflict_and_ignored() {
        // A self-loop on a user-listed channel is filtered by the user-
        // conflict check (reachability from `a` to `a` is trivially true),
        // so it lands in `ignored_edges` rather than `broken_cycle_edges`.
        let reg = registry([("a", Some("a"), None)]);
        let r = resolve(&["a"], &reg);
        assert_eq!(r.order, vec!["a"]);
        assert_eq!(r.ignored_edges.len(), 1);
        assert!(r.broken_cycle_edges.is_empty());
    }

    #[test]
    fn cycle_mixing_user_and_relation_edges_drops_the_relation_edge() {
        // User list: [a, b] -- gives a user edge a -> b (a > b).
        // Registry:  a's base is c, c's base is b.
        // Relation edges not in direct conflict with user ordering:
        //   c -> a   (c is base of a, so c > a)
        //   b -> c   (c is base of b, so c > b -- wait, b's base is c means
        //             c has higher priority, so edge c -> b)
        // Hmm, let me re-pick: use OVERRIDES to form a cleaner cycle:
        //   b overrides c  => b > c   (edge b -> c)
        //   c overrides a  => c > a   (edge c -> a)
        // Combined with the user edge a -> b, we have a cycle
        //     a -> b -> c -> a
        // consisting of one user edge and two relation edges. The
        // algorithm MUST drop one of the relation edges, never the user
        // edge.
        let reg = registry([
            ("a", None, None),
            ("b", None, Some("c")),
            ("c", None, Some("a")),
        ]);
        let r = resolve(&["a", "b"], &reg);

        assert_eq!(r.broken_cycle_edges.len(), 1);
        // The dropped edge must NOT be a user edge.
        assert_ne!(r.broken_cycle_edges[0].source, EdgeSource::User);
        // The user-edge a -> b must still be present in the kept set.
        assert!(
            r.edges
                .iter()
                .any(|e| e.source == EdgeSource::User && e.from == "a" && e.to == "b")
        );
        // Consequently `a` must come before `b` in the final order.
        let pos = |ch: &str| r.order.iter().position(|c| *c == ch).unwrap();
        assert!(pos("a") < pos("b"));
    }

    #[test]
    fn only_one_edge_is_dropped_per_cycle() {
        // A long chain with a single back-edge closing the cycle. The
        // cycle-breaker must drop exactly one edge, not an entire chain.
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
        // With max_depth = 1 we traverse from a (depth 0) to b (depth 1)
        // but stop before reaching c and d.
        let r = resolve_channel_priority(&["a"], &reg, 1);
        assert!(r.channels.contains(&"a"));
        assert!(r.channels.contains(&"b"));
        assert!(!r.channels.contains(&"c"));
        assert!(!r.channels.contains(&"d"));
    }

    #[test]
    fn max_depth_zero_only_keeps_user_channels() {
        let reg = registry([("a", Some("b"), None), ("b", None, None)]);
        let r = resolve_channel_priority(&["a"], &reg, 0);
        assert_eq!(r.channels, vec!["a"]);
        assert_eq!(r.order, vec!["a"]);
    }

    #[test]
    fn diamond_without_cycle_is_resolved() {
        // Two base chains that share a common ancestor `top`.
        let reg = registry([
            ("bottom", Some("left"), None),
            ("left", Some("top"), None),
            ("right", Some("top"), None),
            ("top", None, None),
        ]);
        let r = resolve(&["bottom", "right"], &reg);
        // `top` must come before both `left` and `right`, which must both
        // come before `bottom`. The user also requires `bottom > right`.
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
    fn discovered_channels_preserve_bfs_order() {
        // a -> b (base), a -> c (overrides), b -> d (base)
        let reg = registry([
            ("a", Some("b"), Some("c")),
            ("b", Some("d"), None),
            ("c", None, None),
            ("d", None, None),
        ]);
        let r = resolve(&["a"], &reg);
        assert_eq!(r.channels, vec!["a", "b", "c", "d"]);
    }

    /// User input `[a, a]` produces a self-edge `a -> a`. No
    /// topological order respects it, so it must not appear in
    /// `kept` (`Resolution::edges`). It belongs in `broken_cycle_edges`.
    #[test]
    fn user_self_edge_is_not_kept() {
        let reg: ChannelRegistry<&str> = ChannelRegistry::new();
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
        // Property-style check on a non-trivial graph: the final order
        // must respect every kept edge (from appears before to).
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

    /// Asserts that `tracing::warn!` is emitted when a cycle is broken.
    #[test]
    #[tracing_test::traced_test]
    fn warning_is_logged_when_a_cycle_is_broken() {
        let reg = registry([("a", Some("b"), None), ("b", Some("a"), None)]);
        let _ = resolve(&["a"], &reg);
        assert!(logs_contain("break a cycle"));
    }

    /// No warning is emitted when the graph is cycle-free.
    #[test]
    #[tracing_test::traced_test]
    fn no_warning_on_a_cycle_free_graph() {
        let reg = registry([
            ("bioconda", Some("conda-forge"), None),
            ("conda-forge", None, None),
        ]);
        let _ = resolve(&["bioconda"], &reg);
        assert!(!logs_contain("break a cycle"));
    }
}
