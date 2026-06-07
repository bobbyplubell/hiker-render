//! Conformance tests for [`layout`] — a port of `dagre/test/layout-test.ts`.
//!
//! Each test mirrors a `it(...)` from the oracle. The input graph is a
//! `multigraph + compound` graph with a default edge label of
//! `EdgeLabel::default()` (TS `setDefaultEdgeLabel(() => ({}))`).

use crate::layered::graph::{Edge, Graph, GraphOptions};
use crate::layered::layout::{layout, layout_with_opts, LayoutOptions};
use crate::layered::types::{
    DagreGraph, EdgeLabel, GraphLabel, LabelPos, NodeLabel, RankDir,
};

fn new_graph() -> DagreGraph {
    let mut g: DagreGraph = Graph::new(GraphOptions {
        directed: true,
        multigraph: true,
        compound: true,
    });
    g.set_graph(GraphLabel::default());
    g.set_default_edge_label(EdgeLabel::default());
    g
}

fn node(width: f64, height: f64) -> NodeLabel {
    NodeLabel {
        width,
        height,
        ..Default::default()
    }
}

fn nx(g: &DagreGraph, v: &str) -> f64 {
    g.node(v).unwrap().x.unwrap()
}
fn ny(g: &DagreGraph, v: &str) -> f64 {
    g.node(v).unwrap().y.unwrap()
}
fn edge<'a>(g: &'a DagreGraph, v: &str, w: &str) -> &'a EdgeLabel {
    g.edge(v, w, None).unwrap()
}

#[test]
fn can_layout_a_single_node() {
    let mut g = new_graph();
    g.set_node("a", node(50.0, 100.0));
    layout(&mut g);
    assert_eq!(nx(&g, "a"), 50.0 / 2.0);
    assert_eq!(ny(&g, "a"), 100.0 / 2.0);
}

#[test]
fn can_layout_two_nodes_on_the_same_rank() {
    let mut g = new_graph();
    g.graph_mut().unwrap().nodesep = Some(200.0);
    g.set_node("a", node(50.0, 100.0));
    g.set_node("b", node(75.0, 200.0));
    layout(&mut g);
    assert_eq!(nx(&g, "a"), 50.0 / 2.0);
    assert_eq!(ny(&g, "a"), 200.0 / 2.0);
    assert_eq!(nx(&g, "b"), 50.0 + 200.0 + 75.0 / 2.0);
    assert_eq!(ny(&g, "b"), 200.0 / 2.0);
}

#[test]
fn can_layout_two_nodes_connected_by_an_edge() {
    let mut g = new_graph();
    g.graph_mut().unwrap().ranksep = Some(300.0);
    g.set_node("a", node(50.0, 100.0));
    g.set_node("b", node(75.0, 200.0));
    g.set_edge("a", "b", EdgeLabel::default(), None);
    layout(&mut g);
    assert_eq!(nx(&g, "a"), 75.0 / 2.0);
    assert_eq!(ny(&g, "a"), 100.0 / 2.0);
    assert_eq!(nx(&g, "b"), 75.0 / 2.0);
    assert_eq!(ny(&g, "b"), 100.0 + 300.0 + 200.0 / 2.0);

    // No label → no x/y on the edge.
    let e = edge(&g, "a", "b");
    assert!(e.x.is_none());
    assert!(e.y.is_none());
}

#[test]
fn can_layout_an_edge_with_a_label() {
    let mut g = new_graph();
    g.graph_mut().unwrap().ranksep = Some(300.0);
    g.set_node("a", node(50.0, 100.0));
    g.set_node("b", node(75.0, 200.0));
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            width: Some(60.0),
            height: Some(70.0),
            label_pos: Some(LabelPos::C),
            ..Default::default()
        },
        None,
    );
    layout(&mut g);
    assert_eq!(nx(&g, "a"), 75.0 / 2.0);
    assert_eq!(ny(&g, "a"), 100.0 / 2.0);
    assert_eq!(nx(&g, "b"), 75.0 / 2.0);
    assert_eq!(ny(&g, "b"), 100.0 + 150.0 + 70.0 + 150.0 + 200.0 / 2.0);
    let e = edge(&g, "a", "b");
    assert_eq!(e.x.unwrap(), 75.0 / 2.0);
    assert_eq!(e.y.unwrap(), 100.0 + 150.0 + 70.0 / 2.0);
}

#[test]
fn can_layout_an_edge_with_a_long_label_all_rankdirs() {
    for rankdir in [RankDir::Tb, RankDir::Bt, RankDir::Lr, RankDir::Rl] {
        let mut g = new_graph();
        {
            let gl = g.graph_mut().unwrap();
            gl.nodesep = Some(10.0);
            gl.edgesep = Some(10.0);
            gl.rankdir = Some(rankdir);
        }
        for v in ["a", "b", "c", "d"] {
            g.set_node(v, node(10.0, 10.0));
        }
        g.set_edge(
            "a",
            "c",
            EdgeLabel {
                width: Some(2000.0),
                height: Some(10.0),
                label_pos: Some(LabelPos::C),
                ..Default::default()
            },
            None,
        );
        g.set_edge(
            "b",
            "d",
            EdgeLabel {
                width: Some(1.0),
                height: Some(1.0),
                ..Default::default()
            },
            None,
        );
        layout(&mut g);

        let (p1x, p2x) = if matches!(rankdir, RankDir::Tb | RankDir::Bt) {
            (edge(&g, "a", "c").x.unwrap(), edge(&g, "b", "d").x.unwrap())
        } else {
            (nx(&g, "a"), nx(&g, "c"))
        };
        assert!(
            (p1x - p2x).abs() > 1000.0,
            "rankdir {rankdir:?}: |{p1x} - {p2x}| not > 1000"
        );
    }
}

#[test]
fn can_apply_an_offset_all_rankdirs() {
    for rankdir in [RankDir::Tb, RankDir::Bt, RankDir::Lr, RankDir::Rl] {
        let mut g = new_graph();
        {
            let gl = g.graph_mut().unwrap();
            gl.nodesep = Some(10.0);
            gl.edgesep = Some(10.0);
            gl.rankdir = Some(rankdir);
        }
        for v in ["a", "b", "c", "d"] {
            g.set_node(v, node(10.0, 10.0));
        }
        g.set_edge(
            "a",
            "b",
            EdgeLabel {
                width: Some(10.0),
                height: Some(10.0),
                label_pos: Some(LabelPos::L),
                label_offset: Some(1000.0),
                ..Default::default()
            },
            None,
        );
        g.set_edge(
            "c",
            "d",
            EdgeLabel {
                width: Some(10.0),
                height: Some(10.0),
                label_pos: Some(LabelPos::R),
                label_offset: Some(1000.0),
                ..Default::default()
            },
            None,
        );
        layout(&mut g);

        let ab = edge(&g, "a", "b");
        let cd = edge(&g, "c", "d");
        if matches!(rankdir, RankDir::Tb | RankDir::Bt) {
            assert_eq!(ab.x.unwrap() - ab.points.as_ref().unwrap()[0].x, -1000.0 - 10.0 / 2.0);
            assert_eq!(cd.x.unwrap() - cd.points.as_ref().unwrap()[0].x, 1000.0 + 10.0 / 2.0);
        } else {
            assert_eq!(ab.y.unwrap() - ab.points.as_ref().unwrap()[0].y, -1000.0 - 10.0 / 2.0);
            assert_eq!(cd.y.unwrap() - cd.points.as_ref().unwrap()[0].y, 1000.0 + 10.0 / 2.0);
        }
    }
}

#[test]
fn can_layout_a_long_edge_with_a_label() {
    let mut g = new_graph();
    g.graph_mut().unwrap().ranksep = Some(300.0);
    g.set_node("a", node(50.0, 100.0));
    g.set_node("b", node(75.0, 200.0));
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            width: Some(60.0),
            height: Some(70.0),
            minlen: Some(2),
            label_pos: Some(LabelPos::C),
            ..Default::default()
        },
        None,
    );
    layout(&mut g);
    let e = edge(&g, "a", "b");
    assert_eq!(e.x.unwrap(), 75.0 / 2.0);
    assert!(e.y.unwrap() > ny(&g, "a"));
    assert!(e.y.unwrap() < ny(&g, "b"));
}

#[test]
fn can_layout_a_short_cycle() {
    let mut g = new_graph();
    g.graph_mut().unwrap().ranksep = Some(200.0);
    g.set_node("a", node(100.0, 100.0));
    g.set_node("b", node(100.0, 100.0));
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            weight: Some(2.0),
            ..Default::default()
        },
        None,
    );
    g.set_edge("b", "a", EdgeLabel::default(), None);
    layout(&mut g);
    assert_eq!(nx(&g, "a"), 100.0 / 2.0);
    assert_eq!(ny(&g, "a"), 100.0 / 2.0);
    assert_eq!(nx(&g, "b"), 100.0 / 2.0);
    assert_eq!(ny(&g, "b"), 100.0 + 200.0 + 100.0 / 2.0);
    // One arrow points down, one up.
    let ab = edge(&g, "a", "b").points.as_ref().unwrap();
    assert!(ab[1].y > ab[0].y);
    let ba = edge(&g, "b", "a").points.as_ref().unwrap();
    assert!(ba[0].y > ba[1].y);
}

#[test]
fn adds_rectangle_intersects_for_edges() {
    let mut g = new_graph();
    g.graph_mut().unwrap().ranksep = Some(200.0);
    g.set_node("a", node(100.0, 100.0));
    g.set_node("b", node(100.0, 100.0));
    g.set_edge("a", "b", EdgeLabel::default(), None);
    layout(&mut g);
    let points = edge(&g, "a", "b").points.as_ref().unwrap();
    assert_eq!(points.len(), 3);
    assert_eq!((points[0].x, points[0].y), (100.0 / 2.0, 100.0));
    assert_eq!((points[1].x, points[1].y), (100.0 / 2.0, 100.0 + 200.0 / 2.0));
    assert_eq!((points[2].x, points[2].y), (100.0 / 2.0, 100.0 + 200.0));
}

#[test]
fn adds_rectangle_intersects_for_edges_spanning_multiple_ranks() {
    let mut g = new_graph();
    g.graph_mut().unwrap().ranksep = Some(200.0);
    g.set_node("a", node(100.0, 100.0));
    g.set_node("b", node(100.0, 100.0));
    g.set_edge(
        "a",
        "b",
        EdgeLabel {
            minlen: Some(2),
            ..Default::default()
        },
        None,
    );
    layout(&mut g);
    let points = edge(&g, "a", "b").points.as_ref().unwrap();
    assert_eq!(points.len(), 5);
    let expected = [
        (100.0 / 2.0, 100.0),
        (100.0 / 2.0, 100.0 + 200.0 / 2.0),
        (100.0 / 2.0, 100.0 + 400.0 / 2.0),
        (100.0 / 2.0, 100.0 + 600.0 / 2.0),
        (100.0 / 2.0, 100.0 + 800.0 / 2.0),
    ];
    for (p, e) in points.iter().zip(expected.iter()) {
        assert_eq!((p.x, p.y), *e);
    }
}

#[test]
fn can_layout_a_self_loop_all_rankdirs() {
    for rankdir in [RankDir::Tb, RankDir::Bt, RankDir::Lr, RankDir::Rl] {
        let mut g = new_graph();
        {
            let gl = g.graph_mut().unwrap();
            gl.edgesep = Some(75.0);
            gl.rankdir = Some(rankdir);
        }
        g.set_node("a", node(100.0, 100.0));
        g.set_edge(
            "a",
            "a",
            EdgeLabel {
                width: Some(50.0),
                height: Some(50.0),
                ..Default::default()
            },
            None,
        );
        layout(&mut g);
        let node_a = g.node("a").unwrap().clone();
        let points = edge(&g, "a", "a").points.as_ref().unwrap();
        assert_eq!(points.len(), 7, "rankdir {rankdir:?}");
        for point in points {
            if !matches!(rankdir, RankDir::Lr | RankDir::Rl) {
                assert!(point.x > node_a.x.unwrap());
                assert!((point.y - node_a.y.unwrap()).abs() <= node_a.height / 2.0);
            } else {
                assert!(point.y > node_a.y.unwrap());
                assert!((point.x - node_a.x.unwrap()).abs() <= node_a.width / 2.0);
            }
        }
    }
}

#[test]
fn can_layout_a_graph_with_subgraphs() {
    // Primarily ensures nothing blows up.
    let mut g = new_graph();
    g.set_node("a", node(50.0, 50.0));
    g.set_parent("a", "sg1");
    layout(&mut g);
}

#[test]
fn minimizes_the_height_of_subgraphs() {
    let mut g = new_graph();
    for v in ["a", "b", "c", "d", "x", "y"] {
        g.set_node(v, node(50.0, 50.0));
    }
    g.set_path(&["a", "b", "c", "d"], EdgeLabel::default());
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
        "y",
        "d",
        EdgeLabel {
            weight: Some(100.0),
            ..Default::default()
        },
        None,
    );
    g.set_parent("x", "sg");
    g.set_parent("y", "sg");
    layout(&mut g);
    assert_eq!(ny(&g, "x"), ny(&g, "y"));
}

#[test]
fn minimizes_separation_between_nodes_not_adjacent_to_subgraphs() {
    let mut g = new_graph();
    for v in ["a", "b", "c"] {
        g.set_node(v, node(50.0, 50.0));
    }
    g.set_path(&["a", "b", "c"], EdgeLabel::default());
    g.set_node("sg", NodeLabel::default());
    g.set_parent("c", "sg");
    layout(&mut g);
    assert_eq!(ny(&g, "b") - ny(&g, "a"), 100.0);
}

#[test]
fn can_layout_subgraphs_with_different_rankdirs() {
    let mut g = new_graph();
    g.set_node("a", node(50.0, 50.0));
    g.set_node("sg", NodeLabel::default());
    g.set_parent("a", "sg");

    for rankdir in [RankDir::Tb, RankDir::Bt, RankDir::Lr, RankDir::Rl] {
        g.graph_mut().unwrap().rankdir = Some(rankdir);
        layout(&mut g);
        let sg = g.node("sg").unwrap();
        assert!(sg.width > 50.0, "rankdir {rankdir:?} width {}", sg.width);
        assert!(sg.height > 50.0, "rankdir {rankdir:?} height {}", sg.height);
        assert!(sg.x.unwrap() > 50.0 / 2.0, "rankdir {rankdir:?}");
        assert!(sg.y.unwrap() > 50.0 / 2.0, "rankdir {rankdir:?}");
    }
}

#[test]
fn adds_dimensions_to_the_graph() {
    let mut g = new_graph();
    g.set_node("a", node(100.0, 50.0));
    layout(&mut g);
    let gl = g.graph().unwrap();
    assert_eq!(gl.width.unwrap(), 100.0);
    assert_eq!(gl.height.unwrap(), 50.0);
}

#[test]
fn coordinates_in_bounding_box_node_all_rankdirs() {
    for rankdir in [RankDir::Tb, RankDir::Bt, RankDir::Lr, RankDir::Rl] {
        let mut g = new_graph();
        g.graph_mut().unwrap().rankdir = Some(rankdir);
        g.set_node("a", node(100.0, 200.0));
        layout(&mut g);
        assert_eq!(nx(&g, "a"), 100.0 / 2.0, "rankdir {rankdir:?}");
        assert_eq!(ny(&g, "a"), 200.0 / 2.0, "rankdir {rankdir:?}");
    }
}

#[test]
fn coordinates_in_bounding_box_edge_labelpos_l_all_rankdirs() {
    for rankdir in [RankDir::Tb, RankDir::Bt, RankDir::Lr, RankDir::Rl] {
        let mut g = new_graph();
        g.graph_mut().unwrap().rankdir = Some(rankdir);
        g.set_node("a", node(100.0, 100.0));
        g.set_node("b", node(100.0, 100.0));
        g.set_edge(
            "a",
            "b",
            EdgeLabel {
                width: Some(1000.0),
                height: Some(2000.0),
                label_pos: Some(LabelPos::L),
                label_offset: Some(0.0),
                ..Default::default()
            },
            None,
        );
        layout(&mut g);
        let e = edge(&g, "a", "b");
        if matches!(rankdir, RankDir::Tb | RankDir::Bt) {
            assert_eq!(e.x.unwrap(), 1000.0 / 2.0, "rankdir {rankdir:?}");
        } else {
            assert_eq!(e.y.unwrap(), 2000.0 / 2.0, "rankdir {rankdir:?}");
        }
    }
}

#[test]
fn treats_attributes_with_case_insensitivity() {
    // The TS test sets `nodeSep` (capital S) to verify case-insensitive attr
    // handling. With typed labels there is no mixed-case key; the behaviour
    // under test (nodesep honoured) is intrinsic, so we set `nodesep` directly.
    let mut g = new_graph();
    g.graph_mut().unwrap().nodesep = Some(200.0);
    g.set_node("a", node(50.0, 100.0));
    g.set_node("b", node(75.0, 200.0));
    layout(&mut g);
    assert_eq!(nx(&g, "a"), 50.0 / 2.0);
    assert_eq!(ny(&g, "a"), 200.0 / 2.0);
    assert_eq!(nx(&g, "b"), 50.0 + 200.0 + 75.0 / 2.0);
    assert_eq!(ny(&g, "b"), 200.0 / 2.0);
}

#[test]
fn layout_with_opts_is_callable() {
    let mut g = new_graph();
    g.set_node("a", node(50.0, 100.0));
    layout_with_opts(
        &mut g,
        &LayoutOptions {
            debug_timing: false,
            disable_optimal_order_heuristic: false,
        },
    );
    assert_eq!(nx(&g, "a"), 25.0);
}

// Silence unused-import warnings if Edge is not otherwise referenced.
#[allow(dead_code)]
fn _use_edge(_e: Edge) {}
