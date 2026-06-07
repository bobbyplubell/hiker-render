//! Rankdir coordinate-system transforms — a port of
//! `dagre/lib/coordinate-system.ts`.
//!
//! dagre always lays out top-to-bottom internally. To support the four
//! `rankdir` values it transforms the graph into TB-space before layout and
//! back into the requested space afterwards:
//!
//! * [`adjust`] runs *before* layout: for `LR`/`RL` it swaps every width/height
//!   so the horizontal layouts can be computed as if vertical.
//! * [`undo`] runs *after* layout: for `BT`/`RL` it flips the `y` axis, and for
//!   `LR`/`RL` it swaps `x`/`y` and swaps width/height back.
//!
//! The transforms touch the graph label, every node label, every edge label,
//! and every point of each edge's `points` — mirroring the TS exactly.
//!
//! # Fidelity notes
//!
//! * TS lowercases `rankdir` before comparing (`'lr'`, `'rl'`, `'bt'`); here we
//!   match on the [`RankDir`] enum directly.
//! * `swapWidthHeight` swaps width/height on the **graph label and every node**,
//!   and on **every edge label** too (TS `graph.edges().forEach(...)`).
//! * `reverseY` negates `y` on every node, on every edge label that *has* a `y`
//!   (TS `Object.hasOwn(edgeLabel, 'y')` → `Option::is_some`), and on every
//!   point of each edge's `points`.
//! * `swapXY` swaps `x`/`y` on every node, on every edge label that has an `x`
//!   (TS `Object.hasOwn(edgeLabel, 'x')`), and on every edge point.

use super::graph::Graph;
use super::types::{GraphLabel, EdgeLabel, NodeLabel, Point, RankDir};

type DagreGraph = Graph<GraphLabel, NodeLabel, EdgeLabel>;

/// `adjust(graph)` — pre-layout transform: swap width/height for `LR`/`RL`.
pub fn adjust(graph: &mut DagreGraph) {
    let rank_dir = graph.graph().and_then(|g| g.rankdir);
    if matches!(rank_dir, Some(RankDir::Lr) | Some(RankDir::Rl)) {
        swap_width_height(graph);
    }
}

/// `undo(graph)` — post-layout transform back into the requested rankdir space.
pub fn undo(graph: &mut DagreGraph) {
    let rank_dir = graph.graph().and_then(|g| g.rankdir);

    if matches!(rank_dir, Some(RankDir::Bt) | Some(RankDir::Rl)) {
        reverse_y(graph);
    }

    if matches!(rank_dir, Some(RankDir::Lr) | Some(RankDir::Rl)) {
        swap_xy(graph);
        swap_width_height(graph);
    }
}

/// Swap width/height on the graph label, every node label, and every edge label.
fn swap_width_height(graph: &mut DagreGraph) {
    if let Some(g) = graph.graph_mut() {
        let w = g.width;
        g.width = g.height;
        g.height = w;
    }

    for v in graph.nodes() {
        if let Some(node) = graph.node_mut(&v) {
            let w = node.width;
            node.width = node.height;
            node.height = w;
        }
    }

    for e in graph.edges() {
        if let Some(edge) = graph.edge_mut(&e.v, &e.w, e.name.as_deref()) {
            let w = edge.width;
            edge.width = edge.height;
            edge.height = w;
        }
    }
}

/// Negate `y` on every node, every edge label that has a `y`, and every point
/// of each edge's `points`.
fn reverse_y(graph: &mut DagreGraph) {
    for v in graph.nodes() {
        if let Some(node) = graph.node_mut(&v) {
            reverse_y_node(node);
        }
    }

    for e in graph.edges() {
        if let Some(edge) = graph.edge_mut(&e.v, &e.w, e.name.as_deref()) {
            if let Some(points) = edge.points.as_mut() {
                for p in points.iter_mut() {
                    reverse_y_point(p);
                }
            }
            if edge.y.is_some() {
                reverse_y_edge(edge);
            }
        }
    }
}

fn reverse_y_node(node: &mut NodeLabel) {
    node.y = node.y.map(|y| -y);
}

fn reverse_y_edge(edge: &mut EdgeLabel) {
    edge.y = edge.y.map(|y| -y);
}

fn reverse_y_point(p: &mut Point) {
    p.y = -p.y;
}

/// Swap `x`/`y` on every node, every edge label that has an `x`, and every
/// point of each edge's `points`.
fn swap_xy(graph: &mut DagreGraph) {
    for v in graph.nodes() {
        if let Some(node) = graph.node_mut(&v) {
            swap_xy_node(node);
        }
    }

    for e in graph.edges() {
        if let Some(edge) = graph.edge_mut(&e.v, &e.w, e.name.as_deref()) {
            if let Some(points) = edge.points.as_mut() {
                for p in points.iter_mut() {
                    swap_xy_point(p);
                }
            }
            if edge.x.is_some() {
                swap_xy_edge(edge);
            }
        }
    }
}

fn swap_xy_node(node: &mut NodeLabel) {
    let x = node.x;
    node.x = node.y;
    node.y = x;
}

fn swap_xy_edge(edge: &mut EdgeLabel) {
    let x = edge.x;
    edge.x = edge.y;
    edge.y = x;
}

fn swap_xy_point(p: &mut Point) {
    let x = p.x;
    p.x = p.y;
    p.y = x;
}

#[cfg(test)]
mod tests;
