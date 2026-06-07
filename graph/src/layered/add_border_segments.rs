//! Add border segments — a port of `dagre/lib/add-border-segments.ts`.
//!
//! For every compound node that has been assigned a `minRank`/`maxRank` range
//! (i.e. a cluster), add left and right border-segment dummy nodes at each rank
//! in `[minRank, maxRank]`, parent them to the cluster, and chain consecutive
//! ranks' border nodes together vertically. The per-rank border node ids are
//! stored on the cluster's `borderLeft` / `borderRight` arrays (indexed by
//! rank).

use super::types::{BorderType, DagreGraph, DummyKind, EdgeLabel, NodeLabel};
use super::util::{add_dummy_node, GRAPH_NODE};

/// `addBorderSegments(graph)`.
pub fn add_border_segments(graph: &mut DagreGraph) {
    for v in graph.children(GRAPH_NODE) {
        dfs(graph, &v);
    }
}

fn dfs(graph: &mut DagreGraph, v: &str) {
    let children = graph.children(v);
    if !children.is_empty() {
        for child in &children {
            dfs(graph, child);
        }
    }

    // Object.hasOwn(node, "minRank")
    let has_min_rank = graph.node(v).map(|n| n.min_rank.is_some()).unwrap_or(false);
    if has_min_rank {
        let min_rank = graph.node(v).and_then(|n| n.min_rank).unwrap();
        let max_rank = graph.node(v).and_then(|n| n.max_rank).unwrap();

        if let Some(node) = graph.node_mut(v) {
            node.border_left = Some(Vec::new());
            node.border_right = Some(Vec::new());
        }

        let mut rank = min_rank;
        while rank < max_rank + 1 {
            add_border_node(graph, BorderType::BorderLeft, "_bl", v, rank);
            add_border_node(graph, BorderType::BorderRight, "_br", v, rank);
            rank += 1;
        }
    }
}

fn add_border_node(
    graph: &mut DagreGraph,
    prop: BorderType,
    prefix: &str,
    sg: &str,
    rank: i32,
) {
    let label = NodeLabel {
        width: 0.0,
        height: 0.0,
        rank: Some(rank),
        border_type: Some(prop),
        ..Default::default()
    };

    // prev = sgNode[prop][rank - 1]
    let prev: Option<String> = if rank >= 1 {
        let idx = (rank - 1) as usize;
        let arr = match prop {
            BorderType::BorderLeft => graph.node(sg).and_then(|n| n.border_left.as_ref()),
            BorderType::BorderRight => graph.node(sg).and_then(|n| n.border_right.as_ref()),
        };
        arr.and_then(|a| a.get(idx).cloned())
    } else {
        None
    };

    let curr: String = add_dummy_node(graph, DummyKind::Border, label, prefix);

    // sgNode[prop][rank] = curr  (the array is sparse / grows by index in JS)
    if let Some(node) = graph.node_mut(sg) {
        let arr = match prop {
            BorderType::BorderLeft => node.border_left.get_or_insert_with(Vec::new),
            BorderType::BorderRight => node.border_right.get_or_insert_with(Vec::new),
        };
        let idx = rank as usize;
        if idx >= arr.len() {
            arr.resize(idx + 1, String::new());
        }
        arr[idx] = curr.clone();
    }

    graph.set_parent(curr.clone(), sg);

    if let Some(prev) = prev {
        if !prev.is_empty() {
            graph.set_edge(
                prev,
                curr,
                EdgeLabel {
                    weight: Some(1.0),
                    ..Default::default()
                },
                None,
            );
        }
    }
}

#[cfg(test)]
mod tests;
