//! Recursive subgraph sort — a port of `dagre/lib/order/sort-subgraph.ts`.
//!
//! Sorts the movable children of `v` by barycenter, recursing into nested
//! subgraphs and merging their aggregate barycenters, resolving constraint
//! conflicts, then flattening. Border nodes (`borderLeft`/`borderRight`) are
//! pinned to the extremes and contribute a barycenter from their predecessors'
//! orders.

use super::barycenter::{barycenter, BarycenterEntry};
use super::build_layer_graph::LayerGraph;
use super::resolve_conflicts::{resolve_conflicts, ResolvedEntry};
use super::sort::{sort, SortResult};

/// `sortSubgraph(graph, v, constraintGraph, biasRight)`.
pub fn sort_subgraph<CgG, CgN, CgE>(
    graph: &LayerGraph,
    v: &str,
    constraint_graph: &super::graph::Graph<CgG, CgN, CgE>,
    bias_right: bool,
) -> SortResult {
    use std::collections::HashMap;

    let mut movable = graph.children(v);
    let node = graph.node(v);
    let bl: Option<String> = node.and_then(|n| n.border_left.clone());
    let br: Option<String> = node.and_then(|n| n.border_right.clone());
    let mut subgraphs: HashMap<String, SortResult> = HashMap::new();

    if let Some(bl) = &bl {
        movable.retain(|w| w != bl && Some(w) != br.as_ref());
    }

    let mut barycenters: Vec<BarycenterEntry> = barycenter(graph, &movable);
    for entry in barycenters.iter_mut() {
        if !graph.children(&entry.v).is_empty() {
            let subgraph_result = sort_subgraph(graph, &entry.v, constraint_graph, bias_right);
            if subgraph_result.barycenter.is_some() {
                merge_barycenters(entry, &subgraph_result);
            }
            subgraphs.insert(entry.v.clone(), subgraph_result);
        }
    }

    let mut entries: Vec<ResolvedEntry> = resolve_conflicts(&barycenters, constraint_graph);
    expand_subgraphs(&mut entries, &subgraphs);

    let mut result = sort(entries, bias_right);

    if let (Some(bl), Some(br)) = (&bl, &br) {
        let mut new_vs = vec![bl.clone()];
        new_vs.extend(result.vs.clone());
        new_vs.push(br.clone());
        result.vs = new_vs;

        let bl_predecessors = graph.predecessors(bl).unwrap_or_default();
        if !bl_predecessors.is_empty() {
            let bl_pred_order = order_of(graph, &bl_predecessors[0]);
            let br_predecessors = graph.predecessors(br).unwrap_or_default();
            let br_pred_order = order_of(graph, &br_predecessors[0]);

            if result.barycenter.is_none() {
                result.barycenter = Some(0.0);
                result.weight = Some(0.0);
            }
            let bc = result.barycenter.unwrap();
            let wt = result.weight.unwrap();
            result.barycenter =
                Some((bc * wt + bl_pred_order + br_pred_order) / (wt + 2.0));
            result.weight = Some(wt + 2.0);
        }
    }

    result
}

fn order_of(graph: &LayerGraph, v: &str) -> f64 {
    graph.node(v).and_then(|n| n.order).unwrap_or(0) as f64
}

fn expand_subgraphs(entries: &mut [ResolvedEntry], subgraphs: &std::collections::HashMap<String, SortResult>) {
    for entry in entries.iter_mut() {
        let mut expanded: Vec<String> = Vec::new();
        for v in &entry.vs {
            match subgraphs.get(v) {
                Some(sub) => expanded.extend(sub.vs.clone()),
                None => expanded.push(v.clone()),
            }
        }
        entry.vs = expanded;
    }
}

fn merge_barycenters(target: &mut BarycenterEntry, other: &SortResult) {
    match target.barycenter {
        Some(tb) => {
            let tw = target.weight.unwrap();
            let ob = other.barycenter.unwrap();
            let ow = other.weight.unwrap();
            target.barycenter = Some((tb * tw + ob * ow) / (tw + ow));
            target.weight = Some(tw + ow);
        }
        None => {
            target.barycenter = other.barycenter;
            target.weight = other.weight;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layered::graph::{Graph, GraphOptions};
    use crate::layered::order::{new_constraint_graph, OrderEdge, OrderNode};

    // Build a compound graph matching the TS sort-subgraph tests: default node
    // label {} (order None), default edge label {weight: 1}.
    fn g_setup() -> LayerGraph {
        let mut g: LayerGraph = Graph::new(GraphOptions {
            directed: true,
            multigraph: false,
            compound: true,
        });
        g.set_default_node_label(OrderNode::default());
        g.set_default_edge_label(OrderEdge { weight: Some(1.0) });
        for (i, v) in ["0", "1", "2", "3", "4"].iter().enumerate() {
            g.set_node(*v, OrderNode::with_order(i));
        }
        g
    }

    fn w(weight: f64) -> OrderEdge {
        OrderEdge {
            weight: Some(weight),
        }
    }

    fn vs(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn sorts_flat_subgraph_based_on_barycenter() {
        let mut g = g_setup();
        g.ensure_edge("3", "x", None);
        g.set_edge("1", "y", w(2.0), None);
        g.ensure_edge("4", "y", None);
        g.set_parent("x", "movable");
        g.set_parent("y", "movable");
        let cg = new_constraint_graph();
        assert_eq!(sort_subgraph(&g, "movable", &cg, false).vs, vs(&["y", "x"]));
    }

    #[test]
    fn preserves_pos_of_node_without_neighbors() {
        let mut g = g_setup();
        g.ensure_edge("3", "x", None);
        g.ensure_node("y");
        g.set_edge("1", "z", w(2.0), None);
        g.ensure_edge("4", "z", None);
        for v in ["x", "y", "z"] {
            g.set_parent(v, "movable");
        }
        let cg = new_constraint_graph();
        assert_eq!(
            sort_subgraph(&g, "movable", &cg, false).vs,
            vs(&["z", "y", "x"])
        );
    }

    #[test]
    fn biases_left_without_reverse_bias() {
        let mut g = g_setup();
        g.ensure_edge("1", "x", None);
        g.ensure_edge("1", "y", None);
        for v in ["x", "y"] {
            g.set_parent(v, "movable");
        }
        let cg = new_constraint_graph();
        assert_eq!(
            sort_subgraph(&g, "movable", &cg, false).vs,
            vs(&["x", "y"])
        );
    }

    #[test]
    fn biases_right_with_reverse_bias() {
        let mut g = g_setup();
        g.ensure_edge("1", "x", None);
        g.ensure_edge("1", "y", None);
        for v in ["x", "y"] {
            g.set_parent(v, "movable");
        }
        let cg = new_constraint_graph();
        assert_eq!(
            sort_subgraph(&g, "movable", &cg, true).vs,
            vs(&["y", "x"])
        );
    }

    #[test]
    fn aggregates_stats_about_subgraph() {
        let mut g = g_setup();
        g.ensure_edge("3", "x", None);
        g.set_edge("1", "y", w(2.0), None);
        g.ensure_edge("4", "y", None);
        for v in ["x", "y"] {
            g.set_parent(v, "movable");
        }
        let cg = new_constraint_graph();
        let results = sort_subgraph(&g, "movable", &cg, false);
        assert_eq!(results.barycenter, Some(2.25));
        assert_eq!(results.weight, Some(4.0));
    }

    #[test]
    fn can_sort_nested_subgraph_with_no_barycenter() {
        let mut g = g_setup();
        for v in ["a", "b", "c"] {
            g.ensure_node(v);
        }
        g.set_parent("a", "y");
        g.set_parent("b", "y");
        g.set_parent("c", "y");
        g.ensure_edge("0", "x", None);
        g.ensure_edge("1", "z", None);
        g.ensure_edge("2", "y", None);
        for v in ["x", "y", "z"] {
            g.set_parent(v, "movable");
        }
        let cg = new_constraint_graph();
        assert_eq!(
            sort_subgraph(&g, "movable", &cg, false).vs,
            vs(&["x", "z", "a", "b", "c"])
        );
    }

    #[test]
    fn can_sort_nested_subgraph_with_barycenter() {
        let mut g = g_setup();
        for v in ["a", "b", "c"] {
            g.ensure_node(v);
        }
        g.set_parent("a", "y");
        g.set_parent("b", "y");
        g.set_parent("c", "y");
        g.set_edge("0", "a", w(3.0), None);
        g.ensure_edge("0", "x", None);
        g.ensure_edge("1", "z", None);
        g.ensure_edge("2", "y", None);
        for v in ["x", "y", "z"] {
            g.set_parent(v, "movable");
        }
        let cg = new_constraint_graph();
        assert_eq!(
            sort_subgraph(&g, "movable", &cg, false).vs,
            vs(&["x", "a", "b", "c", "z"])
        );
    }

    #[test]
    fn can_sort_nested_subgraph_with_no_in_edges() {
        let mut g = g_setup();
        for v in ["a", "b", "c"] {
            g.ensure_node(v);
        }
        g.set_parent("a", "y");
        g.set_parent("b", "y");
        g.set_parent("c", "y");
        g.ensure_edge("0", "a", None);
        g.ensure_edge("1", "b", None);
        g.ensure_edge("0", "x", None);
        g.ensure_edge("1", "z", None);
        for v in ["x", "y", "z"] {
            g.set_parent(v, "movable");
        }
        let cg = new_constraint_graph();
        assert_eq!(
            sort_subgraph(&g, "movable", &cg, false).vs,
            vs(&["x", "a", "b", "c", "z"])
        );
    }

    #[test]
    fn sorts_border_nodes_to_extremes() {
        let mut g = g_setup();
        g.ensure_edge("0", "x", None);
        g.ensure_edge("1", "y", None);
        g.ensure_edge("2", "z", None);
        g.set_node(
            "sg1",
            OrderNode {
                order: None,
                border_left: Some("bl".to_string()),
                border_right: Some("br".to_string()),
            },
        );
        for v in ["x", "y", "z", "bl", "br"] {
            g.set_parent(v, "sg1");
        }
        let cg = new_constraint_graph();
        assert_eq!(
            sort_subgraph(&g, "sg1", &cg, false).vs,
            vs(&["bl", "x", "y", "z", "br"])
        );
    }

    #[test]
    fn assigns_barycenter_to_subgraph_based_on_previous_border_nodes() {
        let mut g = g_setup();
        g.set_node("bl1", OrderNode::with_order(0));
        g.set_node("br1", OrderNode::with_order(1));
        g.ensure_edge("bl1", "bl2", None);
        g.ensure_edge("br1", "br2", None);
        g.set_parent("bl2", "sg");
        g.set_parent("br2", "sg");
        g.set_node(
            "sg",
            OrderNode {
                order: None,
                border_left: Some("bl2".to_string()),
                border_right: Some("br2".to_string()),
            },
        );
        let cg = new_constraint_graph();
        let result = sort_subgraph(&g, "sg", &cg, false);
        assert_eq!(result.barycenter, Some(0.5));
        assert_eq!(result.weight, Some(2.0));
        assert_eq!(result.vs, vs(&["bl2", "br2"]));
    }
}
