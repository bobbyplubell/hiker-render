//! Make a graph acyclic by reversing a feedback set — a port of
//! `dagre/lib/acyclic.ts`.
//!
//! [`run`] chooses an acyclicer (`Acyclicer::Greedy` → [`greedy_fas`] with a
//! weight fn reading `edge.weight`; otherwise the DFS feedback set [`dfs_fas`]),
//! then reverses each edge in the feedback set in place. [`undo`] restores the
//! original directions and labels.
//!
//! # Edge-reversal relabeling (must match dagre exactly)
//!
//! For each feedback edge `e` with label `label`:
//! 1. `removeEdge(e)`,
//! 2. `label.forwardName = e.name` (stash the original edge name),
//! 3. `label.reversed = true`,
//! 4. `setEdge(e.w, e.v, label, uniqueId("rev"))` — re-add reversed with a fresh
//!    unique name so it never collides with an existing back-edge (multigraph).
//!
//! `undo` walks every edge; for any with `reversed == Some(true)` it removes the
//! edge, clears `reversed`/`forward_name`, and re-adds `setEdge(e.w, e.v, label,
//! forward_name)` — restoring both the direction *and* the original name. This
//! round-trip is what later normalize/undo passes depend on.

use super::graph::{Edge, Graph};
use super::greedy_fas::greedy_fas;
use super::types::{Acyclicer, EdgeLabel, GraphLabel, NodeLabel};
use super::util::unique_id;

type DagreGraph = Graph<GraphLabel, NodeLabel, EdgeLabel>;

/// `run(graph)` — reverse a feedback arc set in place so `graph` becomes
/// acyclic.
pub fn run(graph: &mut DagreGraph) {
    let acyclicer = graph.graph().and_then(|g| g.acyclicer);
    let fas = if acyclicer == Some(Acyclicer::Greedy) {
        // weightFn = (e) => g.edge(e)!.weight!
        let wf = |e: &Edge| graph.edge_by_obj(e).and_then(|l| l.weight).unwrap();
        greedy_fas(graph, Some(&wf))
    } else {
        dfs_fas(graph)
    };

    for e in fas {
        let mut label = graph.edge_by_obj(&e).cloned().unwrap_or_default();
        graph.remove_edge_obj(&e);
        label.forward_name = e.name.clone();
        label.reversed = Some(true);
        graph.set_edge(e.w.clone(), e.v.clone(), label, Some(&unique_id("rev")));
    }
}

/// `dfsFAS(graph)` — collect back-edges found by a depth-first traversal. A
/// back-edge (an out-edge whose target is currently on the DFS stack) closes a
/// cycle, so it is added to the feedback set.
fn dfs_fas(graph: &DagreGraph) -> Vec<Edge> {
    let mut fas: Vec<Edge> = Vec::new();
    let mut stack: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();

    fn dfs(
        graph: &DagreGraph,
        v: &str,
        stack: &mut std::collections::HashSet<String>,
        visited: &mut std::collections::HashSet<String>,
        fas: &mut Vec<Edge>,
    ) {
        if visited.contains(v) {
            return;
        }
        visited.insert(v.to_string());
        stack.insert(v.to_string());
        if let Some(out_edges) = graph.out_edges(v, None) {
            for e in out_edges {
                if stack.contains(&e.w) {
                    fas.push(e);
                } else {
                    dfs(graph, &e.w, stack, visited, fas);
                }
            }
        }
        stack.remove(v);
    }

    for v in graph.nodes() {
        dfs(graph, &v, &mut stack, &mut visited, &mut fas);
    }
    fas
}

/// `undo(graph)` — restore every reversed edge to its original direction, name,
/// and label.
pub fn undo(graph: &mut DagreGraph) {
    for e in graph.edges() {
        let label = match graph.edge_by_obj(&e) {
            Some(l) => l.clone(),
            None => continue,
        };
        if label.reversed == Some(true) {
            graph.remove_edge_obj(&e);
            let forward_name = label.forward_name.clone();
            let mut label = label;
            label.reversed = None;
            label.forward_name = None;
            graph.set_edge(e.w.clone(), e.v.clone(), label, forward_name.as_deref());
        }
    }
}

#[cfg(test)]
mod tests;
