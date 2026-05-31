//! Layer-graph construction — a port of `dagre/lib/order/build-layer-graph.ts`.
//!
//! Builds a compound graph for sorting a single rank: all base/subgraph nodes
//! of that rank in their original hierarchy (roots re-parented under a fresh
//! `root` node recorded in the graph label), plus the in- or out-edges incident
//! on them (per `relationship`), with weights aggregated since the result is a
//! simple graph.
//!
//! # Fidelity notes
//!
//! In dagre the layer-graph node label *is* the original node object (via
//! `setDefaultNodeLabel(v => graph.node(v))`), so `order` written during a sweep
//! is shared back. Here the layer node ([`OrderNode`]) carries a snapshot of the
//! original node's `order`; the [`order`](super::order) driver re-syncs orders
//! from the original graph into the layer graph before each sweep and writes the
//! resulting orders back, reproducing the shared-object semantics.

use super::graph::Graph;
use super::{LayerGraphLabel, OrderEdge, OrderNode};
use super::util;
use crate::layered::types::{EdgeLabel, GraphLabel, NodeLabel};

/// Which incident edges to copy into the layer graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relationship {
    InEdges,
    OutEdges,
}

/// The layer graph type produced by [`build_layer_graph`].
pub type LayerGraph = Graph<LayerGraphLabel, OrderNode, OrderEdge>;

fn create_root_node(graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>) -> String {
    loop {
        let v = util::unique_id("_root");
        if !graph.has_node(&v) {
            return v;
        }
    }
}

/// `buildLayerGraph(graph, rank, relationship, nodesWithRank)`.
pub fn build_layer_graph(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
    rank: i32,
    relationship: Relationship,
    nodes_with_rank: Option<&[String]>,
) -> LayerGraph {
    let owned_all;
    let nodes_with_rank: &[String] = match nodes_with_rank {
        Some(n) => n,
        None => {
            owned_all = graph.nodes();
            &owned_all
        }
    };

    let root = create_root_node(graph);
    let mut result: LayerGraph = Graph::new(crate::layered::graph::GraphOptions {
        directed: true,
        multigraph: false,
        compound: true,
    });
    result.set_graph(LayerGraphLabel { root: root.clone() });

    for v in nodes_with_rank {
        let node = match graph.node(v) {
            Some(n) => n,
            None => continue,
        };
        let in_range = node.rank == Some(rank)
            || (node.min_rank.map(|mn| mn <= rank).unwrap_or(false)
                && node.max_rank.map(|mx| rank <= mx).unwrap_or(false));
        if !in_range {
            continue;
        }

        // setNode(v) using the default node label = snapshot of original order.
        let order_snapshot = node.order;
        result.set_node(
            v.clone(),
            OrderNode {
                order: order_snapshot,
                border_left: None,
                border_right: None,
            },
        );
        match graph.parent(v) {
            Some(p) => {
                result.set_parent(v.clone(), p);
            }
            None => {
                result.set_parent(v.clone(), root.clone());
            }
        }

        // This assumes we have only short edges!
        let edges = match relationship {
            Relationship::InEdges => graph.in_edges(v, None),
            Relationship::OutEdges => graph.out_edges(v, None),
        }
        .unwrap_or_default();
        for e in &edges {
            let u = if e.v == *v { e.w.clone() } else { e.v.clone() };
            let existing = result.edge(&u, v, None).and_then(|l| l.weight).unwrap_or(0.0);
            let ew = graph.edge_by_obj(e).and_then(|l| l.weight).unwrap_or(0.0);
            result.set_edge(
                u,
                v.clone(),
                OrderEdge {
                    weight: Some(ew + existing),
                },
                None,
            );
        }

        // Object.hasOwn(node, "minRank") — node is a subgraph node.
        if node.min_rank.is_some() {
            let bl = node
                .border_left
                .as_ref()
                .and_then(|arr| arr.get(rank as usize))
                .cloned();
            let br = node
                .border_right
                .as_ref()
                .and_then(|arr| arr.get(rank as usize))
                .cloned();
            result.set_node(
                v.clone(),
                OrderNode {
                    order: order_snapshot,
                    border_left: bl,
                    border_right: br,
                },
            );
        }
    }

    // In dagre the layer-graph node label *is* the original node object
    // (`setDefaultNodeLabel(v => graph.node(v))`), so every node — including the
    // non-movable neighbours auto-created by `setEdge` — exposes the original
    // `order`. Reproduce that by syncing `order` onto all result nodes.
    for v in result.nodes() {
        let order = graph.node(&v).and_then(|n| n.order);
        if let Some(ln) = result.node_mut(&v) {
            if ln.order.is_none() {
                ln.order = order;
            }
        } else {
            // Node label was None (auto-created without a label); give it one.
            result.set_node(
                v.clone(),
                OrderNode {
                    order,
                    border_left: None,
                    border_right: None,
                },
            );
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layered::graph::GraphOptions;
    use crate::layered::util::GRAPH_NODE;

    fn base_graph() -> Graph<GraphLabel, NodeLabel, EdgeLabel> {
        Graph::new(GraphOptions {
            directed: true,
            multigraph: true,
            compound: true,
        })
    }

    fn node_rank(r: i32) -> NodeLabel {
        NodeLabel {
            rank: Some(r),
            ..Default::default()
        }
    }

    fn w(weight: f64) -> EdgeLabel {
        EdgeLabel {
            weight: Some(weight),
            ..Default::default()
        }
    }

    #[test]
    fn places_movable_nodes_with_no_parents_under_root() {
        let mut g = base_graph();
        g.set_node("a", node_rank(1));
        g.set_node("b", node_rank(1));
        g.set_node("c", node_rank(2));
        g.set_node("d", node_rank(3));

        let lg = build_layer_graph(&g, 1, Relationship::InEdges, None);
        let root = lg.graph().unwrap().root.clone();
        assert!(lg.has_node(&root));
        assert_eq!(lg.children(GRAPH_NODE), vec![root.clone()]);
        assert_eq!(lg.children(&root), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn copies_flat_nodes_from_layer() {
        let mut g = base_graph();
        g.set_node("a", node_rank(1));
        g.set_node("b", node_rank(1));
        g.set_node("c", node_rank(2));
        g.set_node("d", node_rank(3));

        assert!(build_layer_graph(&g, 1, Relationship::InEdges, None)
            .nodes()
            .contains(&"a".to_string()));
        assert!(build_layer_graph(&g, 1, Relationship::InEdges, None)
            .nodes()
            .contains(&"b".to_string()));
        assert!(build_layer_graph(&g, 2, Relationship::InEdges, None)
            .nodes()
            .contains(&"c".to_string()));
        assert!(build_layer_graph(&g, 3, Relationship::InEdges, None)
            .nodes()
            .contains(&"d".to_string()));
    }

    #[test]
    fn uses_original_node_order_for_copied_nodes() {
        // Port of "uses the original node label for copied nodes": the layer
        // node carries the original node's `order` snapshot.
        let mut g = base_graph();
        g.set_node(
            "a",
            NodeLabel {
                rank: Some(1),
                order: Some(1),
                ..Default::default()
            },
        );
        g.set_node(
            "b",
            NodeLabel {
                rank: Some(2),
                order: Some(2),
                ..Default::default()
            },
        );
        g.set_edge("a", "b", w(1.0), None);

        let lg = build_layer_graph(&g, 2, Relationship::InEdges, None);
        assert_eq!(lg.node("a").unwrap().order, Some(1));
        assert_eq!(lg.node("b").unwrap().order, Some(2));
    }

    #[test]
    fn copies_edges_incident_in_edges() {
        let mut g = base_graph();
        g.set_node("a", node_rank(1));
        g.set_node("b", node_rank(1));
        g.set_node("c", node_rank(2));
        g.set_node("d", node_rank(3));
        g.set_edge("a", "c", w(2.0), None);
        g.set_edge("b", "c", w(3.0), None);
        g.set_edge("c", "d", w(4.0), None);

        assert_eq!(
            build_layer_graph(&g, 1, Relationship::InEdges, None).edge_count(),
            0
        );
        let lg2 = build_layer_graph(&g, 2, Relationship::InEdges, None);
        assert_eq!(lg2.edge_count(), 2);
        assert_eq!(lg2.edge("a", "c", None).unwrap().weight, Some(2.0));
        assert_eq!(lg2.edge("b", "c", None).unwrap().weight, Some(3.0));
        let lg3 = build_layer_graph(&g, 3, Relationship::InEdges, None);
        assert_eq!(lg3.edge_count(), 1);
        assert_eq!(lg3.edge("c", "d", None).unwrap().weight, Some(4.0));
    }

    #[test]
    fn copies_edges_incident_out_edges() {
        let mut g = base_graph();
        g.set_node("a", node_rank(1));
        g.set_node("b", node_rank(1));
        g.set_node("c", node_rank(2));
        g.set_node("d", node_rank(3));
        g.set_edge("a", "c", w(2.0), None);
        g.set_edge("b", "c", w(3.0), None);
        g.set_edge("c", "d", w(4.0), None);

        let lg1 = build_layer_graph(&g, 1, Relationship::OutEdges, None);
        assert_eq!(lg1.edge_count(), 2);
        assert_eq!(lg1.edge("c", "a", None).unwrap().weight, Some(2.0));
        assert_eq!(lg1.edge("c", "b", None).unwrap().weight, Some(3.0));
        let lg2 = build_layer_graph(&g, 2, Relationship::OutEdges, None);
        assert_eq!(lg2.edge_count(), 1);
        assert_eq!(lg2.edge("d", "c", None).unwrap().weight, Some(4.0));
        assert_eq!(
            build_layer_graph(&g, 3, Relationship::OutEdges, None).edge_count(),
            0
        );
    }

    #[test]
    fn collapses_multi_edges() {
        let mut g = base_graph();
        g.set_node("a", node_rank(1));
        g.set_node("b", node_rank(2));
        g.set_edge("a", "b", w(2.0), None);
        g.set_edge("a", "b", w(3.0), Some("multi"));

        let lg = build_layer_graph(&g, 2, Relationship::InEdges, None);
        assert_eq!(lg.edge("a", "b", None).unwrap().weight, Some(5.0));
    }

    #[test]
    fn preserves_hierarchy_for_movable_layer() {
        let mut g = base_graph();
        g.set_node("a", node_rank(0));
        g.set_node("b", node_rank(0));
        g.set_node("c", node_rank(0));
        g.set_node(
            "sg",
            NodeLabel {
                min_rank: Some(0),
                max_rank: Some(0),
                border_left: Some(vec!["bl".to_string()]),
                border_right: Some(vec!["br".to_string()]),
                ..Default::default()
            },
        );
        g.set_parent("a", "sg");
        g.set_parent("b", "sg");

        let lg = build_layer_graph(&g, 0, Relationship::InEdges, None);
        let root = lg.graph().unwrap().root.clone();
        let mut kids = lg.children(&root);
        kids.sort();
        assert_eq!(kids, vec!["c".to_string(), "sg".to_string()]);
        assert_eq!(lg.parent("a"), Some("sg".to_string()));
        assert_eq!(lg.parent("b"), Some("sg".to_string()));
    }
}
