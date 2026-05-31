//! Port of `dagre/test/add-border-segments-test.ts`. Test names mirror the TS
//! so a reviewer can diff against the original oracle.

use super::*;
use crate::layered::graph::{Graph, GraphOptions};
use crate::layered::types::{BorderType, DummyKind, NodeLabel};

/// `new Graph({compound: true})` — no graph label set.
fn new_graph() -> DagreGraph {
    Graph::new(GraphOptions {
        directed: true,
        multigraph: false,
        compound: true,
    })
}

fn border_node_is(n: &NodeLabel, bt: BorderType, rank: i32) {
    assert_eq!(n.dummy, Some(DummyKind::Border));
    assert_eq!(n.border_type, Some(bt));
    assert_eq!(n.rank, Some(rank));
    assert_eq!(n.width, 0.0);
    assert_eq!(n.height, 0.0);
}

#[test]
fn does_not_add_border_nodes_for_a_non_compound_graph() {
    let mut g: DagreGraph = Graph::directed();
    g.set_node(
        "a",
        NodeLabel {
            rank: Some(0),
            ..Default::default()
        },
    );
    add_border_segments(&mut g);
    assert_eq!(g.node_count(), 1);
    assert_eq!(g.node("a").unwrap().rank, Some(0));
}

#[test]
fn does_not_add_border_nodes_for_a_graph_with_no_clusters() {
    let mut g = new_graph();
    g.set_node(
        "a",
        NodeLabel {
            rank: Some(0),
            ..Default::default()
        },
    );
    add_border_segments(&mut g);
    assert_eq!(g.node_count(), 1);
    assert_eq!(g.node("a").unwrap().rank, Some(0));
}

#[test]
fn adds_a_border_for_a_single_rank_subgraph() {
    let mut g = new_graph();
    g.set_node(
        "sg",
        NodeLabel {
            min_rank: Some(1),
            max_rank: Some(1),
            ..Default::default()
        },
    );
    add_border_segments(&mut g);

    let bl = g.node("sg").unwrap().border_left.clone().unwrap()[1].clone();
    let br = g.node("sg").unwrap().border_right.clone().unwrap()[1].clone();
    border_node_is(g.node(&bl).unwrap(), BorderType::BorderLeft, 1);
    assert_eq!(g.parent(&bl).as_deref(), Some("sg"));
    border_node_is(g.node(&br).unwrap(), BorderType::BorderRight, 1);
    assert_eq!(g.parent(&br).as_deref(), Some("sg"));
}

#[test]
fn adds_a_border_for_a_multi_rank_subgraph() {
    let mut g = new_graph();
    g.set_node(
        "sg",
        NodeLabel {
            min_rank: Some(1),
            max_rank: Some(2),
            ..Default::default()
        },
    );
    add_border_segments(&mut g);

    let bl_left = g.node("sg").unwrap().border_left.clone().unwrap();
    let br_right = g.node("sg").unwrap().border_right.clone().unwrap();

    let bl2 = bl_left[1].clone();
    let br2 = br_right[1].clone();
    border_node_is(g.node(&bl2).unwrap(), BorderType::BorderLeft, 1);
    assert_eq!(g.parent(&bl2).as_deref(), Some("sg"));
    border_node_is(g.node(&br2).unwrap(), BorderType::BorderRight, 1);
    assert_eq!(g.parent(&br2).as_deref(), Some("sg"));

    let bl1 = bl_left[2].clone();
    let br1 = br_right[2].clone();
    border_node_is(g.node(&bl1).unwrap(), BorderType::BorderLeft, 2);
    assert_eq!(g.parent(&bl1).as_deref(), Some("sg"));
    border_node_is(g.node(&br1).unwrap(), BorderType::BorderRight, 2);
    assert_eq!(g.parent(&br1).as_deref(), Some("sg"));

    assert!(g.has_edge(&bl_left[1], &bl_left[2], None));
    assert!(g.has_edge(&br_right[1], &br_right[2], None));
}

#[test]
fn adds_borders_for_nested_subgraphs() {
    let mut g = new_graph();
    g.set_node(
        "sg1",
        NodeLabel {
            min_rank: Some(1),
            max_rank: Some(1),
            ..Default::default()
        },
    );
    g.set_node(
        "sg2",
        NodeLabel {
            min_rank: Some(1),
            max_rank: Some(1),
            ..Default::default()
        },
    );
    g.set_parent("sg2", "sg1");
    add_border_segments(&mut g);

    let bl1 = g.node("sg1").unwrap().border_left.clone().unwrap()[1].clone();
    let br1 = g.node("sg1").unwrap().border_right.clone().unwrap()[1].clone();
    border_node_is(g.node(&bl1).unwrap(), BorderType::BorderLeft, 1);
    assert_eq!(g.parent(&bl1).as_deref(), Some("sg1"));
    border_node_is(g.node(&br1).unwrap(), BorderType::BorderRight, 1);
    assert_eq!(g.parent(&br1).as_deref(), Some("sg1"));

    let bl2 = g.node("sg2").unwrap().border_left.clone().unwrap()[1].clone();
    let br2 = g.node("sg2").unwrap().border_right.clone().unwrap()[1].clone();
    border_node_is(g.node(&bl2).unwrap(), BorderType::BorderLeft, 1);
    assert_eq!(g.parent(&bl2).as_deref(), Some("sg2"));
    border_node_is(g.node(&br2).unwrap(), BorderType::BorderRight, 1);
    assert_eq!(g.parent(&br2).as_deref(), Some("sg2"));
}
