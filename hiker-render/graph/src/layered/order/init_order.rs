//! Initial DFS ordering — a port of `dagre/lib/order/init-order.ts`.
//!
//! Assigns an initial per-rank order by DFS from the lowest-ranked simple
//! (leaf-in-the-compound-tree) nodes. Returns a layering matrix (array per
//! rank, each in visit order). From Gansner, et al., "A Technique for Drawing
//! Directed Graphs."

use super::graph::Graph;
use super::util;
use crate::layered::types::{EdgeLabel, GraphLabel, NodeLabel};

/// `initOrder(graph)`.
pub fn init_order(graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>) -> Vec<Vec<String>> {
    use std::collections::HashSet;

    let simple_nodes: Vec<String> = graph
        .nodes()
        .into_iter()
        .filter(|v| graph.children(v).is_empty())
        .collect();
    let simple_ranks: Vec<i32> = simple_nodes
        .iter()
        .map(|v| graph.node(v).and_then(|n| n.rank).unwrap_or(0))
        .collect();
    let max_rank = util::apply_with_chunking_max(&simple_ranks);

    let layer_count = if max_rank < 0 { 0 } else { (max_rank + 1) as usize };
    let mut layers: Vec<Vec<String>> = vec![Vec::new(); layer_count];

    let mut visited: HashSet<String> = HashSet::new();

    // orderedVs = simpleNodes sorted by rank (stable).
    let mut ordered_vs = simple_nodes.clone();
    ordered_vs.sort_by(|a, b| {
        let ra = graph.node(a).and_then(|n| n.rank).unwrap_or(0);
        let rb = graph.node(b).and_then(|n| n.rank).unwrap_or(0);
        ra.cmp(&rb)
    });

    // Iterative DFS preserving the recursive pre-order visit order.
    for start in &ordered_vs {
        if visited.contains(start) {
            continue;
        }
        let mut stack: Vec<String> = vec![start.clone()];
        while let Some(v) = stack.pop() {
            if visited.contains(&v) {
                continue;
            }
            visited.insert(v.clone());
            if let Some(node) = graph.node(&v) {
                if let Some(rank) = node.rank {
                    if rank >= 0 {
                        layers[rank as usize].push(v.clone());
                    }
                }
            }
            // successors().forEach(dfs): push in reverse so they pop in order.
            if let Some(succs) = graph.successors(&v) {
                for s in succs.into_iter().rev() {
                    if !visited.contains(&s) {
                        stack.push(s);
                    }
                }
            }
        }
    }

    layers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layered::graph::GraphOptions;

    fn g_compound() -> Graph<GraphLabel, NodeLabel, EdgeLabel> {
        let mut g = Graph::new(GraphOptions {
            directed: true,
            multigraph: false,
            compound: true,
        });
        g.set_default_edge_label(EdgeLabel {
            weight: Some(1.0),
            ..Default::default()
        });
        g
    }

    fn node_rank(r: i32) -> NodeLabel {
        NodeLabel {
            rank: Some(r),
            ..Default::default()
        }
    }

    #[test]
    fn non_overlapping_orders_in_tree() {
        let mut g = g_compound();
        for (v, r) in [("a", 0), ("b", 1), ("c", 2), ("d", 2), ("e", 1)] {
            g.set_node(v, node_rank(r));
        }
        g.set_path(&["a", "b", "c"], EdgeLabel::default());
        g.set_edge("b", "d", EdgeLabel::default(), None);
        g.set_edge("a", "e", EdgeLabel::default(), None);

        let layering = init_order(&g);
        assert_eq!(layering[0], vec!["a".to_string()]);
        let mut l1 = layering[1].clone();
        l1.sort();
        assert_eq!(l1, vec!["b".to_string(), "e".to_string()]);
        let mut l2 = layering[2].clone();
        l2.sort();
        assert_eq!(l2, vec!["c".to_string(), "d".to_string()]);
    }

    #[test]
    fn non_overlapping_orders_in_dag() {
        let mut g = g_compound();
        for (v, r) in [("a", 0), ("b", 1), ("c", 1), ("d", 2)] {
            g.set_node(v, node_rank(r));
        }
        g.set_path(&["a", "b", "d"], EdgeLabel::default());
        g.set_path(&["a", "c", "d"], EdgeLabel::default());

        let layering = init_order(&g);
        assert_eq!(layering[0], vec!["a".to_string()]);
        let mut l1 = layering[1].clone();
        l1.sort();
        assert_eq!(l1, vec!["b".to_string(), "c".to_string()]);
        let mut l2 = layering[2].clone();
        l2.sort();
        assert_eq!(l2, vec!["d".to_string()]);
    }

    #[test]
    fn does_not_assign_order_to_subgraph_nodes() {
        let mut g = g_compound();
        g.set_node("a", node_rank(0));
        g.set_node("sg1", NodeLabel::default());
        g.set_parent("a", "sg1");

        let layering = init_order(&g);
        assert_eq!(layering, vec![vec!["a".to_string()]]);
    }
}
