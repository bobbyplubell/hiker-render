//! Port of `dagre/test/position/bk-test.ts` and `dagre/test/position-test.ts`.
//!
//! Test names mirror the TS so a reviewer can diff against the original oracle.
//! The bk tests reach directly into the module-internal functions
//! (`findType1Conflicts`, `verticalAlignment`, `horizontalCompaction`, …),
//! exposed here as `pub(crate)` on [`super::bk`].

use std::collections::HashMap;

use super::bk::{
    add_conflict, align_coordinates, balance, find_smallest_width_alignment,
    find_type1_conflicts, find_type2_conflicts, has_conflict, horizontal_compaction, position_x,
    vertical_alignment, AlignmentResult, Conflicts, Xss,
};
use super::position;
use crate::layered::graph::{Graph, GraphOptions, NodeId};
use crate::layered::types::{
    DagreGraph, DummyKind, EdgeLabel, GraphLabel, LabelPos, NodeLabel,
};
use crate::layered::util::build_layer_matrix;

// ── helpers ──────────────────────────────────────────────────────────────────

fn new_graph() -> DagreGraph {
    let mut g: DagreGraph = Graph::new(GraphOptions::default());
    g.set_graph(GraphLabel::default());
    g
}

fn map_str(pairs: &[(&str, &str)]) -> HashMap<NodeId, NodeId> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn map_f64(pairs: &[(&str, f64)]) -> HashMap<NodeId, f64> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn get(m: &HashMap<NodeId, f64>, k: &str) -> f64 {
    *m.get(k).unwrap()
}

/// Set the `dummy` flag on an existing node (mirrors `g.node(v).dummy = ...`).
fn set_dummy(g: &mut DagreGraph, v: &str, kind: DummyKind) {
    g.node_mut(v).unwrap().dummy = Some(kind);
}

// ── findType1Conflicts ───────────────────────────────────────────────────────

mod find_type1_conflicts_tests {
    use super::*;

    fn setup() -> (DagreGraph, Vec<Vec<NodeId>>) {
        let mut g = new_graph();
        g.set_default_edge_label(EdgeLabel::default());
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), ..Default::default() });
        g.set_node("d", NodeLabel { rank: Some(1), order: Some(1), ..Default::default() });
        g.ensure_edge("a", "d", None);
        g.ensure_edge("b", "c", None);
        let layering = build_layer_matrix(&g);
        (g, layering)
    }

    #[test]
    fn does_not_mark_edges_that_have_no_conflict() {
        let (mut g, layering) = setup();
        g.remove_edge("a", "d", None);
        g.remove_edge("b", "c", None);
        g.ensure_edge("a", "c", None);
        g.ensure_edge("b", "d", None);

        let conflicts = find_type1_conflicts(&g, &layering);
        assert!(!has_conflict(&conflicts, "a", "c"));
        assert!(!has_conflict(&conflicts, "b", "d"));
    }

    #[test]
    fn does_not_mark_type0_conflicts_no_dummies() {
        let (g, layering) = setup();
        let conflicts = find_type1_conflicts(&g, &layering);
        assert!(!has_conflict(&conflicts, "a", "d"));
        assert!(!has_conflict(&conflicts, "b", "c"));
    }

    #[test]
    fn does_not_mark_type0_conflicts_single_dummy() {
        for v in ["a", "b", "c", "d"] {
            let (mut g, layering) = setup();
            set_dummy(&mut g, v, DummyKind::Edge);
            let conflicts = find_type1_conflicts(&g, &layering);
            assert!(!has_conflict(&conflicts, "a", "d"), "{v}");
            assert!(!has_conflict(&conflicts, "b", "c"), "{v}");
        }
    }

    #[test]
    fn does_mark_type1_conflicts_non_dummy() {
        for v in ["a", "b", "c", "d"] {
            let (mut g, layering) = setup();
            for w in ["a", "b", "c", "d"] {
                if v != w {
                    set_dummy(&mut g, w, DummyKind::Edge);
                }
            }
            let conflicts = find_type1_conflicts(&g, &layering);
            if v == "a" || v == "d" {
                assert!(has_conflict(&conflicts, "a", "d"), "{v}");
                assert!(!has_conflict(&conflicts, "b", "c"), "{v}");
            } else {
                assert!(!has_conflict(&conflicts, "a", "d"), "{v}");
                assert!(has_conflict(&conflicts, "b", "c"), "{v}");
            }
        }
    }

    #[test]
    fn does_not_mark_type2_conflicts_all_dummies() {
        let (mut g, layering) = setup();
        for v in ["a", "b", "c", "d"] {
            set_dummy(&mut g, v, DummyKind::Edge);
        }
        let conflicts = find_type1_conflicts(&g, &layering);
        assert!(!has_conflict(&conflicts, "a", "d"));
        assert!(!has_conflict(&conflicts, "b", "c"));
    }
}

// ── findType2Conflicts ───────────────────────────────────────────────────────

mod find_type2_conflicts_tests {
    use super::*;

    fn setup() -> (DagreGraph, Vec<Vec<NodeId>>) {
        let mut g = new_graph();
        g.set_default_edge_label(EdgeLabel::default());
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), ..Default::default() });
        g.set_node("d", NodeLabel { rank: Some(1), order: Some(1), ..Default::default() });
        g.ensure_edge("a", "d", None);
        g.ensure_edge("b", "c", None);
        let layering = build_layer_matrix(&g);
        (g, layering)
    }

    #[test]
    fn marks_type2_conflicts_favoring_border_segments_1() {
        let (mut g, layering) = setup();
        for v in ["a", "d"] {
            set_dummy(&mut g, v, DummyKind::Edge);
        }
        for v in ["b", "c"] {
            set_dummy(&mut g, v, DummyKind::Border);
        }
        let conflicts = find_type2_conflicts(&g, &layering);
        assert!(has_conflict(&conflicts, "a", "d"));
        assert!(!has_conflict(&conflicts, "b", "c"));
    }

    #[test]
    fn marks_type2_conflicts_favoring_border_segments_2() {
        let (mut g, layering) = setup();
        for v in ["b", "c"] {
            set_dummy(&mut g, v, DummyKind::Edge);
        }
        for v in ["a", "d"] {
            set_dummy(&mut g, v, DummyKind::Border);
        }
        let conflicts = find_type2_conflicts(&g, &layering);
        assert!(!has_conflict(&conflicts, "a", "d"));
        assert!(has_conflict(&conflicts, "b", "c"));
    }
}

// ── hasConflict ──────────────────────────────────────────────────────────────

mod has_conflict_tests {
    use super::*;

    #[test]
    fn type1_regardless_of_edge_orientation() {
        let mut conflicts: Conflicts = HashMap::new();
        add_conflict(&mut conflicts, "b", "a");
        assert!(has_conflict(&conflicts, "a", "b"));
        assert!(has_conflict(&conflicts, "b", "a"));
    }

    #[test]
    fn works_for_multiple_conflicts_with_same_node() {
        let mut conflicts: Conflicts = HashMap::new();
        add_conflict(&mut conflicts, "a", "b");
        add_conflict(&mut conflicts, "a", "c");
        assert!(has_conflict(&conflicts, "a", "b"));
        assert!(has_conflict(&conflicts, "a", "c"));
    }
}

// ── verticalAlignment ────────────────────────────────────────────────────────

mod vertical_alignment_tests {
    use super::*;

    fn run(g: &DagreGraph, layering: &[Vec<NodeId>], conflicts: &Conflicts) -> AlignmentResult {
        vertical_alignment(g, layering, conflicts, |v| g.predecessors(v).unwrap_or_default())
    }

    #[test]
    fn aligns_with_itself_if_no_adjacencies() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(1), order: Some(0), ..Default::default() });
        let layering = build_layer_matrix(&g);
        let result = run(&g, &layering, &HashMap::new());
        assert_eq!(result.root, map_str(&[("a", "a"), ("b", "b")]));
        assert_eq!(result.align, map_str(&[("a", "a"), ("b", "b")]));
    }

    #[test]
    fn aligns_with_sole_adjacency() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(1), order: Some(0), ..Default::default() });
        g.ensure_edge("a", "b", None);
        let layering = build_layer_matrix(&g);
        let result = run(&g, &layering, &HashMap::new());
        assert_eq!(result.root, map_str(&[("a", "a"), ("b", "a")]));
        assert_eq!(result.align, map_str(&[("a", "b"), ("b", "a")]));
    }

    #[test]
    fn aligns_with_left_median_when_possible() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), ..Default::default() });
        g.ensure_edge("a", "c", None);
        g.ensure_edge("b", "c", None);
        let layering = build_layer_matrix(&g);
        let result = run(&g, &layering, &HashMap::new());
        assert_eq!(result.root, map_str(&[("a", "a"), ("b", "b"), ("c", "a")]));
        assert_eq!(result.align, map_str(&[("a", "c"), ("b", "b"), ("c", "a")]));
    }

    #[test]
    fn aligns_correctly_regardless_of_insertion_order() {
        let mut g = new_graph();
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), ..Default::default() });
        g.set_node("z", NodeLabel { rank: Some(0), order: Some(0), ..Default::default() });
        g.ensure_edge("z", "c", None);
        g.ensure_edge("b", "c", None);
        let layering = build_layer_matrix(&g);
        let result = run(&g, &layering, &HashMap::new());
        assert_eq!(result.root, map_str(&[("z", "z"), ("b", "b"), ("c", "z")]));
        assert_eq!(result.align, map_str(&[("z", "c"), ("b", "b"), ("c", "z")]));
    }

    #[test]
    fn aligns_with_right_median_when_left_unavailable() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), ..Default::default() });
        g.ensure_edge("a", "c", None);
        g.ensure_edge("b", "c", None);
        let layering = build_layer_matrix(&g);
        let mut conflicts: Conflicts = HashMap::new();
        add_conflict(&mut conflicts, "a", "c");
        let result = run(&g, &layering, &conflicts);
        assert_eq!(result.root, map_str(&[("a", "a"), ("b", "b"), ("c", "b")]));
        assert_eq!(result.align, map_str(&[("a", "a"), ("b", "c"), ("c", "b")]));
    }

    #[test]
    fn aligns_with_neither_median_if_both_unavailable() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), ..Default::default() });
        g.set_node("d", NodeLabel { rank: Some(1), order: Some(1), ..Default::default() });
        g.ensure_edge("a", "d", None);
        g.ensure_edge("b", "c", None);
        g.ensure_edge("b", "d", None);
        let layering = build_layer_matrix(&g);
        let result = run(&g, &layering, &HashMap::new());
        assert_eq!(
            result.root,
            map_str(&[("a", "a"), ("b", "b"), ("c", "b"), ("d", "d")])
        );
        assert_eq!(
            result.align,
            map_str(&[("a", "a"), ("b", "c"), ("c", "b"), ("d", "d")])
        );
    }

    #[test]
    fn aligns_with_single_median_for_odd_adjacencies() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(0), order: Some(2), ..Default::default() });
        g.set_node("d", NodeLabel { rank: Some(1), order: Some(0), ..Default::default() });
        g.ensure_edge("a", "d", None);
        g.ensure_edge("b", "d", None);
        g.ensure_edge("c", "d", None);
        let layering = build_layer_matrix(&g);
        let result = run(&g, &layering, &HashMap::new());
        assert_eq!(
            result.root,
            map_str(&[("a", "a"), ("b", "b"), ("c", "c"), ("d", "b")])
        );
        assert_eq!(
            result.align,
            map_str(&[("a", "a"), ("b", "d"), ("c", "c"), ("d", "b")])
        );
    }

    #[test]
    fn aligns_blocks_across_multiple_layers() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(1), order: Some(0), ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(1), ..Default::default() });
        g.set_node("d", NodeLabel { rank: Some(2), order: Some(0), ..Default::default() });
        g.ensure_path(&["a", "b", "d"]);
        g.ensure_path(&["a", "c", "d"]);
        let layering = build_layer_matrix(&g);
        let result = run(&g, &layering, &HashMap::new());
        assert_eq!(
            result.root,
            map_str(&[("a", "a"), ("b", "a"), ("c", "c"), ("d", "a")])
        );
        assert_eq!(
            result.align,
            map_str(&[("a", "b"), ("b", "d"), ("c", "c"), ("d", "a")])
        );
    }
}

// ── horizontalCompaction ─────────────────────────────────────────────────────

mod horizontal_compaction_tests {
    use super::*;

    #[test]
    fn places_single_node_at_origin() {
        let mut g = new_graph();
        let root = map_str(&[("a", "a")]);
        let align = map_str(&[("a", "a")]);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), 0.0);
    }

    #[test]
    fn separates_adjacent_nodes_by_node_separation() {
        let mut g = new_graph();
        let root = map_str(&[("a", "a"), ("b", "b")]);
        let align = map_str(&[("a", "a"), ("b", "b")]);
        g.graph_mut().unwrap().nodesep = Some(100.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 100.0, ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), width: 200.0, ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), 0.0);
        assert_eq!(get(&xs, "b"), 100.0 / 2.0 + 100.0 + 200.0 / 2.0);
    }

    #[test]
    fn separates_adjacent_edges_by_edge_separation() {
        let mut g = new_graph();
        let root = map_str(&[("a", "a"), ("b", "b")]);
        let align = map_str(&[("a", "a"), ("b", "b")]);
        g.graph_mut().unwrap().edgesep = Some(20.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 100.0, dummy: Some(DummyKind::Edge), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), width: 200.0, dummy: Some(DummyKind::Edge), ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), 0.0);
        assert_eq!(get(&xs, "b"), 100.0 / 2.0 + 20.0 + 200.0 / 2.0);
    }

    #[test]
    fn aligns_centers_of_nodes_in_same_block() {
        let mut g = new_graph();
        let root = map_str(&[("a", "a"), ("b", "a")]);
        let align = map_str(&[("a", "b"), ("b", "a")]);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 100.0, ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(1), order: Some(0), width: 200.0, ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), 0.0);
        assert_eq!(get(&xs, "b"), 0.0);
    }

    #[test]
    fn separates_blocks_with_appropriate_separation() {
        let mut g = new_graph();
        let root = map_str(&[("a", "a"), ("b", "a"), ("c", "c")]);
        let align = map_str(&[("a", "b"), ("b", "a"), ("c", "c")]);
        g.graph_mut().unwrap().nodesep = Some(75.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 100.0, ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(1), order: Some(1), width: 200.0, ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), width: 50.0, ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), 50.0 / 2.0 + 75.0 + 200.0 / 2.0);
        assert_eq!(get(&xs, "b"), 50.0 / 2.0 + 75.0 + 200.0 / 2.0);
        assert_eq!(get(&xs, "c"), 0.0);
    }

    #[test]
    fn separates_classes_with_appropriate_separation() {
        let mut g = new_graph();
        let root = map_str(&[("a", "a"), ("b", "b"), ("c", "c"), ("d", "b")]);
        let align = map_str(&[("a", "a"), ("b", "d"), ("c", "c"), ("d", "b")]);
        g.graph_mut().unwrap().nodesep = Some(75.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 100.0, ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), width: 200.0, ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), width: 50.0, ..Default::default() });
        g.set_node("d", NodeLabel { rank: Some(1), order: Some(1), width: 80.0, ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), 0.0);
        assert_eq!(get(&xs, "b"), 100.0 / 2.0 + 75.0 + 200.0 / 2.0);
        assert_eq!(get(&xs, "c"), 100.0 / 2.0 + 75.0 + 200.0 / 2.0 - 80.0 / 2.0 - 75.0 - 50.0 / 2.0);
        assert_eq!(get(&xs, "d"), 100.0 / 2.0 + 75.0 + 200.0 / 2.0);
    }

    #[test]
    fn shifts_classes_by_max_sep_from_adjacent_block_1() {
        let mut g = new_graph();
        let root = map_str(&[("a", "a"), ("b", "b"), ("c", "a"), ("d", "b")]);
        let align = map_str(&[("a", "c"), ("b", "d"), ("c", "a"), ("d", "b")]);
        g.graph_mut().unwrap().nodesep = Some(75.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 50.0, ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), width: 150.0, ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), width: 60.0, ..Default::default() });
        g.set_node("d", NodeLabel { rank: Some(1), order: Some(1), width: 70.0, ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), 0.0);
        assert_eq!(get(&xs, "b"), 50.0 / 2.0 + 75.0 + 150.0 / 2.0);
        assert_eq!(get(&xs, "c"), 0.0);
        assert_eq!(get(&xs, "d"), 50.0 / 2.0 + 75.0 + 150.0 / 2.0);
    }

    #[test]
    fn shifts_classes_by_max_sep_from_adjacent_block_2() {
        let mut g = new_graph();
        let root = map_str(&[("a", "a"), ("b", "b"), ("c", "a"), ("d", "b")]);
        let align = map_str(&[("a", "c"), ("b", "d"), ("c", "a"), ("d", "b")]);
        g.graph_mut().unwrap().nodesep = Some(75.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 50.0, ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), width: 70.0, ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), width: 60.0, ..Default::default() });
        g.set_node("d", NodeLabel { rank: Some(1), order: Some(1), width: 150.0, ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), 0.0);
        assert_eq!(get(&xs, "b"), 60.0 / 2.0 + 75.0 + 150.0 / 2.0);
        assert_eq!(get(&xs, "c"), 0.0);
        assert_eq!(get(&xs, "d"), 60.0 / 2.0 + 75.0 + 150.0 / 2.0);
    }

    #[test]
    fn cascades_class_shift() {
        let mut g = new_graph();
        let root = map_str(&[
            ("a", "a"), ("b", "b"), ("c", "c"), ("d", "d"), ("e", "b"), ("f", "f"), ("g", "d"),
        ]);
        let align = map_str(&[
            ("a", "a"), ("b", "e"), ("c", "c"), ("d", "g"), ("e", "b"), ("f", "f"), ("g", "d"),
        ]);
        g.graph_mut().unwrap().nodesep = Some(75.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 50.0, ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), width: 50.0, ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), width: 50.0, ..Default::default() });
        g.set_node("d", NodeLabel { rank: Some(1), order: Some(1), width: 50.0, ..Default::default() });
        g.set_node("e", NodeLabel { rank: Some(1), order: Some(2), width: 50.0, ..Default::default() });
        g.set_node("f", NodeLabel { rank: Some(2), order: Some(0), width: 50.0, ..Default::default() });
        g.set_node("g", NodeLabel { rank: Some(2), order: Some(1), width: 50.0, ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), get(&xs, "b") - 50.0 / 2.0 - 75.0 - 50.0 / 2.0);
        assert_eq!(get(&xs, "b"), get(&xs, "e"));
        assert_eq!(get(&xs, "c"), get(&xs, "f"));
        assert_eq!(get(&xs, "d"), get(&xs, "c") + 50.0 / 2.0 + 75.0 + 50.0 / 2.0);
        assert_eq!(get(&xs, "e"), get(&xs, "d") + 50.0 / 2.0 + 75.0 + 50.0 / 2.0);
        assert_eq!(get(&xs, "g"), get(&xs, "f") + 50.0 / 2.0 + 75.0 + 50.0 / 2.0);
    }

    #[test]
    fn handles_labelpos_l() {
        let mut g = new_graph();
        let root = map_str(&[("a", "a"), ("b", "b"), ("c", "c")]);
        let align = map_str(&[("a", "a"), ("b", "b"), ("c", "c")]);
        g.graph_mut().unwrap().edgesep = Some(50.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 100.0, dummy: Some(DummyKind::Edge), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), width: 200.0, dummy: Some(DummyKind::EdgeLabel), label_pos: Some(LabelPos::L), ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(0), order: Some(2), width: 300.0, dummy: Some(DummyKind::Edge), ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), 0.0);
        assert_eq!(get(&xs, "b"), get(&xs, "a") + 100.0 / 2.0 + 50.0 + 200.0);
        assert_eq!(get(&xs, "c"), get(&xs, "b") + 0.0 + 50.0 + 300.0 / 2.0);
    }

    #[test]
    fn handles_labelpos_c() {
        let mut g = new_graph();
        let root = map_str(&[("a", "a"), ("b", "b"), ("c", "c")]);
        let align = map_str(&[("a", "a"), ("b", "b"), ("c", "c")]);
        g.graph_mut().unwrap().edgesep = Some(50.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 100.0, dummy: Some(DummyKind::Edge), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), width: 200.0, dummy: Some(DummyKind::EdgeLabel), label_pos: Some(LabelPos::C), ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(0), order: Some(2), width: 300.0, dummy: Some(DummyKind::Edge), ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), 0.0);
        assert_eq!(get(&xs, "b"), get(&xs, "a") + 100.0 / 2.0 + 50.0 + 200.0 / 2.0);
        assert_eq!(get(&xs, "c"), get(&xs, "b") + 200.0 / 2.0 + 50.0 + 300.0 / 2.0);
    }

    #[test]
    fn handles_labelpos_r() {
        let mut g = new_graph();
        let root = map_str(&[("a", "a"), ("b", "b"), ("c", "c")]);
        let align = map_str(&[("a", "a"), ("b", "b"), ("c", "c")]);
        g.graph_mut().unwrap().edgesep = Some(50.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 100.0, dummy: Some(DummyKind::Edge), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), width: 200.0, dummy: Some(DummyKind::EdgeLabel), label_pos: Some(LabelPos::R), ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(0), order: Some(2), width: 300.0, dummy: Some(DummyKind::Edge), ..Default::default() });
        let xs = horizontal_compaction(&g, &build_layer_matrix(&g), &root, &align, false);
        assert_eq!(get(&xs, "a"), 0.0);
        assert_eq!(get(&xs, "b"), get(&xs, "a") + 100.0 / 2.0 + 50.0 + 0.0);
        assert_eq!(get(&xs, "c"), get(&xs, "b") + 200.0 + 50.0 + 300.0 / 2.0);
    }
}

// ── alignCoordinates ─────────────────────────────────────────────────────────

mod align_coordinates_tests {
    use super::*;

    #[test]
    fn aligns_a_single_node() {
        let mut xss = Xss {
            ul: map_f64(&[("a", 50.0)]),
            ur: map_f64(&[("a", 100.0)]),
            dl: map_f64(&[("a", 50.0)]),
            dr: map_f64(&[("a", 200.0)]),
        };
        let align_to = xss.ul.clone();
        align_coordinates(&mut xss, &align_to);
        assert_eq!(xss.ul, map_f64(&[("a", 50.0)]));
        assert_eq!(xss.ur, map_f64(&[("a", 50.0)]));
        assert_eq!(xss.dl, map_f64(&[("a", 50.0)]));
        assert_eq!(xss.dr, map_f64(&[("a", 50.0)]));
    }

    #[test]
    fn aligns_multiple_nodes() {
        let mut xss = Xss {
            ul: map_f64(&[("a", 50.0), ("b", 1000.0)]),
            ur: map_f64(&[("a", 100.0), ("b", 900.0)]),
            dl: map_f64(&[("a", 150.0), ("b", 800.0)]),
            dr: map_f64(&[("a", 200.0), ("b", 700.0)]),
        };
        let align_to = xss.ul.clone();
        align_coordinates(&mut xss, &align_to);
        assert_eq!(xss.ul, map_f64(&[("a", 50.0), ("b", 1000.0)]));
        assert_eq!(xss.ur, map_f64(&[("a", 200.0), ("b", 1000.0)]));
        assert_eq!(xss.dl, map_f64(&[("a", 50.0), ("b", 700.0)]));
        assert_eq!(xss.dr, map_f64(&[("a", 500.0), ("b", 1000.0)]));
    }
}

// ── findSmallestWidthAlignment ───────────────────────────────────────────────

mod find_smallest_width_alignment_tests {
    use super::*;

    #[test]
    fn finds_the_alignment_with_the_smallest_width() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { width: 50.0, ..Default::default() });
        g.set_node("b", NodeLabel { width: 50.0, ..Default::default() });
        let xss = Xss {
            ul: map_f64(&[("a", 0.0), ("b", 1000.0)]),
            ur: map_f64(&[("a", -5.0), ("b", 1000.0)]),
            dl: map_f64(&[("a", 5.0), ("b", 2000.0)]),
            dr: map_f64(&[("a", 0.0), ("b", 200.0)]),
        };
        assert_eq!(find_smallest_width_alignment(&g, &xss), xss.dr);
    }

    #[test]
    fn takes_node_width_into_account() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { width: 50.0, ..Default::default() });
        g.set_node("b", NodeLabel { width: 50.0, ..Default::default() });
        g.set_node("c", NodeLabel { width: 200.0, ..Default::default() });
        let xss = Xss {
            ul: map_f64(&[("a", 0.0), ("b", 100.0), ("c", 75.0)]),
            ur: map_f64(&[("a", 0.0), ("b", 100.0), ("c", 80.0)]),
            dl: map_f64(&[("a", 0.0), ("b", 100.0), ("c", 85.0)]),
            dr: map_f64(&[("a", 0.0), ("b", 100.0), ("c", 90.0)]),
        };
        assert_eq!(find_smallest_width_alignment(&g, &xss), xss.ul);
    }
}

// ── balance ──────────────────────────────────────────────────────────────────

mod balance_tests {
    use super::*;

    #[test]
    fn aligns_single_node_to_shared_median() {
        let xss = Xss {
            ul: map_f64(&[("a", 0.0)]),
            ur: map_f64(&[("a", 100.0)]),
            dl: map_f64(&[("a", 100.0)]),
            dr: map_f64(&[("a", 200.0)]),
        };
        assert_eq!(balance(&xss, None), map_f64(&[("a", 100.0)]));
    }

    #[test]
    fn aligns_single_node_to_average_of_different_medians() {
        let xss = Xss {
            ul: map_f64(&[("a", 0.0)]),
            ur: map_f64(&[("a", 75.0)]),
            dl: map_f64(&[("a", 125.0)]),
            dr: map_f64(&[("a", 200.0)]),
        };
        assert_eq!(balance(&xss, None), map_f64(&[("a", 100.0)]));
    }

    #[test]
    fn balances_multiple_nodes() {
        let xss = Xss {
            ul: map_f64(&[("a", 0.0), ("b", 50.0)]),
            ur: map_f64(&[("a", 75.0), ("b", 0.0)]),
            dl: map_f64(&[("a", 125.0), ("b", 60.0)]),
            dr: map_f64(&[("a", 200.0), ("b", 75.0)]),
        };
        assert_eq!(balance(&xss, None), map_f64(&[("a", 100.0), ("b", 55.0)]));
    }
}

// ── positionX ────────────────────────────────────────────────────────────────

mod position_x_tests {
    use super::*;

    #[test]
    fn positions_a_single_node_at_origin() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 100.0, ..Default::default() });
        assert_eq!(position_x(&g), map_f64(&[("a", 0.0)]));
    }

    #[test]
    fn positions_a_single_node_block_at_origin() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 100.0, ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(1), order: Some(0), width: 100.0, ..Default::default() });
        g.ensure_edge("a", "b", None);
        assert_eq!(position_x(&g), map_f64(&[("a", 0.0), ("b", 0.0)]));
    }

    #[test]
    fn positions_a_single_node_block_at_origin_even_when_sizes_differ() {
        let mut g = new_graph();
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 40.0, ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(1), order: Some(0), width: 500.0, ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(2), order: Some(0), width: 20.0, ..Default::default() });
        g.ensure_path(&["a", "b", "c"]);
        assert_eq!(position_x(&g), map_f64(&[("a", 0.0), ("b", 0.0), ("c", 0.0)]));
    }

    #[test]
    fn centers_a_node_if_predecessor_of_two_same_sized_nodes() {
        let mut g = new_graph();
        g.graph_mut().unwrap().nodesep = Some(10.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 20.0, ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(1), order: Some(0), width: 50.0, ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(1), width: 50.0, ..Default::default() });
        g.ensure_edge("a", "b", None);
        g.ensure_edge("a", "c", None);
        let pos = position_x(&g);
        let a = get(&pos, "a");
        assert_eq!(pos, map_f64(&[("a", a), ("b", a - (25.0 + 5.0)), ("c", a + (25.0 + 5.0))]));
    }

    #[test]
    fn shifts_blocks_on_both_sides_of_aligned_block() {
        let mut g = new_graph();
        g.graph_mut().unwrap().nodesep = Some(10.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 50.0, ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), width: 60.0, ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), width: 70.0, ..Default::default() });
        g.set_node("d", NodeLabel { rank: Some(1), order: Some(1), width: 80.0, ..Default::default() });
        g.ensure_edge("b", "c", None);
        let pos = position_x(&g);
        let b = get(&pos, "b");
        let c = b;
        assert_eq!(
            pos,
            map_f64(&[
                ("a", b - 60.0 / 2.0 - 10.0 - 50.0 / 2.0),
                ("b", b),
                ("c", c),
                ("d", c + 70.0 / 2.0 + 10.0 + 80.0 / 2.0),
            ])
        );
    }

    #[test]
    fn aligns_inner_segments() {
        let mut g = new_graph();
        g.graph_mut().unwrap().nodesep = Some(10.0);
        g.graph_mut().unwrap().edgesep = Some(10.0);
        g.set_node("a", NodeLabel { rank: Some(0), order: Some(0), width: 50.0, dummy: Some(DummyKind::Edge), ..Default::default() });
        g.set_node("b", NodeLabel { rank: Some(0), order: Some(1), width: 60.0, ..Default::default() });
        g.set_node("c", NodeLabel { rank: Some(1), order: Some(0), width: 70.0, ..Default::default() });
        g.set_node("d", NodeLabel { rank: Some(1), order: Some(1), width: 80.0, dummy: Some(DummyKind::Edge), ..Default::default() });
        g.ensure_edge("b", "c", None);
        g.ensure_edge("a", "d", None);
        let pos = position_x(&g);
        let a = get(&pos, "a");
        let d = a;
        assert_eq!(
            pos,
            map_f64(&[
                ("a", a),
                ("b", a + 50.0 / 2.0 + 10.0 + 60.0 / 2.0),
                ("c", d - 70.0 / 2.0 - 10.0 - 80.0 / 2.0),
                ("d", d),
            ])
        );
    }
}

// ── position (index.ts) ──────────────────────────────────────────────────────

mod position_tests {
    use super::*;

    fn new_compound_graph() -> DagreGraph {
        let mut g: DagreGraph = Graph::new(GraphOptions {
            directed: true,
            multigraph: false,
            compound: true,
        });
        g.set_graph(GraphLabel {
            ranksep: Some(50.0),
            nodesep: Some(50.0),
            edgesep: Some(10.0),
            ..Default::default()
        });
        g
    }

    #[test]
    fn respects_ranksep() {
        let mut g = new_compound_graph();
        g.graph_mut().unwrap().ranksep = Some(1000.0);
        g.set_node("a", NodeLabel { width: 50.0, height: 100.0, rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { width: 50.0, height: 80.0, rank: Some(1), order: Some(0), ..Default::default() });
        g.ensure_edge("a", "b", None);
        position(&mut g);
        assert_eq!(g.node("b").unwrap().y, Some(100.0 + 1000.0 + 80.0 / 2.0));
    }

    #[test]
    fn uses_largest_height_in_each_rank_with_ranksep() {
        let mut g = new_compound_graph();
        g.graph_mut().unwrap().ranksep = Some(1000.0);
        g.set_node("a", NodeLabel { width: 50.0, height: 100.0, rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { width: 50.0, height: 80.0, rank: Some(0), order: Some(1), ..Default::default() });
        g.set_node("c", NodeLabel { width: 50.0, height: 90.0, rank: Some(1), order: Some(0), ..Default::default() });
        g.ensure_edge("a", "c", None);
        position(&mut g);
        assert_eq!(g.node("a").unwrap().y, Some(100.0 / 2.0));
        assert_eq!(g.node("b").unwrap().y, Some(100.0 / 2.0));
        assert_eq!(g.node("c").unwrap().y, Some(100.0 + 1000.0 + 90.0 / 2.0));
    }

    #[test]
    fn respects_nodesep() {
        let mut g = new_compound_graph();
        g.graph_mut().unwrap().nodesep = Some(1000.0);
        g.set_node("a", NodeLabel { width: 50.0, height: 100.0, rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { width: 70.0, height: 80.0, rank: Some(0), order: Some(1), ..Default::default() });
        position(&mut g);
        let ax = g.node("a").unwrap().x.unwrap();
        assert_eq!(g.node("b").unwrap().x, Some(ax + 50.0 / 2.0 + 1000.0 + 70.0 / 2.0));
    }

    #[test]
    fn should_not_position_the_subgraph_node_itself() {
        let mut g = new_compound_graph();
        g.set_node("a", NodeLabel { width: 50.0, height: 50.0, rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("sg1", NodeLabel::default());
        g.set_parent("a", "sg1");
        position(&mut g);
        assert_eq!(g.node("sg1").unwrap().x, None);
        assert_eq!(g.node("sg1").unwrap().y, None);
    }

    #[test]
    fn aligns_nodes_to_top_when_rankalign_top() {
        let mut g = new_compound_graph();
        g.graph_mut().unwrap().rank_align = Some(crate::layered::types::RankAlign::Top);
        g.set_node("a", NodeLabel { width: 50.0, height: 100.0, rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { width: 50.0, height: 60.0, rank: Some(0), order: Some(1), ..Default::default() });
        position(&mut g);
        assert_eq!(g.node("a").unwrap().y, Some(100.0 / 2.0));
        assert_eq!(g.node("b").unwrap().y, Some(60.0 / 2.0));
    }

    #[test]
    fn aligns_nodes_to_bottom_when_rankalign_bottom() {
        let mut g = new_compound_graph();
        g.graph_mut().unwrap().rank_align = Some(crate::layered::types::RankAlign::Bottom);
        g.set_node("a", NodeLabel { width: 50.0, height: 100.0, rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { width: 50.0, height: 60.0, rank: Some(0), order: Some(1), ..Default::default() });
        position(&mut g);
        assert_eq!(g.node("a").unwrap().y, Some(100.0 - 100.0 / 2.0));
        assert_eq!(g.node("b").unwrap().y, Some(100.0 - 60.0 / 2.0));
    }

    #[test]
    fn aligns_nodes_to_center_when_rankalign_center() {
        let mut g = new_compound_graph();
        g.graph_mut().unwrap().rank_align = Some(crate::layered::types::RankAlign::Center);
        g.set_node("a", NodeLabel { width: 50.0, height: 100.0, rank: Some(0), order: Some(0), ..Default::default() });
        g.set_node("b", NodeLabel { width: 50.0, height: 60.0, rank: Some(0), order: Some(1), ..Default::default() });
        position(&mut g);
        assert_eq!(g.node("a").unwrap().y, Some(100.0 / 2.0));
        assert_eq!(g.node("b").unwrap().y, Some(100.0 / 2.0));
    }
}
