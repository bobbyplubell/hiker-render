//! Feasible (tight) tree construction — a port of
//! `dagre/lib/rank/feasible-tree.ts`.
//!
//! Builds a spanning tree of *tight* edges (slack 0), adjusting node ranks so
//! every tree edge is tight. The returned tree is an **undirected** [`Graph`]
//! whose node labels carry `low`/`lim`/`parent` ([`TreeNodeLabel`]) and whose
//! edge labels carry `cutvalue` ([`TreeEdgeLabel`]); these are filled in later
//! by network-simplex.
//!
//! Test names in the `#[cfg(test)]` module mirror
//! `dagre/test/rank/feasible-tree-test.ts`.

use crate::layered::graph::{Edge, Graph, GraphOptions};
use crate::layered::types::DagreGraph;

use super::util::slack;

/// Tree-graph node label — TS `TreeNodeLabel { low?, lim?, parent? }`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeNodeLabel {
    pub low: Option<i32>,
    pub lim: Option<i32>,
    pub parent: Option<String>,
}

/// Tree-graph edge label — TS `TreeEdgeLabel { cutvalue? }`.
///
/// Cut values are kept as `f64` because `calcCutValue` accumulates edge
/// `weight`s, which dagre models as floating point (e.g. after `simplify`
/// sums multi-edge weights).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeEdgeLabel {
    pub cutvalue: Option<f64>,
}

/// The undirected tree graph used by network-simplex:
/// `Graph<object, TreeNodeLabel, TreeEdgeLabel>`.
pub type TreeGraph = Graph<(), TreeNodeLabel, TreeEdgeLabel>;

/// Construct an undirected, directed=false tree graph with the default
/// (empty) node/edge labels, matching `new Graph({directed: false})`.
pub(crate) fn new_tree() -> TreeGraph {
    let mut t: TreeGraph = Graph::new(GraphOptions {
        directed: false,
        multigraph: false,
        compound: false,
    });
    t.set_default_node_label(TreeNodeLabel::default());
    t.set_default_edge_label(TreeEdgeLabel::default());
    t
}

/// `feasibleTree(graph)` — build a tight spanning tree, shifting `graph` node
/// ranks so all tree edges are tight, and return the tree.
///
/// # Panics
/// Panics if `graph` has no nodes (matching the TS thrown error).
pub fn feasible_tree(graph: &mut DagreGraph) -> TreeGraph {
    let mut tree = new_tree();

    let nodes = graph.nodes();
    if nodes.is_empty() {
        panic!("Graph must have at least one node");
    }
    let start = nodes[0].clone();
    let size = graph.node_count();
    tree.set_node(start.clone(), TreeNodeLabel::default());

    while tight_tree(&mut tree, graph) < size {
        let edge = match find_min_slack_edge(&tree, graph) {
            Some(e) => e,
            None => break,
        };
        let delta = if tree.has_node(&edge.v) {
            slack(graph, &edge)
        } else {
            -slack(graph, &edge)
        };
        shift_ranks(&tree, graph, delta);
    }

    tree
}

/// Finds a maximal tree of tight edges (growing `tree` in place) and returns
/// the resulting node count.
fn tight_tree(tree: &mut TreeGraph, graph: &DagreGraph) -> usize {
    // graphlib's forEach over a snapshot of nodes(); the recursive dfs may add
    // nodes, but the outer loop iterates the original list (new nodes are
    // reached via recursion).
    let initial: Vec<String> = tree.nodes();
    for v in initial {
        dfs_tight(tree, graph, &v);
    }
    tree.node_count()
}

fn dfs_tight(tree: &mut TreeGraph, graph: &DagreGraph, v: &str) {
    let node_edges = graph.node_edges(v, None).unwrap_or_default();
    for e in node_edges {
        let edge_v = &e.v;
        let w = if v == edge_v { e.w.clone() } else { edge_v.clone() };
        if !tree.has_node(&w) && slack(graph, &e) == 0 {
            tree.set_node(w.clone(), TreeNodeLabel::default());
            tree.set_edge(v.to_string(), w.clone(), TreeEdgeLabel::default(), None);
            dfs_tight(tree, graph, &w);
        }
    }
}

/// Finds the edge of `graph` with the smallest slack that has exactly one
/// endpoint in `tree`.
fn find_min_slack_edge(tree: &TreeGraph, graph: &DagreGraph) -> Option<Edge> {
    let mut best_slack = i32::MAX;
    let mut best: Option<Edge> = None;
    for edge in graph.edges() {
        let edge_slack = if tree.has_node(&edge.v) != tree.has_node(&edge.w) {
            slack(graph, &edge)
        } else {
            i32::MAX
        };
        if edge_slack < best_slack {
            best_slack = edge_slack;
            best = Some(edge);
        }
    }
    best
}

fn shift_ranks(tree: &TreeGraph, graph: &mut DagreGraph, delta: i32) {
    for v in tree.nodes() {
        if let Some(node) = graph.node_mut(&v) {
            if let Some(r) = node.rank {
                node.rank = Some(r + delta);
            }
        }
    }
}

#[cfg(test)]
mod tests;
