//! Port of `dagre/test/rank/util-test.ts`. Test names mirror the TS.

use super::*;
use crate::layered::graph::{Graph, GraphOptions};
use crate::layered::types::{EdgeLabel, NodeLabel};
use crate::layered::util::normalize_ranks;

fn new_graph() -> DagreGraph {
    let mut g: DagreGraph = Graph::new(GraphOptions::default());
    g.set_default_node_label(NodeLabel::default());
    g.set_default_edge_label(EdgeLabel {
        minlen: Some(1),
        ..Default::default()
    });
    g
}

fn rank_of(g: &DagreGraph, v: &str) -> i32 {
    g.node(v).and_then(|n| n.rank).unwrap()
}

#[test]
fn can_assign_a_rank_to_a_single_node_graph() {
    let mut g = new_graph();
    g.ensure_node("a");
    longest_path(&mut g);
    normalize_ranks(&mut g);
    assert_eq!(rank_of(&g, "a"), 0);
}

#[test]
fn can_assign_ranks_to_unconnected_nodes() {
    let mut g = new_graph();
    g.ensure_node("a");
    g.ensure_node("b");
    longest_path(&mut g);
    normalize_ranks(&mut g);
    assert_eq!(rank_of(&g, "a"), 0);
    assert_eq!(rank_of(&g, "b"), 0);
}

#[test]
fn can_assign_ranks_to_connected_nodes() {
    let mut g = new_graph();
    g.ensure_edge("a", "b", None);
    longest_path(&mut g);
    normalize_ranks(&mut g);
    assert_eq!(rank_of(&g, "a"), 0);
    assert_eq!(rank_of(&g, "b"), 1);
}

#[test]
fn can_assign_ranks_for_a_diamond() {
    let mut g = new_graph();
    g.ensure_path(&["a", "b", "d"]);
    g.ensure_path(&["a", "c", "d"]);
    longest_path(&mut g);
    normalize_ranks(&mut g);
    assert_eq!(rank_of(&g, "a"), 0);
    assert_eq!(rank_of(&g, "b"), 1);
    assert_eq!(rank_of(&g, "c"), 1);
    assert_eq!(rank_of(&g, "d"), 2);
}

#[test]
fn uses_the_minlen_attribute_on_the_edge() {
    let mut g = new_graph();
    g.ensure_path(&["a", "b", "d"]);
    g.ensure_edge("a", "c", None);
    g.set_edge(
        "c",
        "d",
        EdgeLabel {
            minlen: Some(2),
            ..Default::default()
        },
        None,
    );
    longest_path(&mut g);
    normalize_ranks(&mut g);
    assert_eq!(rank_of(&g, "a"), 0);
    // longest path biases towards the lowest rank it can assign
    assert_eq!(rank_of(&g, "b"), 2);
    assert_eq!(rank_of(&g, "c"), 1);
    assert_eq!(rank_of(&g, "d"), 3);
}
