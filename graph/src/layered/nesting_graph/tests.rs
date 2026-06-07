//! Port of `dagre/test/nesting-graph-test.ts`. Test names mirror the TS so a
//! reviewer can diff against the original oracle.

use std::collections::{HashSet, VecDeque};

use super::*;
use crate::layered::graph::{Graph, GraphOptions};
use crate::layered::types::{DummyKind, EdgeLabel, GraphLabel, NodeLabel};

/// `new Graph({compound: true}).setGraph({}).setDefaultNodeLabel(() => ({}))`.
fn new_graph() -> DagreGraph {
    let mut g: DagreGraph = Graph::new(GraphOptions {
        directed: true,
        multigraph: false,
        compound: true,
    });
    g.set_graph(GraphLabel::default());
    g.set_default_node_label(NodeLabel::default());
    g
}

/// Count connected components treating edges as undirected.
fn component_count(g: &DagreGraph) -> usize {
    let nodes = g.nodes();
    let mut seen: HashSet<String> = HashSet::new();
    let mut count = 0;
    for start in &nodes {
        if seen.contains(start) {
            continue;
        }
        count += 1;
        let mut queue: VecDeque<String> = VecDeque::new();
        queue.push_back(start.clone());
        seen.insert(start.clone());
        while let Some(v) = queue.pop_front() {
            let mut neighbors: Vec<String> = Vec::new();
            if let Some(p) = g.predecessors(&v) {
                neighbors.extend(p);
            }
            if let Some(s) = g.successors(&v) {
                neighbors.extend(s);
            }
            for n in neighbors {
                if seen.insert(n.clone()) {
                    queue.push_back(n);
                }
            }
        }
    }
    count
}

fn out_edge_minlen(g: &DagreGraph, v: &str, w: &str) -> i32 {
    g.edge(v, w, None).and_then(|l| l.minlen).unwrap()
}

// ── run ─────────────────────────────────────────────────────────────────

#[test]
fn connects_a_disconnected_graph() {
    let mut g = new_graph();
    g.ensure_node("a");
    g.ensure_node("b");
    assert_eq!(component_count(&g), 2);
    run(&mut g);
    assert_eq!(component_count(&g), 1);
    assert!(g.has_node("a"));
    assert!(g.has_node("b"));
}

#[test]
fn adds_border_nodes_to_top_and_bottom_of_a_subgraph() {
    let mut g = new_graph();
    g.set_parent("a", "sg1");
    run(&mut g);

    let border_top = g.node("sg1").unwrap().border_top.clone().unwrap();
    let border_bottom = g.node("sg1").unwrap().border_bottom.clone().unwrap();
    assert_eq!(g.parent(&border_top).as_deref(), Some("sg1"));
    assert_eq!(g.parent(&border_bottom).as_deref(), Some("sg1"));

    assert_eq!(g.out_edges(&border_top, Some("a")).unwrap().len(), 1);
    assert_eq!(out_edge_minlen(&g, &border_top, "a"), 1);
    assert_eq!(g.out_edges("a", Some(&border_bottom)).unwrap().len(), 1);
    assert_eq!(out_edge_minlen(&g, "a", &border_bottom), 1);

    let bt = g.node(&border_top).unwrap();
    assert_eq!(bt.width, 0.0);
    assert_eq!(bt.height, 0.0);
    assert_eq!(bt.dummy, Some(DummyKind::Border));
    let bb = g.node(&border_bottom).unwrap();
    assert_eq!(bb.width, 0.0);
    assert_eq!(bb.height, 0.0);
    assert_eq!(bb.dummy, Some(DummyKind::Border));
}

#[test]
fn adds_edges_between_borders_of_nested_subgraphs() {
    let mut g = new_graph();
    g.set_parent("sg2", "sg1");
    g.set_parent("a", "sg2");
    run(&mut g);

    let sg1_top = g.node("sg1").unwrap().border_top.clone().unwrap();
    let sg1_bottom = g.node("sg1").unwrap().border_bottom.clone().unwrap();
    let sg2_top = g.node("sg2").unwrap().border_top.clone().unwrap();
    let sg2_bottom = g.node("sg2").unwrap().border_bottom.clone().unwrap();

    assert_eq!(g.out_edges(&sg1_top, Some(&sg2_top)).unwrap().len(), 1);
    assert_eq!(out_edge_minlen(&g, &sg1_top, &sg2_top), 1);
    assert_eq!(g.out_edges(&sg2_bottom, Some(&sg1_bottom)).unwrap().len(), 1);
    assert_eq!(out_edge_minlen(&g, &sg2_bottom, &sg1_bottom), 1);
}

#[test]
fn adds_sufficient_weight_to_border_to_node_edges() {
    let mut g = new_graph();
    g.set_parent("x", "sg");
    g.set_edge(
        "a",
        "x",
        EdgeLabel {
            weight: Some(100.0),
            ..Default::default()
        },
        None,
    );
    g.set_edge(
        "x",
        "b",
        EdgeLabel {
            weight: Some(200.0),
            ..Default::default()
        },
        None,
    );
    run(&mut g);

    let top = g.node("sg").unwrap().border_top.clone().unwrap();
    let bot = g.node("sg").unwrap().border_bottom.clone().unwrap();
    assert!(g.edge(&top, "x", None).unwrap().weight.unwrap() > 300.0);
    assert!(g.edge("x", &bot, None).unwrap().weight.unwrap() > 300.0);
}

#[test]
fn adds_an_edge_from_the_root_to_the_tops_of_top_level_subgraphs() {
    let mut g = new_graph();
    g.set_parent("a", "sg1");
    run(&mut g);

    let root = g.graph().unwrap().nesting_root.clone().unwrap();
    let border_top = g.node("sg1").unwrap().border_top.clone().unwrap();
    assert_eq!(g.out_edges(&root, Some(&border_top)).unwrap().len(), 1);
    assert!(g.has_edge(&root, &border_top, None));
}

#[test]
fn adds_an_edge_from_root_to_each_node_with_correct_minlen_1() {
    let mut g = new_graph();
    g.ensure_node("a");
    run(&mut g);

    let root = g.graph().unwrap().nesting_root.clone().unwrap();
    assert_eq!(g.out_edges(&root, Some("a")).unwrap().len(), 1);
    let label = g.edge(&root, "a", None).unwrap();
    assert_eq!(label.weight, Some(0.0));
    assert_eq!(label.minlen, Some(1));
}

#[test]
fn adds_an_edge_from_root_to_each_node_with_correct_minlen_2() {
    let mut g = new_graph();
    g.set_parent("a", "sg1");
    run(&mut g);

    let root = g.graph().unwrap().nesting_root.clone().unwrap();
    assert_eq!(g.out_edges(&root, Some("a")).unwrap().len(), 1);
    let label = g.edge(&root, "a", None).unwrap();
    assert_eq!(label.weight, Some(0.0));
    assert_eq!(label.minlen, Some(3));
}

#[test]
fn adds_an_edge_from_root_to_each_node_with_correct_minlen_3() {
    let mut g = new_graph();
    g.set_parent("sg2", "sg1");
    g.set_parent("a", "sg2");
    run(&mut g);

    let root = g.graph().unwrap().nesting_root.clone().unwrap();
    assert_eq!(g.out_edges(&root, Some("a")).unwrap().len(), 1);
    let label = g.edge(&root, "a", None).unwrap();
    assert_eq!(label.weight, Some(0.0));
    assert_eq!(label.minlen, Some(5));
}

#[test]
fn does_not_add_an_edge_from_the_root_to_itself() {
    let mut g = new_graph();
    g.ensure_node("a");
    run(&mut g);

    let root = g.graph().unwrap().nesting_root.clone().unwrap();
    assert_eq!(g.out_edges(&root, Some(&root)).unwrap(), Vec::new());
}

#[test]
fn expands_inter_node_edges_to_separate_sg_border_and_nodes_1() {
    let mut g = new_graph();
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            minlen: Some(1),
            ..Default::default()
        },
        None,
    );
    run(&mut g);
    assert_eq!(g.edge("a", "b", None).unwrap().minlen, Some(1));
}

#[test]
fn expands_inter_node_edges_to_separate_sg_border_and_nodes_2() {
    let mut g = new_graph();
    g.set_parent("a", "sg1");
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            minlen: Some(1),
            ..Default::default()
        },
        None,
    );
    run(&mut g);
    assert_eq!(g.edge("a", "b", None).unwrap().minlen, Some(3));
}

#[test]
fn expands_inter_node_edges_to_separate_sg_border_and_nodes_3() {
    let mut g = new_graph();
    g.set_parent("sg2", "sg1");
    g.set_parent("a", "sg2");
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            minlen: Some(1),
            ..Default::default()
        },
        None,
    );
    run(&mut g);
    assert_eq!(g.edge("a", "b", None).unwrap().minlen, Some(5));
}

#[test]
fn sets_minlen_correctly_for_nested_sg_border_to_children() {
    let mut g = new_graph();
    g.set_parent("a", "sg1");
    g.set_parent("sg2", "sg1");
    g.set_parent("b", "sg2");
    run(&mut g);

    let root = g.graph().unwrap().nesting_root.clone().unwrap();
    let sg1_top = g.node("sg1").unwrap().border_top.clone().unwrap();
    let sg1_bot = g.node("sg1").unwrap().border_bottom.clone().unwrap();
    let sg2_top = g.node("sg2").unwrap().border_top.clone().unwrap();
    let sg2_bot = g.node("sg2").unwrap().border_bottom.clone().unwrap();

    assert_eq!(out_edge_minlen(&g, &root, &sg1_top), 3);
    assert_eq!(out_edge_minlen(&g, &sg1_top, &sg2_top), 1);
    assert_eq!(out_edge_minlen(&g, &sg1_top, "a"), 2);
    assert_eq!(out_edge_minlen(&g, "a", &sg1_bot), 2);
    assert_eq!(out_edge_minlen(&g, &sg2_top, "b"), 1);
    assert_eq!(out_edge_minlen(&g, "b", &sg2_bot), 1);
    assert_eq!(out_edge_minlen(&g, &sg2_bot, &sg1_bot), 1);
}

// ── cleanup ───────────────────────────────────────────────────────────────

#[test]
fn removes_nesting_graph_edges() {
    let mut g = new_graph();
    g.set_parent("a", "sg1");
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            minlen: Some(1),
            ..Default::default()
        },
        None,
    );
    run(&mut g);
    cleanup(&mut g);
    assert_eq!(g.successors("a").unwrap(), vec!["b".to_string()]);
}

#[test]
fn removes_the_root_node() {
    let mut g = new_graph();
    g.set_parent("a", "sg1");
    run(&mut g);
    cleanup(&mut g);
    // sg1 + sg1Top + sg1Bottom + "a"
    assert_eq!(g.node_count(), 4);
}
