//! Port of `dagre/test/rank/feasible-tree-test.ts`. Test names mirror the TS.

use super::*;
use crate::layered::types::{EdgeLabel, NodeLabel};

fn node(rank: i32) -> NodeLabel {
    NodeLabel {
        rank: Some(rank),
        ..Default::default()
    }
}

fn edge(minlen: i32) -> EdgeLabel {
    EdgeLabel {
        minlen: Some(minlen),
        ..Default::default()
    }
}

fn rank_of(g: &DagreGraph, v: &str) -> i32 {
    g.node(v).and_then(|n| n.rank).unwrap()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn creates_a_tree_for_a_trivial_input_graph() {
    let mut g: DagreGraph = Graph::new(GraphOptions::default());
    g.set_node("a", node(0));
    g.set_node("b", node(1));
    g.set_edge("a", "b", edge(1), None);

    let tree = feasible_tree(&mut g);
    assert_eq!(rank_of(&g, "b"), rank_of(&g, "a") + 1);
    assert_eq!(tree.neighbors("a").unwrap(), vec!["b".to_string()]);
}

#[test]
fn correctly_shortens_slack_by_pulling_a_node_up() {
    let mut g: DagreGraph = Graph::new(GraphOptions::default());
    g.set_node("a", node(0));
    g.set_node("b", node(1));
    g.set_node("c", node(2));
    g.set_node("d", node(2));
    g.set_path(&["a", "b", "c"], edge(1));
    g.set_edge("a", "d", edge(1), None);

    let tree = feasible_tree(&mut g);
    assert_eq!(rank_of(&g, "b"), rank_of(&g, "a") + 1);
    assert_eq!(rank_of(&g, "c"), rank_of(&g, "b") + 1);
    assert_eq!(rank_of(&g, "d"), rank_of(&g, "a") + 1);
    assert_eq!(sorted(tree.neighbors("a").unwrap()), vec!["b", "d"]);
    assert_eq!(sorted(tree.neighbors("b").unwrap()), vec!["a", "c"]);
    assert_eq!(tree.neighbors("c").unwrap(), vec!["b".to_string()]);
    assert_eq!(tree.neighbors("d").unwrap(), vec!["a".to_string()]);
}

#[test]
fn correctly_shortens_slack_by_pulling_a_node_down() {
    let mut g: DagreGraph = Graph::new(GraphOptions::default());
    g.set_node("a", node(2));
    g.set_node("b", node(0));
    g.set_node("c", node(2));
    g.set_edge("b", "a", edge(1), None);
    g.set_edge("b", "c", edge(1), None);

    let tree = feasible_tree(&mut g);
    assert_eq!(rank_of(&g, "a"), rank_of(&g, "b") + 1);
    assert_eq!(rank_of(&g, "c"), rank_of(&g, "b") + 1);
    assert_eq!(sorted(tree.neighbors("a").unwrap()), vec!["b"]);
    assert_eq!(sorted(tree.neighbors("b").unwrap()), vec!["a", "c"]);
    assert_eq!(sorted(tree.neighbors("c").unwrap()), vec!["b"]);
}
