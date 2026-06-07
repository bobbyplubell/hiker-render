//! Port of `dagre/test/coordinate-system-test.ts`.

use super::{adjust, undo};
use crate::layered::graph::Graph;
use crate::layered::types::{GraphLabel, NodeLabel, RankDir};

fn new_graph() -> Graph<GraphLabel, NodeLabel, crate::layered::types::EdgeLabel> {
    Graph::directed()
}

fn graph_label(rankdir: RankDir) -> GraphLabel {
    GraphLabel {
        rankdir: Some(rankdir),
        ..Default::default()
    }
}

/// Node with the given dimensions, no coords (adjust suite).
fn node_dims(width: f64, height: f64) -> NodeLabel {
    NodeLabel {
        width,
        height,
        ..Default::default()
    }
}

/// Node with dimensions and coords (undo suite).
fn node_full(width: f64, height: f64, x: f64, y: f64) -> NodeLabel {
    NodeLabel {
        width,
        height,
        x: Some(x),
        y: Some(y),
        ..Default::default()
    }
}

// ── adjust ──────────────────────────────────────────────────────────────────

#[test]
fn adjust_does_nothing_to_node_dimensions_with_rankdir_tb() {
    let mut g = new_graph();
    g.set_node("a", node_dims(100.0, 200.0));
    g.set_graph(graph_label(RankDir::Tb));
    adjust(&mut g);
    assert_eq!(g.node("a"), Some(&node_dims(100.0, 200.0)));
}

#[test]
fn adjust_does_nothing_to_node_dimensions_with_rankdir_bt() {
    let mut g = new_graph();
    g.set_node("a", node_dims(100.0, 200.0));
    g.set_graph(graph_label(RankDir::Bt));
    adjust(&mut g);
    assert_eq!(g.node("a"), Some(&node_dims(100.0, 200.0)));
}

#[test]
fn adjust_swaps_width_and_height_for_nodes_with_rankdir_lr() {
    let mut g = new_graph();
    g.set_node("a", node_dims(100.0, 200.0));
    g.set_graph(graph_label(RankDir::Lr));
    adjust(&mut g);
    assert_eq!(g.node("a"), Some(&node_dims(200.0, 100.0)));
}

#[test]
fn adjust_swaps_width_and_height_for_nodes_with_rankdir_rl() {
    let mut g = new_graph();
    g.set_node("a", node_dims(100.0, 200.0));
    g.set_graph(graph_label(RankDir::Rl));
    adjust(&mut g);
    assert_eq!(g.node("a"), Some(&node_dims(200.0, 100.0)));
}

// ── undo ──────────────────────────────────────────────────────────────────

#[test]
fn undo_does_nothing_to_points_with_rankdir_tb() {
    let mut g = new_graph();
    g.set_node("a", node_full(100.0, 200.0, 20.0, 40.0));
    g.set_graph(graph_label(RankDir::Tb));
    undo(&mut g);
    assert_eq!(g.node("a"), Some(&node_full(100.0, 200.0, 20.0, 40.0)));
}

#[test]
fn undo_flips_the_y_coordinate_for_points_with_rankdir_bt() {
    let mut g = new_graph();
    g.set_node("a", node_full(100.0, 200.0, 20.0, 40.0));
    g.set_graph(graph_label(RankDir::Bt));
    undo(&mut g);
    // {x: 20, y: -40, width: 100, height: 200}
    assert_eq!(g.node("a"), Some(&node_full(100.0, 200.0, 20.0, -40.0)));
}

#[test]
fn undo_swaps_dimensions_and_coordinates_for_points_with_rankdir_lr() {
    let mut g = new_graph();
    g.set_node("a", node_full(100.0, 200.0, 20.0, 40.0));
    g.set_graph(graph_label(RankDir::Lr));
    undo(&mut g);
    // {x: 40, y: 20, width: 200, height: 100}
    assert_eq!(g.node("a"), Some(&node_full(200.0, 100.0, 40.0, 20.0)));
}

#[test]
fn undo_swaps_dims_and_coords_and_flips_x_for_points_with_rankdir_rl() {
    let mut g = new_graph();
    g.set_node("a", node_full(100.0, 200.0, 20.0, 40.0));
    g.set_graph(graph_label(RankDir::Rl));
    undo(&mut g);
    // {x: -40, y: 20, width: 200, height: 100}
    assert_eq!(g.node("a"), Some(&node_full(200.0, 100.0, -40.0, 20.0)));
}
