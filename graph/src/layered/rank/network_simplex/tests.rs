//! Port of `dagre/test/rank/network-simplex-test.ts`. Test names mirror the TS.
//!
//! `g` is the directed (multigraph) `DagreGraph`; `t` is the undirected tree
//! graph (`TreeGraph`). The internal helpers are called directly, exactly as
//! the TS reaches into `networkSimplex.initLowLimValues`, etc.

use super::*;
use crate::layered::graph::{Graph, GraphOptions};
use crate::layered::rank::feasible_tree::{TreeEdgeLabel, TreeNodeLabel};
use crate::layered::rank::util::longest_path;
use crate::layered::types::{EdgeLabel, NodeLabel};
use crate::layered::util::normalize_ranks;

// ── graph constructors mirroring the TS beforeEach ───────────────────────────

fn new_g() -> DagreGraph {
    let mut g: DagreGraph = Graph::new(GraphOptions {
        directed: true,
        multigraph: true,
        compound: false,
    });
    g.set_default_node_label(NodeLabel::default());
    g.set_default_edge_label(EdgeLabel {
        minlen: Some(1),
        weight: Some(1.0),
        ..Default::default()
    });
    g
}

fn new_t() -> TreeGraph {
    let mut t: TreeGraph = Graph::new(GraphOptions {
        directed: false,
        multigraph: false,
        compound: false,
    });
    t.set_default_node_label(TreeNodeLabel::default());
    t.set_default_edge_label(TreeEdgeLabel::default());
    t
}

fn gansner_graph() -> DagreGraph {
    let mut g: DagreGraph = Graph::new(GraphOptions::default());
    g.set_default_node_label(NodeLabel::default());
    g.set_default_edge_label(EdgeLabel {
        minlen: Some(1),
        weight: Some(1.0),
        ..Default::default()
    });
    g.ensure_path(&["a", "b", "c", "d", "h"]);
    g.ensure_path(&["a", "e", "g", "h"]);
    g.ensure_path(&["a", "f", "g"]);
    g
}

fn gansner_tree() -> TreeGraph {
    let mut t = new_t();
    t.ensure_path(&["a", "b", "c", "d", "h", "g", "e"]);
    t.ensure_edge("g", "f", None);
    t
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn ns(g: &mut DagreGraph) {
    network_simplex(g);
    normalize_ranks(g);
}

fn rank_of(g: &DagreGraph, v: &str) -> i32 {
    g.node(v).and_then(|n| n.rank).unwrap()
}

fn cutvalue(t: &TreeGraph, v: &str, w: &str) -> f64 {
    t.edge(v, w, None).and_then(|e| e.cutvalue).unwrap()
}

fn lim_of(t: &TreeGraph, v: &str) -> i32 {
    t.node(v).and_then(|n| n.lim).unwrap()
}

fn undirected_edge(e: &Edge) -> (String, String) {
    if e.v < e.w {
        (e.v.clone(), e.w.clone())
    } else {
        (e.w.clone(), e.v.clone())
    }
}

fn sorted_lims(t: &TreeGraph) -> Vec<i32> {
    let mut lims: Vec<i32> = t.nodes().iter().map(|v| lim_of(t, v)).collect();
    lims.sort();
    lims
}

// ── full ranking ─────────────────────────────────────────────────────────────

#[test]
fn can_assign_a_rank_to_a_single_node() {
    let mut g = new_g();
    g.ensure_node("a");
    ns(&mut g);
    assert_eq!(rank_of(&g, "a"), 0);
}

#[test]
fn can_assign_a_rank_to_a_2_node_connected_graph() {
    let mut g = new_g();
    g.ensure_edge("a", "b", None);
    ns(&mut g);
    assert_eq!(rank_of(&g, "a"), 0);
    assert_eq!(rank_of(&g, "b"), 1);
}

#[test]
fn can_assign_ranks_for_a_diamond() {
    let mut g = new_g();
    g.ensure_path(&["a", "b", "d"]);
    g.ensure_path(&["a", "c", "d"]);
    ns(&mut g);
    assert_eq!(rank_of(&g, "a"), 0);
    assert_eq!(rank_of(&g, "b"), 1);
    assert_eq!(rank_of(&g, "c"), 1);
    assert_eq!(rank_of(&g, "d"), 2);
}

#[test]
fn uses_the_minlen_attribute_on_the_edge() {
    let mut g = new_g();
    g.ensure_path(&["a", "b", "d"]);
    g.ensure_edge("a", "c", None);
    g.set_edge(
        "c",
        "d",
        EdgeLabel {
            minlen: Some(2),
            weight: Some(1.0),
            ..Default::default()
        },
        None,
    );
    ns(&mut g);
    assert_eq!(rank_of(&g, "a"), 0);
    assert_eq!(rank_of(&g, "b"), 2);
    assert_eq!(rank_of(&g, "c"), 1);
    assert_eq!(rank_of(&g, "d"), 3);
}

#[test]
fn can_rank_the_gansner_graph() {
    let mut g = gansner_graph();
    ns(&mut g);
    assert_eq!(rank_of(&g, "a"), 0);
    assert_eq!(rank_of(&g, "b"), 1);
    assert_eq!(rank_of(&g, "c"), 2);
    assert_eq!(rank_of(&g, "d"), 3);
    assert_eq!(rank_of(&g, "h"), 4);
    assert_eq!(rank_of(&g, "e"), 1);
    assert_eq!(rank_of(&g, "f"), 1);
    assert_eq!(rank_of(&g, "g"), 2);
}

#[test]
fn can_handle_multi_edges() {
    let mut g = new_g();
    g.ensure_path(&["a", "b", "c", "d"]);
    g.set_edge(
        "a",
        "e",
        EdgeLabel {
            weight: Some(2.0),
            minlen: Some(1),
            ..Default::default()
        },
        None,
    );
    g.ensure_edge("e", "d", None);
    g.set_edge(
        "b",
        "c",
        EdgeLabel {
            weight: Some(1.0),
            minlen: Some(2),
            ..Default::default()
        },
        Some("multi"),
    );
    ns(&mut g);
    assert_eq!(rank_of(&g, "a"), 0);
    assert_eq!(rank_of(&g, "b"), 1);
    // b -> c has minlen = 1 and minlen = 2, so it should be 2 ranks apart.
    assert_eq!(rank_of(&g, "c"), 3);
    assert_eq!(rank_of(&g, "d"), 4);
    assert_eq!(rank_of(&g, "e"), 1);
}

// ── leaveEdge ────────────────────────────────────────────────────────────────

#[test]
fn leave_edge_returns_none_if_no_negative_cutvalue() {
    let mut tree = new_t();
    tree.set_edge("a", "b", TreeEdgeLabel { cutvalue: Some(1.0) }, None);
    tree.set_edge("b", "c", TreeEdgeLabel { cutvalue: Some(1.0) }, None);
    assert!(leave_edge(&tree).is_none());
}

#[test]
fn leave_edge_returns_an_edge_if_found_with_negative_cutvalue() {
    let mut tree = new_t();
    tree.set_edge("a", "b", TreeEdgeLabel { cutvalue: Some(1.0) }, None);
    tree.set_edge("b", "c", TreeEdgeLabel { cutvalue: Some(-1.0) }, None);
    let e = leave_edge(&tree).unwrap();
    assert_eq!((e.v.as_str(), e.w.as_str()), ("b", "c"));
}

// ── enterEdge ────────────────────────────────────────────────────────────────

fn enter_g_3() -> DagreGraph {
    let mut g = new_g();
    g.set_node("a", NodeLabel { rank: Some(0), ..Default::default() });
    g.set_node("b", NodeLabel { rank: Some(2), ..Default::default() });
    g.set_node("c", NodeLabel { rank: Some(3), ..Default::default() });
    g.ensure_path(&["a", "b", "c"]);
    g.ensure_edge("a", "c", None);
    g
}

#[test]
fn enter_edge_finds_edge_from_head_to_tail_component() {
    let g = enter_g_3();
    let mut t = new_t();
    t.ensure_path(&["b", "c", "a"]);
    init_low_lim_values(&mut t, Some("c"));

    let f = enter_edge(&t, &g, &Edge::new("b", "c", None));
    assert_eq!(undirected_edge(&f), undirected_edge(&Edge::new("a", "b", None)));
}

#[test]
fn enter_edge_works_when_root_in_tail_component() {
    let g = enter_g_3();
    let mut t = new_t();
    t.ensure_path(&["b", "c", "a"]);
    init_low_lim_values(&mut t, Some("b"));

    let f = enter_edge(&t, &g, &Edge::new("b", "c", None));
    assert_eq!(undirected_edge(&f), undirected_edge(&Edge::new("a", "b", None)));
}

#[test]
fn enter_edge_finds_edge_with_least_slack() {
    let mut g = new_g();
    g.set_node("a", NodeLabel { rank: Some(0), ..Default::default() });
    g.set_node("b", NodeLabel { rank: Some(1), ..Default::default() });
    g.set_node("c", NodeLabel { rank: Some(3), ..Default::default() });
    g.set_node("d", NodeLabel { rank: Some(4), ..Default::default() });
    g.ensure_edge("a", "d", None);
    g.ensure_path(&["a", "c", "d"]);
    g.ensure_edge("b", "c", None);
    let mut t = new_t();
    t.ensure_path(&["c", "d", "a", "b"]);
    init_low_lim_values(&mut t, Some("a"));

    let f = enter_edge(&t, &g, &Edge::new("c", "d", None));
    assert_eq!(undirected_edge(&f), undirected_edge(&Edge::new("b", "c", None)));
}

#[test]
fn enter_edge_gansner_1() {
    let mut g = gansner_graph();
    let mut t = gansner_tree();
    longest_path(&mut g);
    init_low_lim_values(&mut t, Some("a"));
    let f = enter_edge(&t, &g, &Edge::new("g", "h", None));
    let ue = undirected_edge(&f);
    assert_eq!(ue.0, "a");
    assert!(ue.1 == "e" || ue.1 == "f");
}

#[test]
fn enter_edge_gansner_2() {
    let mut g = gansner_graph();
    let mut t = gansner_tree();
    longest_path(&mut g);
    init_low_lim_values(&mut t, Some("e"));
    let f = enter_edge(&t, &g, &Edge::new("g", "h", None));
    let ue = undirected_edge(&f);
    assert_eq!(ue.0, "a");
    assert!(ue.1 == "e" || ue.1 == "f");
}

#[test]
fn enter_edge_gansner_3() {
    let mut g = gansner_graph();
    let mut t = gansner_tree();
    longest_path(&mut g);
    init_low_lim_values(&mut t, Some("a"));
    let f = enter_edge(&t, &g, &Edge::new("h", "g", None));
    let ue = undirected_edge(&f);
    assert_eq!(ue.0, "a");
    assert!(ue.1 == "e" || ue.1 == "f");
}

#[test]
fn enter_edge_gansner_4() {
    let mut g = gansner_graph();
    let mut t = gansner_tree();
    longest_path(&mut g);
    init_low_lim_values(&mut t, Some("e"));
    let f = enter_edge(&t, &g, &Edge::new("h", "g", None));
    let ue = undirected_edge(&f);
    assert_eq!(ue.0, "a");
    assert!(ue.1 == "e" || ue.1 == "f");
}

// ── initLowLimValues ─────────────────────────────────────────────────────────

#[test]
fn init_low_lim_assigns_low_lim_parent() {
    let mut g = new_t();
    g.set_default_node_label(TreeNodeLabel::default());
    g.ensure_nodes(&["a", "b", "c", "d", "e"]);
    g.ensure_path(&["a", "b", "a", "c", "d", "c", "e"]);

    init_low_lim_values(&mut g, Some("a"));

    assert_eq!(sorted_lims(&g), vec![1, 2, 3, 4, 5]);

    let a = g.node("a").unwrap().clone();
    assert_eq!(a.low, Some(1));
    assert_eq!(a.lim, Some(5));

    let b = g.node("b").unwrap().clone();
    assert_eq!(b.parent.as_deref(), Some("a"));
    assert!(b.lim.unwrap() < a.lim.unwrap());

    let c = g.node("c").unwrap().clone();
    assert_eq!(c.parent.as_deref(), Some("a"));
    assert!(c.lim.unwrap() < a.lim.unwrap());
    assert_ne!(c.lim, b.lim);

    let d = g.node("d").unwrap().clone();
    assert_eq!(d.parent.as_deref(), Some("c"));
    assert!(d.lim.unwrap() < c.lim.unwrap());

    let e = g.node("e").unwrap().clone();
    assert_eq!(e.parent.as_deref(), Some("c"));
    assert!(e.lim.unwrap() < c.lim.unwrap());
    assert_ne!(e.lim, d.lim);
}

// ── exchangeEdges ────────────────────────────────────────────────────────────

#[test]
fn exchange_edges_updates_cut_values_and_low_lim() {
    let mut g = gansner_graph();
    let mut t = gansner_tree();
    longest_path(&mut g);
    init_low_lim_values(&mut t, None);

    exchange_edges(
        &mut t,
        &mut g,
        &Edge::new("g", "h", None),
        &Edge::new("a", "e", None),
    );

    assert_eq!(cutvalue(&t, "a", "b"), 2.0);
    assert_eq!(cutvalue(&t, "b", "c"), 2.0);
    assert_eq!(cutvalue(&t, "c", "d"), 2.0);
    assert_eq!(cutvalue(&t, "d", "h"), 2.0);
    assert_eq!(cutvalue(&t, "a", "e"), 1.0);
    assert_eq!(cutvalue(&t, "e", "g"), 1.0);
    assert_eq!(cutvalue(&t, "g", "f"), 0.0);

    assert_eq!(sorted_lims(&t), vec![1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn exchange_edges_updates_ranks() {
    let mut g = gansner_graph();
    let mut t = gansner_tree();
    longest_path(&mut g);
    init_low_lim_values(&mut t, None);

    exchange_edges(
        &mut t,
        &mut g,
        &Edge::new("g", "h", None),
        &Edge::new("a", "e", None),
    );
    normalize_ranks(&mut g);

    assert_eq!(rank_of(&g, "a"), 0);
    assert_eq!(rank_of(&g, "b"), 1);
    assert_eq!(rank_of(&g, "c"), 2);
    assert_eq!(rank_of(&g, "d"), 3);
    assert_eq!(rank_of(&g, "e"), 1);
    assert_eq!(rank_of(&g, "f"), 1);
    assert_eq!(rank_of(&g, "g"), 2);
    assert_eq!(rank_of(&g, "h"), 4);
}

// ── calcCutValue ─────────────────────────────────────────────────────────────
//
// p = parent, c = child, gc_x = grandchild, o = other.

fn cv_tree_edge(cutvalue: i32) -> TreeEdgeLabel {
    TreeEdgeLabel {
        cutvalue: Some(cutvalue as f64),
    }
}

fn weight_edge(weight: f64) -> EdgeLabel {
    EdgeLabel {
        weight: Some(weight),
        minlen: Some(1),
        ..Default::default()
    }
}

#[test]
fn calc_cut_value_2node_c_to_p() {
    let mut g = new_g();
    g.ensure_path(&["c", "p"]);
    let mut t = new_t();
    t.ensure_path(&["p", "c"]);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), 1.0);
}

#[test]
fn calc_cut_value_2node_c_from_p() {
    let mut g = new_g();
    g.ensure_path(&["p", "c"]);
    let mut t = new_t();
    t.ensure_path(&["p", "c"]);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), 1.0);
}

#[test]
fn calc_cut_value_3node_gc_c_p() {
    let mut g = new_g();
    g.ensure_path(&["gc", "c", "p"]);
    let mut t = new_t();
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_edge("p", "c", None);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), 3.0);
}

#[test]
fn calc_cut_value_3node_gc_to_c_from_p() {
    let mut g = new_g();
    g.ensure_edge("p", "c", None);
    g.ensure_edge("gc", "c", None);
    let mut t = new_t();
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_edge("p", "c", None);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), -1.0);
}

#[test]
fn calc_cut_value_3node_gc_from_c_to_p() {
    let mut g = new_g();
    g.ensure_edge("c", "p", None);
    g.ensure_edge("c", "gc", None);
    let mut t = new_t();
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_edge("p", "c", None);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), -1.0);
}

#[test]
fn calc_cut_value_3node_gc_from_c_from_p() {
    let mut g = new_g();
    g.ensure_path(&["p", "c", "gc"]);
    let mut t = new_t();
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_edge("p", "c", None);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), 3.0);
}

#[test]
fn calc_cut_value_4node_gc_c_p_o_with_o_to_c() {
    let mut g = new_g();
    g.set_edge("o", "c", weight_edge(7.0), None);
    g.ensure_path(&["gc", "c", "p", "o"]);
    let mut t = new_t();
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_path(&["c", "p", "o"]);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), -4.0);
}

#[test]
fn calc_cut_value_4node_gc_c_p_o_with_o_from_c() {
    let mut g = new_g();
    g.set_edge("c", "o", weight_edge(7.0), None);
    g.ensure_path(&["gc", "c", "p", "o"]);
    let mut t = new_t();
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_path(&["c", "p", "o"]);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), 10.0);
}

#[test]
fn calc_cut_value_4node_o_gc_c_p_with_o_to_c() {
    let mut g = new_g();
    g.set_edge("o", "c", weight_edge(7.0), None);
    g.ensure_path(&["o", "gc", "c", "p"]);
    let mut t = new_t();
    t.ensure_edge("o", "gc", None);
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_edge("c", "p", None);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), -4.0);
}

#[test]
fn calc_cut_value_4node_o_gc_c_p_with_o_from_c() {
    let mut g = new_g();
    g.set_edge("c", "o", weight_edge(7.0), None);
    g.ensure_path(&["o", "gc", "c", "p"]);
    let mut t = new_t();
    t.ensure_edge("o", "gc", None);
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_edge("c", "p", None);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), 10.0);
}

#[test]
fn calc_cut_value_4node_gc_c_back_p_o_with_o_to_c() {
    let mut g = new_g();
    g.ensure_edge("gc", "c", None);
    g.ensure_edge("p", "c", None);
    g.ensure_edge("p", "o", None);
    g.set_edge("o", "c", weight_edge(7.0), None);
    let mut t = new_t();
    t.ensure_edge("o", "gc", None);
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_edge("c", "p", None);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), 6.0);
}

#[test]
fn calc_cut_value_4node_gc_c_back_p_o_with_o_from_c() {
    let mut g = new_g();
    g.ensure_edge("gc", "c", None);
    g.ensure_edge("p", "c", None);
    g.ensure_edge("p", "o", None);
    g.set_edge("c", "o", weight_edge(7.0), None);
    let mut t = new_t();
    t.ensure_edge("o", "gc", None);
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_edge("c", "p", None);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), -8.0);
}

#[test]
fn calc_cut_value_4node_o_gc_c_back_p_with_o_to_c() {
    let mut g = new_g();
    g.set_edge("o", "c", weight_edge(7.0), None);
    g.ensure_path(&["o", "gc", "c"]);
    g.ensure_edge("p", "c", None);
    let mut t = new_t();
    t.ensure_edge("o", "gc", None);
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_edge("c", "p", None);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), 6.0);
}

#[test]
fn calc_cut_value_4node_o_gc_c_back_p_with_o_from_c() {
    let mut g = new_g();
    g.set_edge("c", "o", weight_edge(7.0), None);
    g.ensure_path(&["o", "gc", "c"]);
    g.ensure_edge("p", "c", None);
    let mut t = new_t();
    t.ensure_edge("o", "gc", None);
    t.set_edge("gc", "c", cv_tree_edge(3), None);
    t.ensure_edge("c", "p", None);
    init_low_lim_values(&mut t, Some("p"));
    assert_eq!(calc_cut_value(&t, &g, "c"), -8.0);
}

// ── initCutValues ────────────────────────────────────────────────────────────

#[test]
fn init_cut_values_gansner_graph() {
    let g = gansner_graph();
    let mut t = gansner_tree();
    // longestPath is NOT called in the TS test here; gansnerGraph has no ranks
    // assigned, but initCutValues only reads weights and tree structure.
    init_low_lim_values(&mut t, None);
    init_cut_values(&mut t, &g);
    assert_eq!(cutvalue(&t, "a", "b"), 3.0);
    assert_eq!(cutvalue(&t, "b", "c"), 3.0);
    assert_eq!(cutvalue(&t, "c", "d"), 3.0);
    assert_eq!(cutvalue(&t, "d", "h"), 3.0);
    assert_eq!(cutvalue(&t, "g", "h"), -1.0);
    assert_eq!(cutvalue(&t, "e", "g"), 0.0);
    assert_eq!(cutvalue(&t, "f", "g"), 0.0);
}

#[test]
fn init_cut_values_updated_gansner_graph() {
    let g = gansner_graph();
    let mut t = gansner_tree();
    t.remove_edge("g", "h", None);
    t.ensure_edge("a", "e", None);
    init_low_lim_values(&mut t, None);
    init_cut_values(&mut t, &g);
    assert_eq!(cutvalue(&t, "a", "b"), 2.0);
    assert_eq!(cutvalue(&t, "b", "c"), 2.0);
    assert_eq!(cutvalue(&t, "c", "d"), 2.0);
    assert_eq!(cutvalue(&t, "d", "h"), 2.0);
    assert_eq!(cutvalue(&t, "a", "e"), 1.0);
    assert_eq!(cutvalue(&t, "e", "g"), 1.0);
    assert_eq!(cutvalue(&t, "f", "g"), 0.0);
}
