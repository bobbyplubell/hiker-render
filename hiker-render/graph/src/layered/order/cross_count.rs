//! Bilayer crossing count — a port of `dagre/lib/order/cross-count.ts`.
//!
//! Derived from Barth, et al., "Bilayer Cross Counting". Sums weighted
//! crossings between every adjacent pair of layers using the accumulator-tree
//! algorithm. The graph and layering are left unchanged.

use super::graph::Graph;
use super::HasWeight;

/// `crossCount(graph, layering)`.
pub fn cross_count<G, N, E: HasWeight>(graph: &Graph<G, N, E>, layering: &[Vec<String>]) -> i64 {
    let mut cc = 0_i64;
    for i in 1..layering.len() {
        cc += two_layer_cross_count(graph, &layering[i - 1], &layering[i]);
    }
    cc
}

struct SouthEntry {
    pos: usize,
    weight: f64,
}

fn two_layer_cross_count<G, N, E: HasWeight>(
    graph: &Graph<G, N, E>,
    north_layer: &[String],
    south_layer: &[String],
) -> i64 {
    // south node id -> its position in the south layer.
    use std::collections::HashMap;
    let south_pos: HashMap<&str, usize> = south_layer
        .iter()
        .enumerate()
        .map(|(i, v)| (v.as_str(), i))
        .collect();

    // For each north node (in order), its out-edges mapped to south positions,
    // each group sorted by south position.
    let mut south_entries: Vec<SouthEntry> = Vec::new();
    for v in north_layer {
        let edges = graph.out_edges(v, None).unwrap_or_default();
        let mut group: Vec<SouthEntry> = edges
            .iter()
            .map(|e| SouthEntry {
                pos: south_pos[e.w.as_str()],
                weight: graph.edge_by_obj(e).map(|l| l.weight()).unwrap_or(0.0),
            })
            .collect();
        group.sort_by(|a, b| a.pos.cmp(&b.pos));
        south_entries.extend(group);
    }

    // Build the accumulator tree.
    let mut first_index = 1_usize;
    while first_index < south_layer.len() {
        first_index <<= 1;
    }
    let tree_size = 2 * first_index - 1;
    first_index -= 1;
    let mut tree = vec![0.0_f64; tree_size];

    // Calculate the weighted crossings.
    let mut cc = 0.0_f64;
    for entry in &south_entries {
        let mut index = entry.pos + first_index;
        tree[index] += entry.weight;
        let mut weight_sum = 0.0_f64;
        while index > 0 {
            if index % 2 == 1 {
                weight_sum += tree[index + 1];
            }
            index = (index - 1) >> 1;
            tree[index] += entry.weight;
        }
        cc += entry.weight * weight_sum;
    }

    cc as i64
}

#[cfg(test)]
mod tests {
    use super::super::{new_order_graph, OrderEdge};
    use super::*;

    fn layers(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|r| r.iter().map(|s| s.to_string()).collect())
            .collect()
    }

    fn w(weight: f64) -> OrderEdge {
        OrderEdge {
            weight: Some(weight),
        }
    }

    #[test]
    fn returns_0_for_empty_layering() {
        let g = new_order_graph(false);
        assert_eq!(cross_count(&g, &[]), 0);
    }

    #[test]
    fn returns_0_for_layering_with_no_crossings() {
        let mut g = new_order_graph(false);
        g.set_edge("a1", "b1", w(1.0), None);
        g.set_edge("a2", "b2", w(1.0), None);
        assert_eq!(
            cross_count(&g, &layers(&[&["a1", "a2"], &["b1", "b2"]])),
            0
        );
    }

    #[test]
    fn returns_1_for_layering_with_1_crossing() {
        let mut g = new_order_graph(false);
        g.set_edge("a1", "b1", w(1.0), None);
        g.set_edge("a2", "b2", w(1.0), None);
        assert_eq!(
            cross_count(&g, &layers(&[&["a1", "a2"], &["b2", "b1"]])),
            1
        );
    }

    #[test]
    fn returns_weighted_crossing_count_for_1_crossing() {
        let mut g = new_order_graph(false);
        g.set_edge("a1", "b1", w(2.0), None);
        g.set_edge("a2", "b2", w(3.0), None);
        assert_eq!(
            cross_count(&g, &layers(&[&["a1", "a2"], &["b2", "b1"]])),
            6
        );
    }

    #[test]
    fn calculates_crossings_across_layers() {
        let mut g = new_order_graph(false);
        g.set_path(&["a1", "b1", "c1"], w(1.0));
        g.set_path(&["a2", "b2", "c2"], w(1.0));
        assert_eq!(
            cross_count(
                &g,
                &layers(&[&["a1", "a2"], &["b2", "b1"], &["c1", "c2"]])
            ),
            2
        );
    }

    #[test]
    fn works_for_graph_1() {
        let mut g = new_order_graph(false);
        g.set_path(&["a", "b", "c"], w(1.0));
        g.set_path(&["d", "e", "c"], w(1.0));
        g.set_path(&["a", "f", "i"], w(1.0));
        g.set_edge("a", "e", w(1.0), None);
        assert_eq!(
            cross_count(
                &g,
                &layers(&[&["a", "d"], &["b", "e", "f"], &["c", "i"]])
            ),
            1
        );
        assert_eq!(
            cross_count(
                &g,
                &layers(&[&["d", "a"], &["e", "b", "f"], &["c", "i"]])
            ),
            0
        );
    }
}
