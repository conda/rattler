use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::Formatter;
use std::rc::Rc;

use itertools::Itertools;
use petgraph::graph::{DiGraph, EdgeIndex, NodeIndex};
use petgraph::visit::{Bfs, EdgeRef};
use petgraph::Direction;

use crate::pool::{MatchSpecId, Pool};
use crate::rules::RuleKind;
use crate::solvable::SolvableId;
use crate::solver::{RuleId, Solver};

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum ProblemNode {
    Solvable(SolvableId),
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

#[derive(Copy, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub enum ProblemEdge {
    Requires(MatchSpecId),
    Conflict(Conflict),
}

impl ProblemEdge {
    fn try_requires(self) -> Option<MatchSpecId> {
        match self {
            ProblemEdge::Requires(match_spec_id) => Some(match_spec_id),
            ProblemEdge::Conflict(_) => None,
        }
    }

    fn requires(self) -> MatchSpecId {
        match self {
            ProblemEdge::Requires(match_spec_id) => match_spec_id,
            ProblemEdge::Conflict(_) => panic!("expected requires edge, found conflict"),
        }
    }
}

#[derive(Copy, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub enum Conflict {
    Locked(SolvableId),
    Constrains(MatchSpecId),
    ForbidMultipleInstances,
}

pub struct MergedProblemNode {
    pub ids: Vec<SolvableId>,
}

#[derive(Debug)]
pub struct Problem {
    rules: Vec<RuleId>,
}

impl Problem {
    pub(crate) fn default() -> Self {
        Self { rules: Vec::new() }
    }

    pub(crate) fn add_rule(&mut self, rule_id: RuleId) {
        if !self.rules.contains(&rule_id) {
            self.rules.push(rule_id);
        }
    }

    pub fn graph(&self, solver: &Solver) -> ProblemGraph {
        println!("=== Build graph");
        let mut graph = DiGraph::<ProblemNode, ProblemEdge>::default();
        let mut nodes: HashMap<SolvableId, NodeIndex> = HashMap::default();

        let root_node = Self::add_node(&mut graph, &mut nodes, SolvableId::root());
        let unresolved_node = graph.add_node(ProblemNode::UnresolvedDependency);

        for rule_id in &self.rules {
            let rule = &solver.rules[rule_id.index()];
            match rule.kind {
                RuleKind::InstallRoot => (),
                RuleKind::Learnt(..) => unreachable!(),
                RuleKind::Requires(package_id, match_spec_id) => {
                    let package_node = Self::add_node(&mut graph, &mut nodes, package_id);

                    let candidates = solver.pool().match_spec_to_candidates[match_spec_id.index()]
                        .as_deref()
                        .unwrap();
                    if candidates.is_empty() {
                        println!(
                            "{package_id:?} requires {match_spec_id:?}, which has no candidates"
                        );
                        graph.add_edge(
                            package_node,
                            unresolved_node,
                            ProblemEdge::Requires(match_spec_id),
                        );
                    } else {
                        for &candidate_id in candidates {
                            println!("{package_id:?} requires {candidate_id:?}");

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
                RuleKind::ForbidMultipleInstances(instance1_id, instance2_id) => {
                    let node1_id = Self::add_node(&mut graph, &mut nodes, instance1_id);
                    let node2_id = Self::add_node(&mut graph, &mut nodes, instance2_id);

                    let conflict = if instance1_id.is_root() {
                        Conflict::Locked(instance2_id)
                    } else {
                        Conflict::ForbidMultipleInstances
                    };
                    graph.add_edge(node1_id, node2_id, ProblemEdge::Conflict(conflict));
                }
                RuleKind::Constrains(package_id, dep_id) => {
                    let package_node = Self::add_node(&mut graph, &mut nodes, package_id);
                    let dep_node = Self::add_node(&mut graph, &mut nodes, dep_id);

                    let package = solver.pool().resolve_solvable(package_id);
                    let dep = solver.pool().resolve_solvable(dep_id);
                    let ms_id = package
                        .constrains
                        .iter()
                        .cloned()
                        .find(|&ms| {
                            let ms = solver.pool().resolve_match_spec(ms);
                            ms.name.as_deref().unwrap() == dep.record.name
                        })
                        .unwrap();

                    graph.add_edge(
                        package_node,
                        dep_node,
                        ProblemEdge::Conflict(Conflict::Constrains(ms_id)),
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
            unresolved_dependency_node: unresolved_node,
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

    pub fn display_user_friendly<'a>(&self, solver: &'a Solver) -> DisplayUnsat<'a> {
        let graph = self.graph(solver);

        // TODO: remove
        graph.graphviz(solver.pool());

        DisplayUnsat::new(graph, solver.pool())
    }
}

pub struct ProblemGraph {
    graph: DiGraph<ProblemNode, ProblemEdge>,
    root_node: NodeIndex,
    unresolved_dependency_node: Option<NodeIndex>,
}

impl ProblemGraph {
    fn graphviz(&self, pool: &Pool) {
        let graph = &self.graph;

        println!("digraph {{");
        let mut bfs = Bfs::new(&graph, self.root_node);
        while let Some(nx) = bfs.next(&graph) {
            match graph.node_weight(nx).as_ref().unwrap() {
                ProblemNode::Solvable(id) => {
                    let solvable = pool.resolve_solvable_inner(*id);
                    for edge in graph.edges_directed(nx, Direction::Outgoing) {
                        let target = *graph.node_weight(edge.target()).unwrap();

                        let color = match edge.weight() {
                            ProblemEdge::Requires(_)
                                if target != ProblemNode::UnresolvedDependency =>
                            {
                                "black"
                            }
                            _ => "red",
                        };

                        let label = match edge.weight() {
                            ProblemEdge::Requires(match_spec_id)
                            | ProblemEdge::Conflict(Conflict::Constrains(match_spec_id)) => {
                                pool.resolve_match_spec(*match_spec_id).to_string()
                            }
                            ProblemEdge::Conflict(Conflict::ForbidMultipleInstances)
                            | ProblemEdge::Conflict(Conflict::Locked(_)) => {
                                "already installed".to_string()
                            }
                        };

                        let target = match target {
                            ProblemNode::Solvable(solvable_2) => pool
                                .resolve_solvable_inner(solvable_2)
                                .display()
                                .to_string(),
                            ProblemNode::UnresolvedDependency => "unresolved".to_string(),
                        };

                        println!(
                            "\"{}\" -> \"{}\"[color={color}, label=\"{label}\"];",
                            solvable.display(),
                            target
                        );
                    }
                }
                ProblemNode::UnresolvedDependency => {}
            }
        }
        println!("}}");
    }

    fn simplify(&self, pool: &Pool) -> HashMap<SolvableId, Rc<MergedProblemNode>> {
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
                .map(|e| (e.target(), *e.weight()))
                .sorted_unstable()
                .collect();

            let name = pool.resolve_solvable(candidate).name;

            let entry = maybe_merge
                .entry((name, predecessors, successors))
                .or_insert(Vec::new());

            entry.push((node_id, candidate));
        }

        let mut merged_candidates = HashMap::default();
        for mut m in maybe_merge.into_values() {
            if m.len() > 1 {
                m.sort_unstable_by_key(|&(_, id)| &pool.resolve_solvable(id).record.version);
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
        let mut non_installable: HashSet<NodeIndex> = HashSet::new();

        // Definition: a package is installable if all paths from it to the graph's leaves pass
        // through non-conflicting edges (i.e. each dependency, and each dependency's dependencies,
        // etc, can be installed)

        // Gather the starting set of conflicting edges:
        // * Edges into the unresolved dependency node
        // * Edges equal to `ProblemEdge::Conflict`
        let mut conflicting_edges = Vec::new();

        if let Some(unresolved_nx) = self.unresolved_dependency_node {
            conflicting_edges.extend(
                self.graph
                    .edges_directed(unresolved_nx, Direction::Incoming),
            );
        }

        conflicting_edges.extend(
            self.graph
                .edge_references()
                .filter(|e| matches!(e.weight(), ProblemEdge::Conflict(..))),
        );

        // Propagate conflicts up the graph
        while let Some(edge) = conflicting_edges.pop() {
            let source = edge.source();
            if non_installable.insert(source) {
                // Visited for the first time, so make sure the predecessors are also marked as non-installable
                conflicting_edges.extend(self.graph.edges_directed(source, Direction::Incoming));
            }
        }

        // Installable packages are all nodes that were not marked as non-installable
        self.graph
            .node_indices()
            .filter(|nx| !non_installable.contains(nx))
            .collect()
    }
}

pub struct DisplayUnsat<'a> {
    graph: ProblemGraph,
    merged_candidates: HashMap<SolvableId, Rc<MergedProblemNode>>,
    installable_set: HashSet<NodeIndex>,
    pool: &'a Pool,
}

impl<'a> DisplayUnsat<'a> {
    pub fn new(graph: ProblemGraph, pool: &'a Pool) -> Self {
        let merged_candidates = graph.simplify(pool);
        let installable_set = graph.get_installable_set();

        Self {
            graph,
            merged_candidates,
            installable_set,
            pool,
        }
    }
}

impl fmt::Display for DisplayUnsat<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let graph = &self.graph.graph;
        let installable_nodes = &self.installable_set;
        let mut reported: HashSet<SolvableId> = HashSet::new();

        pub enum DisplayOp {
            Requirement(MatchSpecId, Vec<EdgeIndex>),
            Candidate(NodeIndex),
        }

        writeln!(f, "The following packages are incompatible")?;

        // Note: we are only interested in requires edges here
        let mut stack = graph
            .edges(self.graph.root_node)
            .filter(|e| e.weight().try_requires().is_some())
            .group_by(|e| e.weight().requires())
            .into_iter()
            .map(|(match_spec_id, group)| {
                let edges: Vec<_> = group.map(|e| e.id()).collect();
                (match_spec_id, edges)
            })
            .sorted_by_key(|(_match_spec_id, edges)| {
                edges
                    .iter()
                    .any(|&edge| installable_nodes.contains(&graph.edge_endpoints(edge).unwrap().1))
            })
            .map(|(match_spec_id, edges)| (DisplayOp::Requirement(match_spec_id, edges), 0))
            .collect::<Vec<_>>();
        while let Some((node, depth)) = stack.pop() {
            let indent = " ".repeat(depth * 4);

            match node {
                DisplayOp::Requirement(match_spec_id, edges) => {
                    debug_assert!(!edges.is_empty());

                    let installable = edges.iter().any(|&e| {
                        let (_, target) = graph.edge_endpoints(e).unwrap();
                        installable_nodes.contains(&target)
                    });

                    let req = self.pool.resolve_match_spec(match_spec_id).to_string();
                    let target_nx = graph.edge_endpoints(edges[0]).unwrap().1;
                    let missing =
                        edges.len() == 1 && graph[target_nx] == ProblemNode::UnresolvedDependency;
                    if missing {
                        // No candidates for requirement
                        if depth == 0 {
                            writeln!(f, "{indent}|-- No candidates where found for {req}.")?;
                        } else {
                            writeln!(f, "{indent}|-- {req}, for which no candidates where found.",)?;
                        }
                    } else if installable {
                        // Package can be installed (only mentioned for top-level requirements)
                        if depth == 0 {
                            writeln!(
                                f,
                                "|-- {req} can be installed with any of the following options:"
                            )?;
                        } else {
                            writeln!(f, "{indent}|-- {req}, which can be installed with any of the following options:")?;
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
                            writeln!(f, "|-- {req} cannot be installed because there are no viable options:")?;
                        } else {
                            writeln!(f, "{indent}|-- {req}, which cannot be installed because there are no viable options:")?;
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
                    let version = if let Some(merged) = self.merged_candidates.get(&solvable_id) {
                        reported.extend(merged.ids.iter().cloned());
                        merged
                            .ids
                            .iter()
                            .map(|&id| self.pool.resolve_solvable(id).record.version.to_string())
                            .join(" | ")
                    } else {
                        solvable.record.version.to_string()
                    };

                    let is_conflict_source = graph
                        .edges(candidate)
                        .any(|e| e.weight().try_requires().is_none());
                    let is_leaf = graph
                        .edges(candidate)
                        .next()
                        .is_none();

                    if is_conflict_source {
                        writeln!(f, "{indent}|-- {} {version}, which conflicts with the versions reported above.", solvable.record.name)?;
                    } else if is_leaf {
                        writeln!(f, "{indent}|-- {} {version}", solvable.record.name)?;
                    } else {
                        writeln!(
                            f,
                            "{indent}|-- {} {version} would require",
                            solvable.record.name
                        )?;
                        let requirements = graph
                            .edges(candidate)
                            .group_by(|e| e.weight().requires())
                            .into_iter()
                            .map(|(match_spec_id, group)| {
                                let edges: Vec<_> = group.map(|e| e.id()).collect();
                                (match_spec_id, edges)
                            })
                            .sorted_by_key(|(_match_spec_id, edges)| {
                                edges.iter().any(|&edge| {
                                    installable_nodes
                                        .contains(&graph.edge_endpoints(edge).unwrap().1)
                                })
                            })
                            .map(|(match_spec_id, edges)| {
                                (DisplayOp::Requirement(match_spec_id, edges), depth + 1)
                            });

                        stack.extend(requirements);
                    }
                }
            }
        }

        // Report conflicts caused by locked dependencies
        for e in graph.edges(self.graph.root_node) {
            let conflict = match e.weight() {
                ProblemEdge::Requires(_) => continue,
                ProblemEdge::Conflict(conflict) => conflict,
            };

            // The only possible conflict at the root level is a Locked conflict
            let locked_id = match conflict {
                Conflict::Constrains(_) | Conflict::ForbidMultipleInstances => unreachable!(),
                &Conflict::Locked(solvable_id) => solvable_id,
            };

            let locked = self.pool.resolve_solvable(locked_id);
            writeln!(
                f,
                "|-- {} {} is locked, but another version is required as reported above",
                locked.record.name, locked.record.version
            )?;
        }

        Ok(())
    }
}
