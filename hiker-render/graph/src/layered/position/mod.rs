//! Coordinate assignment — a port of `dagre/lib/position/index.ts`.
//!
//! [`position`] assigns each node an `x` (via Brandes–Köpf, see [`bk`]) and a
//! `y` (accumulated per rank). It works on a non-compound copy of the graph
//! (subgraph nodes are excluded) but writes the resulting `x`/`y` back onto the
//! real nodes of the supplied graph.

pub mod bk;

#[cfg(test)]
mod tests;

use super::graph::{Graph, NodeId};
use super::types::{EdgeLabel, GraphLabel, NodeLabel, RankAlign};
use super::util;

/// `position(graph)` — assign `x`/`y` to every (leaf) node.
pub fn position(graph: &mut Graph<GraphLabel, NodeLabel, EdgeLabel>) {
    // Work on a non-compound copy: it omits subgraph (parent) nodes so they
    // are never positioned, and decouples layout reads from the writes we make
    // back onto the original graph.
    let non_compound = util::as_non_compound_graph(graph);

    let ys = position_y(&non_compound);
    for (v, y) in ys {
        if let Some(node) = graph.node_mut(&v) {
            node.y = Some(y);
        }
    }

    let xs = bk::position_x(&non_compound);
    for (v, x) in xs {
        if let Some(node) = graph.node_mut(&v) {
            node.x = Some(x);
        }
    }
}

/// `positionY(graph)` — y per rank: `prevY + maxHeightOfPrevRanks + thisRank`
/// contribution, honoring `rankalign` (top/bottom/center). Returns the y map so
/// the caller can write it onto the original graph's nodes.
fn position_y(graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>) -> Vec<(NodeId, f64)> {
    let layering = util::build_layer_matrix(graph);
    let graph_label = graph.graph();
    let rank_sep = graph_label.and_then(|g| g.ranksep).unwrap_or(0.0);
    let rank_align = graph_label.and_then(|g| g.rank_align);

    let mut out: Vec<(NodeId, f64)> = Vec::new();
    let mut prev_y = 0.0_f64;
    for layer in &layering {
        let max_height = layer.iter().fold(0.0_f64, |acc, v| {
            let height = graph.node(v).map(|n| n.height).unwrap_or(0.0);
            acc.max(height)
        });
        for v in layer {
            let height = graph.node(v).map(|n| n.height).unwrap_or(0.0);
            let y = match rank_align {
                Some(RankAlign::Top) => prev_y + height / 2.0,
                Some(RankAlign::Bottom) => prev_y + max_height - height / 2.0,
                _ => prev_y + max_height / 2.0,
            };
            out.push((v.clone(), y));
        }
        prev_y += max_height + rank_sep;
    }
    out
}
