//! Port of `dagre/test/util-test.ts`. Test names/structure mirror the TS so a
//! reviewer can diff against the original oracle.

use std::collections::HashMap;

use super::*;
use crate::layered::graph::{Graph, GraphOptions, NodeId};
use crate::layered::types::{DummyKind, EdgeLabel, GraphLabel, NodeLabel, Point};

type G = Graph<GraphLabel, NodeLabel, EdgeLabel>;

fn multigraph() -> G {
    Graph::new(GraphOptions {
        directed: true,
        multigraph: true,
        compound: false,
    })
}

fn compound_multigraph() -> G {
    Graph::new(GraphOptions {
        directed: true,
        multigraph: true,
        compound: true,
    })
}

fn elabel(weight: f64, minlen: i32) -> EdgeLabel {
    EdgeLabel {
        weight: Some(weight),
        minlen: Some(minlen),
        ..Default::default()
    }
}

// ── simplify ────────────────────────────────────────────────────────────────

#[test]
fn simplify_copies_without_change_a_graph_with_no_multi_edges() {
    let mut g = multigraph();
    g.set_edge("a", "b", elabel(1.0, 1), None);
    let g2 = simplify(&g);
    let e = g2.edge("a", "b", None).unwrap();
    assert_eq!(e.weight, Some(1.0));
    assert_eq!(e.minlen, Some(1));
    assert_eq!(g2.edge_count(), 1);
}

#[test]
fn simplify_collapses_multi_edges() {
    let mut g = multigraph();
    g.set_edge("a", "b", elabel(1.0, 1), None);
    g.set_edge("a", "b", elabel(2.0, 2), Some("multi"));
    let g2 = simplify(&g);
    assert!(!g2.is_multigraph());
    let e = g2.edge("a", "b", None).unwrap();
    assert_eq!(e.weight, Some(3.0));
    assert_eq!(e.minlen, Some(2));
    assert_eq!(g2.edge_count(), 1);
}

#[test]
fn simplify_copies_the_graph_object() {
    let mut g = multigraph();
    g.set_graph(GraphLabel {
        nesting_root: Some("bar".to_string()),
        ..Default::default()
    });
    let g2 = simplify(&g);
    assert_eq!(g2.graph().unwrap().nesting_root.as_deref(), Some("bar"));
}

// ── asNonCompoundGraph ───────────────────────────────────────────────────────

#[test]
fn as_non_compound_graph_copies_all_nodes() {
    let mut g = compound_multigraph();
    g.set_node(
        "a",
        NodeLabel {
            class: Some("bar".to_string()),
            ..Default::default()
        },
    );
    g.set_node_none("b");
    let g2 = as_non_compound_graph(&g);
    assert_eq!(g2.node("a").unwrap().class.as_deref(), Some("bar"));
    assert!(g2.has_node("b"));
}

#[test]
fn as_non_compound_graph_copies_all_edges() {
    let mut g = compound_multigraph();
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            forward_name: Some("bar".to_string()),
            ..Default::default()
        },
        None,
    );
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            forward_name: Some("baz".to_string()),
            ..Default::default()
        },
        Some("multi"),
    );
    let g2 = as_non_compound_graph(&g);
    assert_eq!(
        g2.edge("a", "b", None).unwrap().forward_name.as_deref(),
        Some("bar")
    );
    assert_eq!(
        g2.edge("a", "b", Some("multi"))
            .unwrap()
            .forward_name
            .as_deref(),
        Some("baz")
    );
}

#[test]
fn as_non_compound_graph_does_not_copy_compound_nodes() {
    let mut g = compound_multigraph();
    g.set_parent("a", "sg1");
    let g2 = as_non_compound_graph(&g);
    assert_eq!(g2.parent("sg1"), None);
    assert_eq!(g2.parent("a"), None);
    assert!(!g2.is_compound());
}

#[test]
fn as_non_compound_graph_copies_the_graph_object() {
    let mut g = compound_multigraph();
    g.set_graph(GraphLabel {
        nesting_root: Some("bar".to_string()),
        ..Default::default()
    });
    let g2 = as_non_compound_graph(&g);
    assert_eq!(g2.graph().unwrap().nesting_root.as_deref(), Some("bar"));
}

// ── successorWeights ─────────────────────────────────────────────────────────

#[test]
fn successor_weights_maps_a_node_to_its_successors_with_associated_weights() {
    let mut g = multigraph();
    g.set_edge("a", "b", elabel_w(2.0), None);
    g.set_edge("b", "c", elabel_w(1.0), None);
    g.set_edge("b", "c", elabel_w(2.0), Some("multi"));
    g.set_edge("b", "d", elabel_w(1.0), Some("multi"));
    let sw = successor_weights(&g);
    assert_eq!(sw["a"], hm(&[("b", 2.0)]));
    assert_eq!(sw["b"], hm(&[("c", 3.0), ("d", 1.0)]));
    assert_eq!(sw["c"], hm(&[]));
    assert_eq!(sw["d"], hm(&[]));
}

// ── predecessorWeights ───────────────────────────────────────────────────────

#[test]
fn predecessor_weights_maps_a_node_to_its_predecessors_with_associated_weights() {
    let mut g = multigraph();
    g.set_edge("a", "b", elabel_w(2.0), None);
    g.set_edge("b", "c", elabel_w(1.0), None);
    g.set_edge("b", "c", elabel_w(2.0), Some("multi"));
    g.set_edge("b", "d", elabel_w(1.0), Some("multi"));
    let pw = predecessor_weights(&g);
    assert_eq!(pw["a"], hm(&[]));
    assert_eq!(pw["b"], hm(&[("a", 2.0)]));
    assert_eq!(pw["c"], hm(&[("b", 3.0)]));
    assert_eq!(pw["d"], hm(&[("b", 1.0)]));
}

fn elabel_w(weight: f64) -> EdgeLabel {
    EdgeLabel {
        weight: Some(weight),
        ..Default::default()
    }
}

fn hm(pairs: &[(&str, f64)]) -> HashMap<NodeId, f64> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

// ── intersectRect ────────────────────────────────────────────────────────────

fn expect_intersects(rect: &Rect, point: &Point) {
    let cross = intersect_rect(rect, point);
    if cross.x != point.x {
        let m = (cross.y - point.y) / (cross.x - point.x);
        let lhs = cross.y - rect.y;
        let rhs = m * (cross.x - rect.x);
        assert!(
            (lhs - rhs).abs() < 1e-9,
            "slope check failed: {lhs} vs {rhs}"
        );
    }
}

fn expect_touches_border(rect: &Rect, point: &Point) {
    let cross = intersect_rect(rect, point);
    if (rect.x - cross.x).abs() != rect.width / 2.0 {
        assert_eq!((rect.y - cross.y).abs(), rect.height / 2.0);
    }
}

#[test]
fn intersect_rect_creates_a_slope_that_will_intersect_the_rectangles_center() {
    let rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };
    expect_intersects(&rect, &Point::new(2.0, 6.0));
    expect_intersects(&rect, &Point::new(2.0, -6.0));
    expect_intersects(&rect, &Point::new(6.0, 2.0));
    expect_intersects(&rect, &Point::new(-6.0, 2.0));
    expect_intersects(&rect, &Point::new(5.0, 0.0));
    expect_intersects(&rect, &Point::new(0.0, 5.0));
}

#[test]
fn intersect_rect_touches_the_border_of_the_rectangle() {
    let rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };
    expect_touches_border(&rect, &Point::new(2.0, 6.0));
    expect_touches_border(&rect, &Point::new(2.0, -6.0));
    expect_touches_border(&rect, &Point::new(6.0, 2.0));
    expect_touches_border(&rect, &Point::new(-6.0, 2.0));
    expect_touches_border(&rect, &Point::new(5.0, 0.0));
    expect_touches_border(&rect, &Point::new(0.0, 5.0));
}

#[test]
#[should_panic]
fn intersect_rect_throws_an_error_if_the_point_is_at_the_center_of_the_rectangle() {
    let rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };
    intersect_rect(&rect, &Point::new(0.0, 0.0));
}

// ── buildLayerMatrix ─────────────────────────────────────────────────────────

#[test]
fn build_layer_matrix_creates_a_matrix_based_on_rank_and_order_of_nodes() {
    let mut g: G = Graph::new(GraphOptions::default());
    g.set_node("a", node_ro(0, 0));
    g.set_node("b", node_ro(0, 1));
    g.set_node("c", node_ro(1, 0));
    g.set_node("d", node_ro(1, 1));
    g.set_node("e", node_ro(2, 0));

    let m = build_layer_matrix(&g);
    assert_eq!(
        m,
        vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string(), "d".to_string()],
            vec!["e".to_string()],
        ]
    );
}

fn node_ro(rank: i32, order: usize) -> NodeLabel {
    NodeLabel {
        rank: Some(rank),
        order: Some(order),
        ..Default::default()
    }
}

// ── time ─────────────────────────────────────────────────────────────────────

#[test]
fn time_returns_the_value_from_the_evaluated_function() {
    assert_eq!(time("foo", || "bar"), "bar");
}

#[test]
fn notime_returns_the_value_from_the_evaluated_function() {
    assert_eq!(notime("foo", || "bar"), "bar");
}

// ── normalizeRanks ───────────────────────────────────────────────────────────

#[test]
fn normalize_ranks_adjust_ranks_such_that_all_are_ge_0_and_at_least_one_is_0() {
    let mut g: G = Graph::new(GraphOptions::default());
    g.set_node("a", node_r(3));
    g.set_node("b", node_r(2));
    g.set_node("c", node_r(4));

    normalize_ranks(&mut g);

    assert_eq!(g.node("a").unwrap().rank, Some(1));
    assert_eq!(g.node("b").unwrap().rank, Some(0));
    assert_eq!(g.node("c").unwrap().rank, Some(2));
}

#[test]
fn normalize_ranks_works_for_negative_ranks() {
    let mut g: G = Graph::new(GraphOptions::default());
    g.set_node("a", node_r(-3));
    g.set_node("b", node_r(-2));

    normalize_ranks(&mut g);

    assert_eq!(g.node("a").unwrap().rank, Some(0));
    assert_eq!(g.node("b").unwrap().rank, Some(1));
}

#[test]
fn normalize_ranks_does_not_assign_a_rank_to_subgraphs() {
    let mut g: G = Graph::new(GraphOptions {
        directed: true,
        multigraph: false,
        compound: true,
    });
    g.set_node("a", node_r(0));
    g.set_node("sg", NodeLabel::default());
    g.set_parent("a", "sg");

    normalize_ranks(&mut g);

    assert_eq!(g.node("sg").unwrap().rank, None);
    assert_eq!(g.node("a").unwrap().rank, Some(0));
}

fn node_r(rank: i32) -> NodeLabel {
    NodeLabel {
        rank: Some(rank),
        ..Default::default()
    }
}

// ── removeEmptyRanks ─────────────────────────────────────────────────────────

#[test]
fn remove_empty_ranks_removes_border_ranks_without_any_nodes() {
    let mut g: G = Graph::new(GraphOptions::default());
    g.set_graph(GraphLabel {
        node_rank_factor: Some(4),
        ..Default::default()
    });
    g.set_node("a", node_r(0));
    g.set_node("b", node_r(4));
    remove_empty_ranks(&mut g);
    assert_eq!(g.node("a").unwrap().rank, Some(0));
    assert_eq!(g.node("b").unwrap().rank, Some(1));
}

#[test]
fn remove_empty_ranks_does_not_remove_non_border_ranks() {
    let mut g: G = Graph::new(GraphOptions::default());
    g.set_graph(GraphLabel {
        node_rank_factor: Some(4),
        ..Default::default()
    });
    g.set_node("a", node_r(0));
    g.set_node("b", node_r(8));
    remove_empty_ranks(&mut g);
    assert_eq!(g.node("a").unwrap().rank, Some(0));
    assert_eq!(g.node("b").unwrap().rank, Some(2));
}

#[test]
fn remove_empty_ranks_handles_parents_with_undefined_ranks() {
    let mut g: G = Graph::new(GraphOptions {
        directed: true,
        multigraph: false,
        compound: true,
    });
    g.set_graph(GraphLabel {
        node_rank_factor: Some(3),
        ..Default::default()
    });
    g.set_node("a", node_r(0));
    g.set_node("b", node_r(6));
    g.set_node("sg", NodeLabel::default());
    g.set_parent("a", "sg");
    remove_empty_ranks(&mut g);
    assert_eq!(g.node("a").unwrap().rank, Some(0));
    assert_eq!(g.node("b").unwrap().rank, Some(2));
    assert_eq!(g.node("sg").unwrap().rank, None);
}

// ── range ────────────────────────────────────────────────────────────────────

#[test]
fn range_builds_an_array_to_the_limit() {
    let r = range(4);
    assert_eq!(r.len(), 4);
    assert_eq!(r.iter().sum::<i32>(), 6);
}

#[test]
fn range_builds_an_array_with_a_start() {
    let r = range_from(2, 4);
    assert_eq!(r.len(), 2);
    assert_eq!(r.iter().sum::<i32>(), 5);
}

#[test]
fn range_builds_an_array_with_a_negative_step() {
    let r = range_step(5, -1, -1);
    assert_eq!(r[0], 5);
    assert_eq!(r[5], 0);
}

// ── mapValues ────────────────────────────────────────────────────────────────

#[test]
fn map_values_creates_an_object_with_the_same_keys() {
    let mut users: HashMap<String, (String, i32)> = HashMap::new();
    users.insert("fred".to_string(), ("fred".to_string(), 40));
    users.insert("pebbles".to_string(), ("pebbles".to_string(), 1));

    let ages = map_values(&users, |user, _k| user.1);
    assert_eq!(ages["fred"], 40);
    assert_eq!(ages["pebbles"], 1);
}

// ── addDummyNode / addBorderNode / uniqueId (extra coverage) ─────────────────

#[test]
fn add_dummy_node_uses_the_requested_name_when_free() {
    let mut g: G = Graph::new(GraphOptions::default());
    let v = add_dummy_node(&mut g, DummyKind::Edge, NodeLabel::default(), "_d");
    assert_eq!(v, "_d");
    assert_eq!(g.node("_d").unwrap().dummy, Some(DummyKind::Edge));
}

#[test]
fn add_dummy_node_generates_a_unique_id_on_collision() {
    let mut g: G = Graph::new(GraphOptions::default());
    let v1 = add_dummy_node(&mut g, DummyKind::Edge, NodeLabel::default(), "_d");
    let v2 = add_dummy_node(&mut g, DummyKind::Edge, NodeLabel::default(), "_d");
    assert_eq!(v1, "_d");
    assert_ne!(v2, "_d");
    assert!(v2.starts_with("_d"));
}

#[test]
fn add_border_node_sets_rank_and_order_when_both_given() {
    let mut g: G = Graph::new(GraphOptions::default());
    let v = add_border_node(&mut g, "_bt", Some(2), Some(1));
    let n = g.node(&v).unwrap();
    assert_eq!(n.dummy, Some(DummyKind::Border));
    assert_eq!(n.width, 0.0);
    assert_eq!(n.height, 0.0);
    assert_eq!(n.rank, Some(2));
    assert_eq!(n.order, Some(1));
}

#[test]
fn add_border_node_omits_rank_and_order_when_not_given() {
    let mut g: G = Graph::new(GraphOptions::default());
    let v = add_border_node(&mut g, "_bt", None, None);
    let n = g.node(&v).unwrap();
    assert_eq!(n.rank, None);
    assert_eq!(n.order, None);
}

// ── maxRank / partition / pick ───────────────────────────────────────────────

#[test]
fn max_rank_returns_the_largest_rank() {
    let mut g: G = Graph::new(GraphOptions::default());
    g.set_node("a", node_r(0));
    g.set_node("b", node_r(2));
    g.set_node("c", node_r(1));
    assert_eq!(max_rank(&g), 2);
}

#[test]
fn partition_splits_a_collection_in_two() {
    let res = partition(vec![1, 2, 3, 4], |&v| v % 2 == 0);
    assert_eq!(res.lhs, vec![2, 4]);
    assert_eq!(res.rhs, vec![1, 3]);
}

#[test]
fn pick_selects_present_keys() {
    let mut src: HashMap<String, i32> = HashMap::new();
    src.insert("a".to_string(), 1);
    src.insert("b".to_string(), 2);
    let got = pick(&src, &["a", "c"]);
    assert_eq!(got.len(), 1);
    assert_eq!(got["a"], 1);
}
