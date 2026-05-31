//! Barycenter computation — a port of `dagre/lib/order/barycenter.ts`.
//!
//! For each movable node, computes the weighted average (`barycenter`) of its
//! in-neighbours' `order`s, weighted by edge weight, plus the total `weight`.
//! A node with no in-edges gets no barycenter (`None`), mirroring the TS
//! `{v: v}` (no `barycenter`/`weight` keys).

use super::graph::Graph;
use super::{OrderEdge, OrderNode};

/// One barycenter entry — TS `{v, barycenter?, weight?}`.
#[derive(Clone, Debug, PartialEq)]
pub struct BarycenterEntry {
    pub v: String,
    pub barycenter: Option<f64>,
    pub weight: Option<f64>,
}

/// `barycenter(graph, movable)`.
pub fn barycenter<G>(
    graph: &Graph<G, OrderNode, OrderEdge>,
    movable: &[String],
) -> Vec<BarycenterEntry> {
    movable
        .iter()
        .map(|v| {
            let in_v = graph.in_edges(v, None).unwrap_or_default();
            if in_v.is_empty() {
                BarycenterEntry {
                    v: v.clone(),
                    barycenter: None,
                    weight: None,
                }
            } else {
                let mut sum = 0.0_f64;
                let mut weight = 0.0_f64;
                for e in &in_v {
                    let edge_w = graph
                        .edge_by_obj(e)
                        .and_then(|l| l.weight)
                        .unwrap_or(0.0);
                    let order_u = graph
                        .node(&e.v)
                        .and_then(|n| n.order)
                        .unwrap_or(0) as f64;
                    sum += edge_w * order_u;
                    weight += edge_w;
                }
                BarycenterEntry {
                    v: v.clone(),
                    barycenter: Some(sum / weight),
                    weight: Some(weight),
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::{new_order_graph, OrderEdge, OrderNode};
    use super::*;

    fn set_order(g: &mut Graph<(), OrderNode, OrderEdge>, v: &str, order: usize) {
        g.set_node(v, OrderNode::with_order(order));
    }

    #[test]
    fn undefined_barycenter_for_node_with_no_predecessors() {
        let mut g = new_order_graph(false);
        g.set_node("x", OrderNode::default());
        let results = barycenter(&g, &["x".to_string()]);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            BarycenterEntry {
                v: "x".into(),
                barycenter: None,
                weight: None
            }
        );
    }

    #[test]
    fn assigns_position_of_sole_predecessor() {
        let mut g = new_order_graph(false);
        set_order(&mut g, "a", 2);
        g.set_edge("a", "x", OrderEdge { weight: Some(1.0) }, None);
        let results = barycenter(&g, &["x".to_string()]);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            BarycenterEntry {
                v: "x".into(),
                barycenter: Some(2.0),
                weight: Some(1.0)
            }
        );
    }

    #[test]
    fn assigns_average_of_multiple_predecessors() {
        let mut g = new_order_graph(false);
        set_order(&mut g, "a", 2);
        set_order(&mut g, "b", 4);
        g.set_edge("a", "x", OrderEdge { weight: Some(1.0) }, None);
        g.set_edge("b", "x", OrderEdge { weight: Some(1.0) }, None);
        let results = barycenter(&g, &["x".to_string()]);
        assert_eq!(
            results[0],
            BarycenterEntry {
                v: "x".into(),
                barycenter: Some(3.0),
                weight: Some(2.0)
            }
        );
    }

    #[test]
    fn takes_into_account_weight_of_edges() {
        let mut g = new_order_graph(false);
        set_order(&mut g, "a", 2);
        set_order(&mut g, "b", 4);
        g.set_edge("a", "x", OrderEdge { weight: Some(3.0) }, None);
        g.set_edge("b", "x", OrderEdge { weight: Some(1.0) }, None);
        let results = barycenter(&g, &["x".to_string()]);
        assert_eq!(
            results[0],
            BarycenterEntry {
                v: "x".into(),
                barycenter: Some(2.5),
                weight: Some(4.0)
            }
        );
    }

    #[test]
    fn calculates_barycenters_for_all_nodes_in_movable_layer() {
        let mut g = new_order_graph(false);
        set_order(&mut g, "a", 1);
        set_order(&mut g, "b", 2);
        set_order(&mut g, "c", 4);
        g.set_edge("a", "x", OrderEdge { weight: Some(1.0) }, None);
        g.set_edge("b", "x", OrderEdge { weight: Some(1.0) }, None);
        g.set_node("y", OrderNode::default());
        g.set_edge("a", "z", OrderEdge { weight: Some(2.0) }, None);
        g.set_edge("c", "z", OrderEdge { weight: Some(1.0) }, None);
        let results = barycenter(
            &g,
            &["x".to_string(), "y".to_string(), "z".to_string()],
        );
        assert_eq!(results.len(), 3);
        assert_eq!(
            results[0],
            BarycenterEntry {
                v: "x".into(),
                barycenter: Some(1.5),
                weight: Some(2.0)
            }
        );
        assert_eq!(
            results[1],
            BarycenterEntry {
                v: "y".into(),
                barycenter: None,
                weight: None
            }
        );
        assert_eq!(
            results[2],
            BarycenterEntry {
                v: "z".into(),
                barycenter: Some(2.0),
                weight: Some(3.0)
            }
        );
    }
}
