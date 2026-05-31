//! Port of `dagre/test/normalize-test.ts`. Test names mirror the TS so a
//! reviewer can diff against the original oracle. Covers run (short edges,
//! splitting long edges, dummy dimensions, label rank, weight preservation) and
//! undo (round-trip, label restoration, points collection/merging, label coords,
//! multi-edges).

use super::*;
use crate::layered::graph::{Edge, GraphOptions};
use crate::layered::types::{DummyKind, EdgeLabel, GraphLabel, NodeLabel, Point};

/// `new Graph({multigraph: true, compound: true}).setGraph({})`.
fn new_graph() -> DagreGraph {
    let mut g: DagreGraph = Graph::new(GraphOptions {
        directed: true,
        multigraph: true,
        compound: true,
    });
    g.set_graph(GraphLabel::default());
    g
}

fn node_rank(g: &DagreGraph, v: &str) -> i32 {
    g.node(v).and_then(|n| n.rank).unwrap()
}

fn set_rank_node(g: &mut DagreGraph, v: &str, rank: i32) {
    g.set_node(
        v,
        NodeLabel {
            rank: Some(rank),
            ..Default::default()
        },
    );
}

fn incident_nodes(e: &Edge) -> (String, String) {
    (e.v.clone(), e.w.clone())
}

// ── run ─────────────────────────────────────────────────────────────────

#[test]
fn does_not_change_a_short_edge() {
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 1);
    g.set_edge("a", "b", EdgeLabel::default(), None);

    run(&mut g);

    let incident: Vec<(String, String)> = g.edges().iter().map(incident_nodes).collect();
    assert_eq!(incident, vec![("a".to_string(), "b".to_string())]);
    assert_eq!(node_rank(&g, "a"), 0);
    assert_eq!(node_rank(&g, "b"), 1);
}

#[test]
fn splits_a_two_layer_edge_into_two_segments() {
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 2);
    g.set_edge("a", "b", EdgeLabel::default(), None);

    run(&mut g);

    let succs = g.successors("a").unwrap();
    assert_eq!(succs.len(), 1);
    let successor = succs[0].clone();
    assert_eq!(g.node(&successor).unwrap().dummy, Some(DummyKind::Edge));
    assert_eq!(g.node(&successor).unwrap().rank, Some(1));
    assert_eq!(g.successors(&successor).unwrap(), vec!["b".to_string()]);
    assert_eq!(node_rank(&g, "a"), 0);
    assert_eq!(node_rank(&g, "b"), 2);

    let chains = g.graph().unwrap().dummy_chains.clone().unwrap();
    assert_eq!(chains.len(), 1);
    assert_eq!(chains[0], successor);
}

#[test]
fn assigns_width_0_height_0_to_dummy_nodes_by_default() {
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 2);
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            width: Some(10.0),
            height: Some(10.0),
            ..Default::default()
        },
        None,
    );

    run(&mut g);

    let succs = g.successors("a").unwrap();
    assert_eq!(succs.len(), 1);
    let successor = &succs[0];
    assert_eq!(g.node(successor).unwrap().width, 0.0);
    assert_eq!(g.node(successor).unwrap().height, 0.0);
}

#[test]
fn assigns_width_and_height_from_the_edge_for_the_node_on_label_rank() {
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 4);
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            width: Some(20.0),
            height: Some(10.0),
            label_rank: Some(2),
            ..Default::default()
        },
        None,
    );

    run(&mut g);

    let s1 = g.successors("a").unwrap()[0].clone();
    let label_v = g.successors(&s1).unwrap()[0].clone();
    let label_node = g.node(&label_v).unwrap();
    assert_eq!(label_node.width, 20.0);
    assert_eq!(label_node.height, 10.0);
}

#[test]
fn preserves_the_weight_for_the_edge() {
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 2);
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            weight: Some(2.0),
            ..Default::default()
        },
        None,
    );

    run(&mut g);

    let succs = g.successors("a").unwrap();
    assert_eq!(succs.len(), 1);
    assert_eq!(g.edge("a", &succs[0], None).unwrap().weight, Some(2.0));
}

// ── undo ────────────────────────────────────────────────────────────────

#[test]
fn reverses_the_run_operation() {
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 2);
    g.set_edge("a", "b", EdgeLabel::default(), None);

    run(&mut g);
    undo(&mut g);

    let incident: Vec<(String, String)> = g.edges().iter().map(incident_nodes).collect();
    assert_eq!(incident, vec![("a".to_string(), "b".to_string())]);
    assert_eq!(node_rank(&g, "a"), 0);
    assert_eq!(node_rank(&g, "b"), 2);
}

#[test]
fn restores_previous_edge_labels() {
    // TS uses {foo: "bar"}; our EdgeLabel has no free-form field, so we use a
    // distinctive numeric field (label_offset) as the marker carried through.
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 2);
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            label_offset: Some(42.0),
            ..Default::default()
        },
        None,
    );

    run(&mut g);
    undo(&mut g);

    assert_eq!(g.edge("a", "b", None).unwrap().label_offset, Some(42.0));
}

#[test]
fn collects_assigned_coordinates_into_the_points_attribute() {
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 2);
    g.set_edge("a", "b", EdgeLabel::default(), None);

    run(&mut g);

    let dummy = g.neighbors("a").unwrap()[0].clone();
    let dl = g.node_mut(&dummy).unwrap();
    dl.x = Some(5.0);
    dl.y = Some(10.0);

    undo(&mut g);

    assert_eq!(
        g.edge("a", "b", None).unwrap().points,
        Some(vec![Point::new(5.0, 10.0)])
    );
}

#[test]
fn merges_assigned_coordinates_into_the_points_attribute() {
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 4);
    g.set_edge("a", "b", EdgeLabel::default(), None);

    run(&mut g);

    let a_suc = g.neighbors("a").unwrap()[0].clone();
    {
        let n = g.node_mut(&a_suc).unwrap();
        n.x = Some(5.0);
        n.y = Some(10.0);
    }

    let mid = g.successors(&g.successors("a").unwrap()[0]).unwrap()[0].clone();
    {
        let n = g.node_mut(&mid).unwrap();
        n.x = Some(20.0);
        n.y = Some(25.0);
    }

    let b_pred = g.neighbors("b").unwrap()[0].clone();
    {
        let n = g.node_mut(&b_pred).unwrap();
        n.x = Some(100.0);
        n.y = Some(200.0);
    }

    undo(&mut g);

    assert_eq!(
        g.edge("a", "b", None).unwrap().points,
        Some(vec![
            Point::new(5.0, 10.0),
            Point::new(20.0, 25.0),
            Point::new(100.0, 200.0),
        ])
    );
}

#[test]
fn sets_coords_and_dims_for_the_label_if_the_edge_has_one() {
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 2);
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            width: Some(10.0),
            height: Some(20.0),
            label_rank: Some(1),
            ..Default::default()
        },
        None,
    );

    run(&mut g);

    let label_v = g.successors("a").unwrap()[0].clone();
    {
        let n = g.node_mut(&label_v).unwrap();
        n.x = Some(50.0);
        n.y = Some(60.0);
        n.width = 20.0;
        n.height = 10.0;
    }

    undo(&mut g);

    let e = g.edge("a", "b", None).unwrap();
    assert_eq!(e.x, Some(50.0));
    assert_eq!(e.y, Some(60.0));
    assert_eq!(e.width, Some(20.0));
    assert_eq!(e.height, Some(10.0));
}

#[test]
fn sets_coords_and_dims_for_the_label_if_the_long_edge_has_one() {
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 4);
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            width: Some(10.0),
            height: Some(20.0),
            label_rank: Some(2),
            ..Default::default()
        },
        None,
    );

    run(&mut g);

    let label_v = g.successors(&g.successors("a").unwrap()[0]).unwrap()[0].clone();
    {
        let n = g.node_mut(&label_v).unwrap();
        n.x = Some(50.0);
        n.y = Some(60.0);
        n.width = 20.0;
        n.height = 10.0;
    }

    undo(&mut g);

    let e = g.edge("a", "b", None).unwrap();
    assert_eq!(e.x, Some(50.0));
    assert_eq!(e.y, Some(60.0));
    assert_eq!(e.width, Some(20.0));
    assert_eq!(e.height, Some(10.0));
}

#[test]
fn restores_multi_edges() {
    let mut g = new_graph();
    set_rank_node(&mut g, "a", 0);
    set_rank_node(&mut g, "b", 2);
    g.set_edge("a", "b", EdgeLabel::default(), Some("bar"));
    g.set_edge("a", "b", EdgeLabel::default(), Some("foo"));

    run(&mut g);

    let mut out_edges = g.out_edges("a", None).unwrap();
    out_edges.sort_by(|a, b| a.name.as_deref().cmp(&b.name.as_deref()));
    assert_eq!(out_edges.len(), 2);

    // bar dummy
    let bar_w = out_edges[0].w.clone();
    {
        let n = g.node_mut(&bar_w).unwrap();
        n.x = Some(5.0);
        n.y = Some(10.0);
    }
    // foo dummy
    let foo_w = out_edges[1].w.clone();
    {
        let n = g.node_mut(&foo_w).unwrap();
        n.x = Some(15.0);
        n.y = Some(20.0);
    }

    undo(&mut g);

    assert!(!g.has_edge("a", "b", None));
    assert_eq!(
        g.edge("a", "b", Some("bar")).unwrap().points,
        Some(vec![Point::new(5.0, 10.0)])
    );
    assert_eq!(
        g.edge("a", "b", Some("foo")).unwrap().points,
        Some(vec![Point::new(15.0, 20.0)])
    );
}
