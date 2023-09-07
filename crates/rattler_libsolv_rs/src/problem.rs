//! Types to examine why a given [`crate::SolveJobs`] was unsatisfiable, and to report the causes
//! to the user

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::Formatter;
use std::rc::Rc;

use itertools::Itertools;
use petgraph::graph::{DiGraph, EdgeIndex, EdgeReference, NodeIndex};
use petgraph::visit::{Bfs, DfsPostOrder, EdgeRef};
use petgraph::Direction;

use crate::id::{ClauseId, SolvableId, VersionSetId};
use crate::pool::Pool;
use crate::solver::clause::Clause;
use crate::solver::Solver;
use crate::{DependencyProvider, VersionSet, VersionTrait};

/// Represents the cause of the solver being unable to find a solution
#[derive(Debug)]
pub struct Problem {
    /// The clauses involved in an unsatisfiable conflict
    clauses: Vec<ClauseId>,
}

impl Problem {
    pub(crate) fn default() -> Self {
        Self {
            clauses: Vec::new(),
        }
    }

    pub(crate) fn add_clause(&mut self, clause_id: ClauseId) {
        if !self.clauses.contains(&clause_id) {
            self.clauses.push(clause_id);
        }
    }

    /// Generates a graph representation of the problem (see [`ProblemGraph`] for details)
    pub fn graph<VS: VersionSet, D: DependencyProvider<VS>>(
        &self,
        solver: &Solver<VS, D>,
    ) -> ProblemGraph {
        let mut graph = DiGraph::<ProblemNode, ProblemEdge>::default();
        let mut nodes: HashMap<SolvableId, NodeIndex> = HashMap::default();

        let root_node = Self::add_node(&mut graph, &mut nodes, SolvableId::root());
        let unresolved_node = graph.add_node(ProblemNode::UnresolvedDependency);

        for clause_id in &self.clauses {
            let clause = &solver.clauses[clause_id.index()];
            match clause.kind {
                Clause::InstallRoot => (),
                Clause::Learnt(..) => unreachable!(),
                Clause::Requires(package_id, match_spec_id) => {
                    let package_node = Self::add_node(&mut graph, &mut nodes, package_id);

                    let candidates = &solver.pool().match_spec_to_sorted_candidates[match_spec_id];
                    if candidates.is_empty() {
                        tracing::info!(
                            "{package_id:?} requires {match_spec_id:?}, which has no candidates"
                        );
                        graph.add_edge(
                            package_node,
                            unresolved_node,
                            ProblemEdge::Requires(match_spec_id),
                        );
                    } else {
                        for &candidate_id in candidates {
                            tracing::info!("{package_id:?} requires {candidate_id:?}");

                            let candidate_node =
                                Self::add_node(&mut graph, &mut nodes, candidate_id);
                            graph.add_edge(
                                package_node,
                                candidate_node,
                                ProblemEdge::Requires(match_spec_id),
                            );
                        }
                    }
                }
                Clause::Lock(locked, forbidden) => {
                    let node2_id = Self::add_node(&mut graph, &mut nodes, forbidden);
                    let conflict = ConflictCause::Locked(locked);
                    graph.add_edge(root_node, node2_id, ProblemEdge::Conflict(conflict));
                }
                Clause::ForbidMultipleInstances(instance1_id, instance2_id) => {
                    let node1_id = Self::add_node(&mut graph, &mut nodes, instance1_id);
                    let node2_id = Self::add_node(&mut graph, &mut nodes, instance2_id);

                    let conflict = ConflictCause::ForbidMultipleInstances;
                    graph.add_edge(node1_id, node2_id, ProblemEdge::Conflict(conflict));
                }
                Clause::Constrains(package_id, dep_id, version_set_id) => {
                    let package_node = Self::add_node(&mut graph, &mut nodes, package_id);
                    let dep_node = Self::add_node(&mut graph, &mut nodes, dep_id);

                    graph.add_edge(
                        package_node,
                        dep_node,
                        ProblemEdge::Conflict(ConflictCause::Constrains(version_set_id)),
                    );
                }
            }
        }

        let unresolved_node = if graph
            .edges_directed(unresolved_node, Direction::Incoming)
            .next()
            .is_none()
        {
            graph.remove_node(unresolved_node);
            None
        } else {
            Some(unresolved_node)
        };

        // Sanity check: all nodes are reachable from root
        let mut visited_nodes = HashSet::new();
        let mut bfs = Bfs::new(&graph, root_node);
        while let Some(nx) = bfs.next(&graph) {
            visited_nodes.insert(nx);
        }
        assert_eq!(graph.node_count(), visited_nodes.len());

        ProblemGraph {
            graph,
            root_node,
            unresolved_node,
        }
    }

    fn add_node(
        graph: &mut DiGraph<ProblemNode, ProblemEdge>,
        nodes: &mut HashMap<SolvableId, NodeIndex>,
        solvable_id: SolvableId,
    ) -> NodeIndex {
        *nodes
            .entry(solvable_id)
            .or_insert_with(|| graph.add_node(ProblemNode::Solvable(solvable_id)))
    }

    /// Display a user-friendly error explaining the problem
    pub fn display_user_friendly<'a, VS: VersionSet, D: DependencyProvider<VS>>(
        &self,
        solver: &'a Solver<VS, D>,
    ) -> DisplayUnsat<'a, VS> {
        let graph = self.graph(solver);
        DisplayUnsat::new(graph, solver.pool())
    }
}

/// A node in the graph representation of a [`Problem`]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum ProblemNode {
    /// Node corresponding to a solvable
    Solvable(SolvableId),
    /// Node representing a dependency without candidates
    UnresolvedDependency,
}

impl ProblemNode {
    fn solvable_id(self) -> SolvableId {
        match self {
            ProblemNode::Solvable(solvable_id) => solvable_id,
            ProblemNode::UnresolvedDependency => {
                panic!("expected solvable node, found unresolved dependency")
            }
        }
    }
}

/// An edge in the graph representation of a [`Problem`]
#[derive(Copy, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub enum ProblemEdge {
    /// The target node is a candidate for the dependency specified by the match spec
    Requires(VersionSetId),
    /// The target node is involved in a conflict, caused by `ConflictCause`
    Conflict(ConflictCause),
}

impl ProblemEdge {
    fn try_requires(self) -> Option<VersionSetId> {
        match self {
            ProblemEdge::Requires(match_spec_id) => Some(match_spec_id),
            ProblemEdge::Conflict(_) => None,
        }
    }

    fn requires(self) -> VersionSetId {
        match self {
            ProblemEdge::Requires(match_spec_id) => match_spec_id,
            ProblemEdge::Conflict(_) => panic!("expected requires edge, found conflict"),
        }
    }
}

/// Conflict causes
#[derive(Copy, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub enum ConflictCause {
    /// The solvable is locked
    Locked(SolvableId),
    /// The target node is constrained by the specified match spec
    Constrains(VersionSetId),
    /// It is forbidden to install multiple instances of the same dependency
    ForbidMultipleInstances,
}

/// Represents a node that has been merged with others
///
/// Merging is done to simplify error messages, and happens when a group of nodes satisfies the
/// following criteria:
///
/// - They all have the same name
/// - They all have the same predecessor nodes
/// - They all have the same successor nodes
/// - None of them have incoming conflicting edges
pub(crate) struct MergedProblemNode {
    pub ids: Vec<SolvableId>,
}

/// Graph representation of [`Problem`]
///
/// The root of the graph is the "root solvable". Note that not all the solvable's requirements are
/// included in the graph, only those that are directly or indirectly involved in the conflict. See
/// [`ProblemNode`] and [`ProblemEdge`] for the kinds of nodes and edges that make up the graph.
pub struct ProblemGraph {
    graph: DiGraph<ProblemNode, ProblemEdge>,
    root_node: NodeIndex,
    unresolved_node: Option<NodeIndex>,
}

impl ProblemGraph {
    /// Writes a graphviz graph that represents this instance to the specified output.
    pub fn graphviz<VS: VersionSet>(
        &self,
        f: &mut impl std::io::Write,
        pool: &Pool<VS>,
        simplify: bool,
    ) -> Result<(), std::io::Error> {
        let graph = &self.graph;

        let merged_nodes = if simplify {
            self.simplify(pool)
        } else {
            HashMap::new()
        };

        write!(f, "digraph {{")?;
        for nx in graph.node_indices() {
            let id = match graph.node_weight(nx).as_ref().unwrap() {
                ProblemNode::Solvable(id) => *id,
                _ => continue,
            };

            // If this is a merged node, skip it unless it is the first one in the group
            if let Some(merged) = merged_nodes.get(&id) {
                if id != merged.ids[0] {
                    continue;
                }
            }

            let solvable = pool.resolve_solvable_inner(id);
            let mut added_edges = HashSet::new();
            for edge in graph.edges_directed(nx, Direction::Outgoing) {
                let target = *graph.node_weight(edge.target()).unwrap();

                let color = match edge.weight() {
                    ProblemEdge::Requires(_) if target != ProblemNode::UnresolvedDependency => {
                        "black"
                    }
                    _ => "red",
                };

                let label = match edge.weight() {
                    ProblemEdge::Requires(version_set_id)
                    | ProblemEdge::Conflict(ConflictCause::Constrains(version_set_id)) => {
                        pool.resolve_version_set(*version_set_id).to_string()
                    }
                    ProblemEdge::Conflict(ConflictCause::ForbidMultipleInstances)
                    | ProblemEdge::Conflict(ConflictCause::Locked(_)) => {
                        "already installed".to_string()
                    }
                };

                let target = match target {
                    ProblemNode::Solvable(mut solvable_2) => {
                        // If the target node has been merged, replace it by the first id in the group
                        if let Some(merged) = merged_nodes.get(&solvable_2) {
                            solvable_2 = merged.ids[0];

                            // Skip the edge if we would be adding a duplicate
                            if !added_edges.insert(solvable_2) {
                                continue;
                            }
                        }

                        pool.resolve_solvable_inner(solvable_2).to_string()
                    }
                    ProblemNode::UnresolvedDependency => "unresolved".to_string(),
                };

                write!(
                    f,
                    "\"{}\" -> \"{}\"[color={color}, label=\"{label}\"];",
                    solvable, target
                )?;
            }
        }
        write!(f, "}}")
    }

    fn simplify<VS: VersionSet>(
        &self,
        pool: &Pool<VS>,
    ) -> HashMap<SolvableId, Rc<MergedProblemNode>> {
        let graph = &self.graph;

        // Gather information about nodes that can be merged
        let mut maybe_merge = HashMap::new();
        for node_id in graph.node_indices() {
            let candidate = match graph[node_id] {
                ProblemNode::UnresolvedDependency => continue,
                ProblemNode::Solvable(solvable_id) => {
                    if solvable_id.is_root() {
                        continue;
                    } else {
                        solvable_id
                    }
                }
            };

            if graph
                .edges_directed(node_id, Direction::Incoming)
                .any(|e| matches!(e.weight(), ProblemEdge::Conflict(..)))
            {
                // Nodes that are the target of a conflict should never be merged
                continue;
            }

            let predecessors: Vec<_> = graph
                .edges_directed(node_id, Direction::Incoming)
                .map(|e| e.source())
                .sorted_unstable()
                .collect();
            let successors: Vec<_> = graph
                .edges(node_id)
                .map(|e| e.target())
                .sorted_unstable()
                .collect();

            let name = pool.resolve_solvable(candidate).name;

            let entry = maybe_merge
                .entry((name, predecessors, successors))
                .or_insert(Vec::new());

            entry.push((node_id, candidate));
        }

        let mut merged_candidates = HashMap::default();
        // TODO: could probably use `sort_candidates` by the dependency provider directly
        // but we need to mantain the mapping in `m` which goes from `NodeIndex` to `SolvableId`
        for mut m in maybe_merge.into_values() {
            if m.len() > 1 {
                m.sort_unstable_by_key(|&(_, id)| pool.resolve_solvable(id).inner.version());
                let m = Rc::new(MergedProblemNode {
                    ids: m.into_iter().map(|(_, snd)| snd).collect(),
                });
                for &id in &m.ids {
                    merged_candidates.insert(id, m.clone());
                }
            }
        }

        merged_candidates
    }

    fn get_installable_set(&self) -> HashSet<NodeIndex> {
        let mut installable = HashSet::new();

        // Definition: a package is installable if it does not have any outgoing conflicting edges
        // and if each of its dependencies has at least one installable option.

        // Algorithm: propagate installability bottom-up
        let mut dfs = DfsPostOrder::new(&self.graph, self.root_node);
        'outer_loop: while let Some(nx) = dfs.next(&self.graph) {
            if self.unresolved_node == Some(nx) {
                // The unresolved node isn't installable
                continue;
            }

            let outgoing_conflicts = self
                .graph
                .edges_directed(nx, Direction::Outgoing)
                .any(|e| matches!(e.weight(), ProblemEdge::Conflict(_)));
            if outgoing_conflicts {
                // Nodes with outgoing conflicts aren't installable
                continue;
            }

            // Edges grouped by dependency
            let dependencies = self
                .graph
                .edges_directed(nx, Direction::Outgoing)
                .map(|e| match e.weight() {
                    ProblemEdge::Requires(version_set_id) => (version_set_id, e.target()),
                    ProblemEdge::Conflict(_) => unreachable!(),
                })
                .group_by(|(&version_set_id, _)| version_set_id);

            for (_, mut deps) in &dependencies {
                if deps.all(|(_, target)| !installable.contains(&target)) {
                    // No installable options for this dep
                    continue 'outer_loop;
                }
            }

            // The package is installable!
            installable.insert(nx);
        }

        installable
    }

    fn get_missing_set(&self) -> HashSet<NodeIndex> {
        // Definition: a package is missing if it is not involved in any conflicts, yet it is not
        // installable

        let mut missing = HashSet::new();
        match self.unresolved_node {
            None => return missing,
            Some(nx) => missing.insert(nx),
        };

        // Algorithm: propagate missing bottom-up
        let mut dfs = DfsPostOrder::new(&self.graph, self.root_node);
        while let Some(nx) = dfs.next(&self.graph) {
            let outgoing_conflicts = self
                .graph
                .edges_directed(nx, Direction::Outgoing)
                .any(|e| matches!(e.weight(), ProblemEdge::Conflict(_)));
            if outgoing_conflicts {
                // Nodes with outgoing conflicts aren't missing
                continue;
            }

            // Edges grouped by dependency
            let dependencies = self
                .graph
                .edges_directed(nx, Direction::Outgoing)
                .map(|e| match e.weight() {
                    ProblemEdge::Requires(version_set_id) => (version_set_id, e.target()),
                    ProblemEdge::Conflict(_) => unreachable!(),
                })
                .group_by(|(&version_set_id, _)| version_set_id);

            // Missing if at least one dependency is missing
            if dependencies
                .into_iter()
                .any(|(_, mut deps)| deps.all(|(_, target)| missing.contains(&target)))
            {
                missing.insert(nx);
            }
        }

        missing
    }
}

/// A struct implementing [`fmt::Display`] that generates a user-friendly representation of a
/// problem graph
pub struct DisplayUnsat<'pool, VS: VersionSet> {
    graph: ProblemGraph,
    merged_candidates: HashMap<SolvableId, Rc<MergedProblemNode>>,
    installable_set: HashSet<NodeIndex>,
    missing_set: HashSet<NodeIndex>,
    pool: &'pool Pool<VS>,
}

impl<'pool, VS: VersionSet> DisplayUnsat<'pool, VS> {
    pub(crate) fn new(graph: ProblemGraph, pool: &'pool Pool<VS>) -> Self {
        let merged_candidates = graph.simplify(pool);
        let installable_set = graph.get_installable_set();
        let missing_set = graph.get_missing_set();

        Self {
            graph,
            merged_candidates,
            installable_set,
            missing_set,
            pool,
        }
    }

    fn get_indent(depth: usize, top_level_indent: bool) -> String {
        let depth_correction = if depth > 0 && !top_level_indent { 1 } else { 0 };

        let mut indent = " ".repeat((depth - depth_correction) * 4);

        let display_tree_char = depth != 0 || top_level_indent;
        if display_tree_char {
            indent.push_str("|-- ");
        }

        indent
    }

    fn fmt_graph(
        &self,
        f: &mut Formatter<'_>,
        top_level_edges: &[EdgeReference<ProblemEdge>],
        top_level_indent: bool,
    ) -> fmt::Result {
        pub enum DisplayOp {
            Requirement(VersionSetId, Vec<EdgeIndex>),
            Candidate(NodeIndex),
        }

        let graph = &self.graph.graph;
        let installable_nodes = &self.installable_set;
        let mut reported: HashSet<SolvableId> = HashSet::new();

        // Note: we are only interested in requires edges here
        let mut stack = top_level_edges
            .iter()
            .filter(|e| e.weight().try_requires().is_some())
            .group_by(|e| e.weight().requires())
            .into_iter()
            .map(|(version_set_id, group)| {
                let edges: Vec<_> = group.map(|e| e.id()).collect();
                (version_set_id, edges)
            })
            .sorted_by_key(|(_version_set_id, edges)| {
                edges
                    .iter()
                    .any(|&edge| installable_nodes.contains(&graph.edge_endpoints(edge).unwrap().1))
            })
            .map(|(version_set_id, edges)| (DisplayOp::Requirement(version_set_id, edges), 0))
            .collect::<Vec<_>>();
        while let Some((node, depth)) = stack.pop() {
            let indent = Self::get_indent(depth, top_level_indent);

            match node {
                DisplayOp::Requirement(version_set_id, edges) => {
                    debug_assert!(!edges.is_empty());

                    let installable = edges.iter().any(|&e| {
                        let (_, target) = graph.edge_endpoints(e).unwrap();
                        installable_nodes.contains(&target)
                    });

                    let req = self.pool.resolve_version_set(version_set_id).to_string();
                    let name = self.pool.resolve_version_set_package_name(version_set_id);
                    let name = self.pool.resolve_package_name(name);
                    let target_nx = graph.edge_endpoints(edges[0]).unwrap().1;
                    let missing =
                        edges.len() == 1 && graph[target_nx] == ProblemNode::UnresolvedDependency;
                    if missing {
                        // No candidates for requirement
                        if depth == 0 {
                            writeln!(f, "{indent}No candidates were found for {name} {req}.")?;
                        } else {
                            writeln!(
                                f,
                                "{indent}{name} {req}, for which no candidates were found.",
                            )?;
                        }
                    } else if installable {
                        // Package can be installed (only mentioned for top-level requirements)
                        if depth == 0 {
                            writeln!(
                                f,
                                "{indent}{name} {req} can be installed with any of the following options:"
                            )?;
                        } else {
                            writeln!(f, "{indent}{name} {req}, which can be installed with any of the following options:")?;
                        }

                        stack.extend(
                            edges
                                .iter()
                                .filter(|&&e| {
                                    installable_nodes.contains(&graph.edge_endpoints(e).unwrap().1)
                                })
                                .map(|&e| {
                                    (
                                        DisplayOp::Candidate(graph.edge_endpoints(e).unwrap().1),
                                        depth + 1,
                                    )
                                }),
                        );
                    } else {
                        // Package cannot be installed (the conflicting requirement is further down the tree)
                        if depth == 0 {
                            writeln!(f, "{indent}{name} {req} cannot be installed because there are no viable options:")?;
                        } else {
                            writeln!(f, "{indent}{name} {req}, which cannot be installed because there are no viable options:")?;
                        }

                        stack.extend(edges.iter().map(|&e| {
                            (
                                DisplayOp::Candidate(graph.edge_endpoints(e).unwrap().1),
                                depth + 1,
                            )
                        }));
                    }
                }
                DisplayOp::Candidate(candidate) => {
                    let solvable_id = graph[candidate].solvable_id();

                    if reported.contains(&solvable_id) {
                        continue;
                    }

                    let solvable = self.pool.resolve_solvable(solvable_id);
                    let name = self.pool.resolve_package_name(solvable.name);
                    let version = if let Some(merged) = self.merged_candidates.get(&solvable_id) {
                        reported.extend(merged.ids.iter().cloned());
                        merged
                            .ids
                            .iter()
                            .map(|&id| self.pool.resolve_solvable(id).inner.version().to_string())
                            .join(" | ")
                    } else {
                        solvable.inner.version().to_string()
                    };

                    let already_installed = graph.edges(candidate).any(|e| {
                        e.weight() == &ProblemEdge::Conflict(ConflictCause::ForbidMultipleInstances)
                    });
                    let constrains_conflict = graph.edges(candidate).any(|e| {
                        matches!(
                            e.weight(),
                            ProblemEdge::Conflict(ConflictCause::Constrains(_))
                        )
                    });
                    let is_leaf = graph.edges(candidate).next().is_none();

                    if is_leaf {
                        writeln!(f, "{indent}{} {version}", solvable.name.display(self.pool))?;
                    } else if already_installed {
                        writeln!(f, "{indent}{} {version}, which conflicts with the versions reported above.", solvable.name.display(self.pool))?;
                    } else if constrains_conflict {
                        let version_sets = graph
                            .edges(candidate)
                            .flat_map(|e| match e.weight() {
                                ProblemEdge::Conflict(ConflictCause::Constrains(
                                    version_set_id,
                                )) => Some(version_set_id),
                                _ => None,
                            })
                            .dedup();


                        writeln!(
                            f,
                            "{indent}{} {version} would constrain",
                            solvable.name.display(self.pool)
                        )?;

                        let indent = Self::get_indent(depth + 1, top_level_indent);
                        for &version_set_id in version_sets {
                            let version_set = self.pool.resolve_version_set(version_set_id);
                            writeln!(
                                f,
                                "{indent}{} , which conflicts with any installable versions previously reported",
                                version_set
                            )?;
                        }
                    } else {
                        writeln!(
                            f,
                            "{indent}{} {version} would require",
                            solvable.name.display(self.pool)
                        )?;
                        let requirements = graph
                            .edges(candidate)
                            .group_by(|e| e.weight().requires())
                            .into_iter()
                            .map(|(version_set_id, group)| {
                                let edges: Vec<_> = group.map(|e| e.id()).collect();
                                (version_set_id, edges)
                            })
                            .sorted_by_key(|(_version_set_id, edges)| {
                                edges.iter().any(|&edge| {
                                    installable_nodes
                                        .contains(&graph.edge_endpoints(edge).unwrap().1)
                                })
                            })
                            .map(|(version_set_id, edges)| {
                                (DisplayOp::Requirement(version_set_id, edges), depth + 1)
                            });

                        stack.extend(requirements);
                    }
                }
            }
        }

        Ok(())
    }
}

impl<VS: VersionSet> fmt::Display for DisplayUnsat<'_, VS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (top_level_missing, top_level_conflicts): (Vec<_>, _) = self
            .graph
            .graph
            .edges(self.graph.root_node)
            .partition(|e| self.missing_set.contains(&e.target()));

        if !top_level_missing.is_empty() {
            self.fmt_graph(f, &top_level_missing, false)?;
        }

        if !top_level_conflicts.is_empty() {
            writeln!(f, "The following packages are incompatible")?;
            self.fmt_graph(f, &top_level_conflicts, true)?;

            // Conflicts caused by locked dependencies
            let indent = Self::get_indent(0, true);
            for e in self.graph.graph.edges(self.graph.root_node) {
                let conflict = match e.weight() {
                    ProblemEdge::Requires(_) => continue,
                    ProblemEdge::Conflict(conflict) => conflict,
                };

                // The only possible conflict at the root level is a Locked conflict
                let locked_id = match conflict {
                    ConflictCause::Constrains(_) | ConflictCause::ForbidMultipleInstances => {
                        unreachable!()
                    }
                    &ConflictCause::Locked(solvable_id) => solvable_id,
                };

                let locked = self.pool.resolve_solvable(locked_id);
                writeln!(
                    f,
                    "{indent}{} {} is locked, but another version is required as reported above",
                    locked.name.display(self.pool),
                    locked.inner.version()
                )?;
            }
        }

        Ok(())
    }
}
