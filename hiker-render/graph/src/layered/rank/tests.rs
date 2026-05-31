//! Port of `dagre/test/rank/rank-test.ts`. Test names mirror the TS.

use super::*;
use crate::layered::graph::{Graph, GraphOptions};
use crate::layered::types::{EdgeLabel, GraphLabel, NodeLabel};

/// The RANKERS list from the TS, mapped to the Rust `Ranker` enum. The unknown
/// case maps to `None`, which the dispatcher resolves to network-simplex.
fn rankers() -> Vec<(&'static str, Option<Ranker>)> {
    vec![
        ("longest-path", Some(Ranker::LongestPath)),
        ("tight-tree", Some(Ranker::TightTree)),
        ("network-simplex", Some(Ranker::NetworkSimplex)),
        ("unknown-should-still-work", None),
    ]
}

fn base_graph(ranker: Option<Ranker>) -> DagreGraph {
    let mut g: DagreGraph = Graph::new(GraphOptions::default());
    g.set_graph(GraphLabel {
        ranker,
        ..Default::default()
    });
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

#[test]
fn respects_the_minlen_attribute() {
    for (_name, ranker) in rankers() {
        let mut g = base_graph(ranker);
        rank(&mut g);
        for e in g.edges() {
            let v_rank = g.node(&e.v).and_then(|n| n.rank).unwrap();
            let w_rank = g.node(&e.w).and_then(|n| n.rank).unwrap();
            let minlen = g.edge_by_obj(&e).and_then(|l| l.minlen).unwrap();
            assert!(
                w_rank - v_rank >= minlen,
                "ranker {:?}: edge {}->{} rank diff {} < minlen {}",
                ranker,
                e.v,
                e.w,
                w_rank - v_rank,
                minlen
            );
        }
    }
}

#[test]
fn can_rank_a_single_node_graph() {
    for (_name, ranker) in rankers() {
        let mut g: DagreGraph = Graph::new(GraphOptions::default());
        g.set_graph(GraphLabel::default());
        g.set_node("a", NodeLabel::default());
        // The graph's ranker drives the dispatch.
        g.graph_mut().unwrap().ranker = ranker;
        rank(&mut g);
        assert_eq!(g.node("a").and_then(|n| n.rank), Some(0));
    }
}
