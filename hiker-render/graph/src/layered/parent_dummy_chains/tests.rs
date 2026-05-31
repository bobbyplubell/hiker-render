//! Port of `dagre/test/parent-dummy-chains-test.ts`. Test names mirror the TS
//! so a reviewer can diff against the original oracle.

use super::*;
use crate::layered::graph::{Edge, Graph, GraphOptions};
use crate::layered::types::{EdgeLabel, GraphLabel, NodeLabel};

/// `new Graph({compound: true}).setGraph({})`.
fn new_graph() -> DagreGraph {
    let mut g: DagreGraph = Graph::new(GraphOptions {
        directed: true,
        multigraph: false,
        compound: true,
    });
    g.set_graph(GraphLabel::default());
    g
}

fn edge_obj(v: &str, w: &str) -> Edge {
    Edge::new(v, w, None)
}

fn set_dummy_chains(g: &mut DagreGraph, chains: &[&str]) {
    g.graph_mut().unwrap().dummy_chains = Some(chains.iter().map(|s| s.to_string()).collect());
}

fn set_path(g: &mut DagreGraph, nodes: &[&str]) {
    g.set_path(nodes, EdgeLabel::default());
}

#[test]
fn does_not_set_a_parent_if_both_tail_and_head_have_no_parent() {
    let mut g = new_graph();
    g.ensure_node("a");
    g.ensure_node("b");
    g.set_node(
        "d1",
        NodeLabel {
            edge_obj: Some(edge_obj("a", "b")),
            ..Default::default()
        },
    );
    set_dummy_chains(&mut g, &["d1"]);
    set_path(&mut g, &["a", "d1", "b"]);

    parent_dummy_chains(&mut g);
    assert_eq!(g.parent("d1"), None);
}

#[test]
fn uses_the_tails_parent_for_the_first_node_if_it_is_not_the_root() {
    let mut g = new_graph();
    g.set_parent("a", "sg1");
    g.set_node(
        "sg1",
        NodeLabel {
            min_rank: Some(0),
            max_rank: Some(2),
            ..Default::default()
        },
    );
    g.set_node(
        "d1",
        NodeLabel {
            edge_obj: Some(edge_obj("a", "b")),
            rank: Some(2),
            ..Default::default()
        },
    );
    set_dummy_chains(&mut g, &["d1"]);
    set_path(&mut g, &["a", "d1", "b"]);

    parent_dummy_chains(&mut g);
    assert_eq!(g.parent("d1").as_deref(), Some("sg1"));
}

#[test]
fn uses_the_heads_parent_for_the_first_node_if_tails_is_root() {
    let mut g = new_graph();
    g.set_parent("b", "sg1");
    g.set_node(
        "sg1",
        NodeLabel {
            min_rank: Some(1),
            max_rank: Some(3),
            ..Default::default()
        },
    );
    g.set_node(
        "d1",
        NodeLabel {
            edge_obj: Some(edge_obj("a", "b")),
            rank: Some(1),
            ..Default::default()
        },
    );
    set_dummy_chains(&mut g, &["d1"]);
    set_path(&mut g, &["a", "d1", "b"]);

    parent_dummy_chains(&mut g);
    assert_eq!(g.parent("d1").as_deref(), Some("sg1"));
}

#[test]
fn handles_a_long_chain_starting_in_a_subgraph() {
    let mut g = new_graph();
    g.set_parent("a", "sg1");
    g.set_node(
        "sg1",
        NodeLabel {
            min_rank: Some(0),
            max_rank: Some(2),
            ..Default::default()
        },
    );
    g.set_node(
        "d1",
        NodeLabel {
            edge_obj: Some(edge_obj("a", "b")),
            rank: Some(2),
            ..Default::default()
        },
    );
    g.set_node(
        "d2",
        NodeLabel {
            rank: Some(3),
            ..Default::default()
        },
    );
    g.set_node(
        "d3",
        NodeLabel {
            rank: Some(4),
            ..Default::default()
        },
    );
    set_dummy_chains(&mut g, &["d1"]);
    set_path(&mut g, &["a", "d1", "d2", "d3", "b"]);

    parent_dummy_chains(&mut g);
    assert_eq!(g.parent("d1").as_deref(), Some("sg1"));
    assert_eq!(g.parent("d2"), None);
    assert_eq!(g.parent("d3"), None);
}

#[test]
fn handles_a_long_chain_ending_in_a_subgraph() {
    let mut g = new_graph();
    g.set_parent("b", "sg1");
    g.set_node(
        "sg1",
        NodeLabel {
            min_rank: Some(3),
            max_rank: Some(5),
            ..Default::default()
        },
    );
    g.set_node(
        "d1",
        NodeLabel {
            edge_obj: Some(edge_obj("a", "b")),
            rank: Some(1),
            ..Default::default()
        },
    );
    g.set_node(
        "d2",
        NodeLabel {
            rank: Some(2),
            ..Default::default()
        },
    );
    g.set_node(
        "d3",
        NodeLabel {
            rank: Some(3),
            ..Default::default()
        },
    );
    set_dummy_chains(&mut g, &["d1"]);
    set_path(&mut g, &["a", "d1", "d2", "d3", "b"]);

    parent_dummy_chains(&mut g);
    assert_eq!(g.parent("d1"), None);
    assert_eq!(g.parent("d2"), None);
    assert_eq!(g.parent("d3").as_deref(), Some("sg1"));
}

#[test]
fn handles_nested_subgraphs() {
    let mut g = new_graph();
    g.set_parent("a", "sg2");
    g.set_parent("sg2", "sg1");
    g.set_node(
        "sg1",
        NodeLabel {
            min_rank: Some(0),
            max_rank: Some(4),
            ..Default::default()
        },
    );
    g.set_node(
        "sg2",
        NodeLabel {
            min_rank: Some(1),
            max_rank: Some(3),
            ..Default::default()
        },
    );
    g.set_parent("b", "sg4");
    g.set_parent("sg4", "sg3");
    g.set_node(
        "sg3",
        NodeLabel {
            min_rank: Some(6),
            max_rank: Some(10),
            ..Default::default()
        },
    );
    g.set_node(
        "sg4",
        NodeLabel {
            min_rank: Some(7),
            max_rank: Some(9),
            ..Default::default()
        },
    );
    for i in 0..5 {
        g.set_node(
            format!("d{}", i + 1),
            NodeLabel {
                rank: Some(i + 3),
                ..Default::default()
            },
        );
    }
    g.node_mut("d1").unwrap().edge_obj = Some(edge_obj("a", "b"));
    set_dummy_chains(&mut g, &["d1"]);
    set_path(&mut g, &["a", "d1", "d2", "d3", "d4", "d5", "b"]);

    parent_dummy_chains(&mut g);
    assert_eq!(g.parent("d1").as_deref(), Some("sg2"));
    assert_eq!(g.parent("d2").as_deref(), Some("sg1"));
    assert_eq!(g.parent("d3"), None);
    assert_eq!(g.parent("d4").as_deref(), Some("sg3"));
    assert_eq!(g.parent("d5").as_deref(), Some("sg4"));
}

#[test]
fn handles_overlapping_rank_ranges() {
    let mut g = new_graph();
    g.set_parent("a", "sg1");
    g.set_node(
        "sg1",
        NodeLabel {
            min_rank: Some(0),
            max_rank: Some(3),
            ..Default::default()
        },
    );
    g.set_parent("b", "sg2");
    g.set_node(
        "sg2",
        NodeLabel {
            min_rank: Some(2),
            max_rank: Some(6),
            ..Default::default()
        },
    );
    g.set_node(
        "d1",
        NodeLabel {
            edge_obj: Some(edge_obj("a", "b")),
            rank: Some(2),
            ..Default::default()
        },
    );
    g.set_node(
        "d2",
        NodeLabel {
            rank: Some(3),
            ..Default::default()
        },
    );
    g.set_node(
        "d3",
        NodeLabel {
            rank: Some(4),
            ..Default::default()
        },
    );
    set_dummy_chains(&mut g, &["d1"]);
    set_path(&mut g, &["a", "d1", "d2", "d3", "b"]);

    parent_dummy_chains(&mut g);
    assert_eq!(g.parent("d1").as_deref(), Some("sg1"));
    assert_eq!(g.parent("d2").as_deref(), Some("sg1"));
    assert_eq!(g.parent("d3").as_deref(), Some("sg2"));
}

#[test]
fn handles_an_lca_that_is_not_the_root_of_the_graph_1() {
    let mut g = new_graph();
    g.set_parent("a", "sg1");
    g.set_parent("sg2", "sg1");
    g.set_node(
        "sg1",
        NodeLabel {
            min_rank: Some(0),
            max_rank: Some(6),
            ..Default::default()
        },
    );
    g.set_parent("b", "sg2");
    g.set_node(
        "sg2",
        NodeLabel {
            min_rank: Some(3),
            max_rank: Some(5),
            ..Default::default()
        },
    );
    g.set_node(
        "d1",
        NodeLabel {
            edge_obj: Some(edge_obj("a", "b")),
            rank: Some(2),
            ..Default::default()
        },
    );
    g.set_node(
        "d2",
        NodeLabel {
            rank: Some(3),
            ..Default::default()
        },
    );
    set_dummy_chains(&mut g, &["d1"]);
    set_path(&mut g, &["a", "d1", "d2", "b"]);

    parent_dummy_chains(&mut g);
    assert_eq!(g.parent("d1").as_deref(), Some("sg1"));
    assert_eq!(g.parent("d2").as_deref(), Some("sg2"));
}

#[test]
fn handles_an_lca_that_is_not_the_root_of_the_graph_2() {
    let mut g = new_graph();
    g.set_parent("a", "sg2");
    g.set_parent("sg2", "sg1");
    g.set_node(
        "sg1",
        NodeLabel {
            min_rank: Some(0),
            max_rank: Some(6),
            ..Default::default()
        },
    );
    g.set_parent("b", "sg1");
    g.set_node(
        "sg2",
        NodeLabel {
            min_rank: Some(1),
            max_rank: Some(3),
            ..Default::default()
        },
    );
    g.set_node(
        "d1",
        NodeLabel {
            edge_obj: Some(edge_obj("a", "b")),
            rank: Some(3),
            ..Default::default()
        },
    );
    g.set_node(
        "d2",
        NodeLabel {
            rank: Some(4),
            ..Default::default()
        },
    );
    set_dummy_chains(&mut g, &["d1"]);
    set_path(&mut g, &["a", "d1", "d2", "b"]);

    parent_dummy_chains(&mut g);
    assert_eq!(g.parent("d1").as_deref(), Some("sg2"));
    assert_eq!(g.parent("d2").as_deref(), Some("sg1"));
}
