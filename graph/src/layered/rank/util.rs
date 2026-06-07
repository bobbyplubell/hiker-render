//! Rank utilities — a port of `dagre/lib/rank/util.ts`.
//!
//! [`longest_path`] assigns initial (unnormalized) ranks via a DFS that pushes
//! each node to the lowest rank its out-edges permit; [`slack`] returns the
//! amount an edge could be shortened (`rank[w] - rank[v] - minlen`).
//!
//! Test names in the `#[cfg(test)]` module mirror `dagre/test/rank/util-test.ts`.

use std::collections::HashSet;

use crate::layered::graph::Edge;
use crate::layered::types::DagreGraph;

/// `slack(graph, edge)` — `rank[w] - rank[v] - minlen`. `minlen` defaults to 1.
///
/// # Panics
/// Mirrors the TS (which would read `undefined` ranks): both endpoints are
/// expected to have a `rank` assigned.
pub fn slack(graph: &DagreGraph, edge: &Edge) -> i32 {
    let w_rank = graph
        .node(&edge.w)
        .and_then(|n| n.rank)
        .expect("slack: edge.w has no rank");
    let v_rank = graph
        .node(&edge.v)
        .and_then(|n| n.rank)
        .expect("slack: edge.v has no rank");
    let minlen = graph.edge_by_obj(edge).and_then(|e| e.minlen).unwrap_or(1);
    w_rank - v_rank - minlen
}

/// `longestPath(graph)` — assigns an initial `rank` to every node.
///
/// DFS from each source; a node's rank is the minimum over its out-edges of
/// `rank(w) - minlen(e)`. Nodes with no out-edges (or unreachable sinks) get
/// rank 0. Ranks are **not** normalized here (callers normalize later).
pub fn longest_path(graph: &mut DagreGraph) {
    let mut visited: HashSet<String> = HashSet::new();
    for v in graph.sources() {
        dfs(graph, &v, &mut visited);
    }
}

fn dfs(graph: &mut DagreGraph, v: &str, visited: &mut HashSet<String>) -> i32 {
    if visited.contains(v) {
        // Already assigned during this run.
        return graph.node(v).and_then(|n| n.rank).unwrap_or(0);
    }
    visited.insert(v.to_string());

    // `applyWithChunking(Math.min, outEdges.map(...))`, with Math.min() ===
    // +Infinity for the no-out-edge case (mapped to rank 0 below).
    let mut rank: Option<i32> = None;
    let out_edges = graph.out_edges(v, None).unwrap_or_default();
    for e in out_edges {
        let minlen = graph.edge_by_obj(&e).and_then(|l| l.minlen).unwrap_or(1);
        let candidate = dfs(graph, &e.w, visited) - minlen;
        rank = Some(match rank {
            Some(r) => r.min(candidate),
            None => candidate,
        });
    }

    let rank = rank.unwrap_or(0);
    if let Some(node) = graph.node_mut(v) {
        node.rank = Some(rank);
    }
    rank
}

#[cfg(test)]
mod tests;
