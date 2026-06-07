//! Break long edges into chains of dummy nodes — a port of
//! `dagre/lib/normalize.ts`.
//!
//! [`run`] replaces every edge that spans more than one rank with a path of
//! dummy nodes, one per intermediate rank, so that all edges in the graph span
//! exactly one rank. The first dummy of each chain is pushed onto the graph
//! label's `dummy_chains`. [`undo`] reverses the operation, collecting the
//! dummies' positions into the original edge's `points` and restoring the
//! original edge.
//!
//! # Bookkeeping carried on each dummy (must match dagre exactly)
//!
//! Each chain dummy is created via [`add_dummy_node`] with `DummyKind::Edge`
//! and carries on its [`NodeLabel`]:
//! * `edge_label` — the original [`EdgeLabel`] (shared across the chain in TS;
//!   here cloned, with the *first* dummy's clone being the one that accumulates
//!   `points` and label coords in [`undo`]),
//! * `edge_obj` — the original [`Edge`] to restore,
//! * `rank` — the intermediate rank.
//!
//! The dummy sitting on the edge's `label_rank` is promoted to
//! `DummyKind::EdgeLabel` and given the edge label's `width`/`height`/`labelpos`
//! (dagre attaches the edge label to that dummy). The reconnecting edges carry
//! the original `weight`, and the original edge's `name` (forward name) is
//! threaded through so reversed/multi-edges round-trip.

use super::graph::{Edge, Graph};
use super::types::{DummyKind, EdgeLabel, GraphLabel, NodeLabel};
use super::util::add_dummy_node;

type DagreGraph = Graph<GraphLabel, NodeLabel, EdgeLabel>;

/// `run(graph)` — break every long edge into a chain of single-rank segments.
///
/// Pre-conditions: the graph is a DAG and every node has a `rank`.
/// Post-conditions: all edges span exactly one rank; dummy nodes fill the gaps;
/// the graph label's `dummy_chains` holds the first dummy of each chain.
pub fn run(graph: &mut DagreGraph) {
    // graph.graph().dummyChains = [];
    if let Some(g) = graph.graph_mut() {
        g.dummy_chains = Some(Vec::new());
    }
    for edge in graph.edges() {
        normalize_edge(graph, &edge);
    }
}

fn normalize_edge(graph: &mut DagreGraph, e: &Edge) {
    let mut v = e.v.clone();
    let mut v_rank = graph.node(&v).and_then(|n| n.rank).unwrap();
    let w = e.w.clone();
    let w_rank = graph.node(&w).and_then(|n| n.rank).unwrap();
    let name = e.name.clone();
    // edgeLabel is shared in TS; we clone it but keep a single canonical copy
    // (carried on the dummies) whose `points` accumulate in undo.
    let mut edge_label = graph.edge_by_obj(e).cloned().unwrap_or_default();
    let label_rank = edge_label.label_rank;

    if w_rank == v_rank + 1 {
        return;
    }

    graph.remove_edge_obj(e);

    let weight = edge_label.weight;

    let mut i = 0;
    v_rank += 1;
    while v_rank < w_rank {
        // edgeLabel.points = []
        edge_label.points = Some(Vec::new());

        let mut attrs = NodeLabel {
            width: 0.0,
            height: 0.0,
            edge_label: Some(Box::new(edge_label.clone())),
            edge_obj: Some(e.clone()),
            rank: Some(v_rank),
            ..Default::default()
        };

        let kind = if Some(v_rank) == label_rank {
            attrs.width = edge_label.width.unwrap_or(0.0);
            attrs.height = edge_label.height.unwrap_or(0.0);
            attrs.label_pos = edge_label.label_pos;
            DummyKind::EdgeLabel
        } else {
            DummyKind::Edge
        };

        let dummy = add_dummy_node(graph, kind, attrs, "_d");

        graph.set_edge(
            v.clone(),
            dummy.clone(),
            EdgeLabel {
                weight,
                ..Default::default()
            },
            name.as_deref(),
        );
        if i == 0 {
            if let Some(g) = graph.graph_mut() {
                g.dummy_chains
                    .get_or_insert_with(Vec::new)
                    .push(dummy.clone());
            }
        }
        v = dummy;
        i += 1;
        v_rank += 1;
    }

    graph.set_edge(
        v,
        w,
        EdgeLabel {
            weight,
            ..Default::default()
        },
        name.as_deref(),
    );
}

/// `undo(graph)` — reverse [`run`], collecting dummy positions into each
/// original edge's `points` and restoring the original edges and labels.
pub fn undo(graph: &mut DagreGraph) {
    let chains = graph
        .graph()
        .and_then(|g| g.dummy_chains.clone())
        .unwrap_or_default();

    for first in chains {
        let mut v = first;
        // node = graph.node(v)
        let mut node = match graph.node(&v) {
            Some(n) => n.clone(),
            None => continue,
        };
        // origLabel = node.edgeLabel
        let mut orig_label = node
            .edge_label
            .clone()
            .map(|b| *b)
            .unwrap_or_default();
        // graph.setEdge(node.edgeObj, origLabel)
        let edge_obj = node.edge_obj.clone().unwrap();

        // We must set the edge first (dagre does), then mutate origLabel while
        // walking and re-set at the end. Since our store holds a clone, mutate
        // the local copy and write it back after the walk.
        graph.set_edge_obj(&edge_obj, orig_label.clone());

        while node.dummy.is_some() {
            // w = graph.successors(v)[0]
            let w = graph
                .successors(&v)
                .and_then(|s| s.into_iter().next())
                .expect("normalize::undo: dummy chain has no successor");
            graph.remove_node(&v);
            orig_label
                .points
                .get_or_insert_with(Vec::new)
                // dagre pushes {x: node.x!, y: node.y!}; an un-positioned dummy
                // yields `undefined` there (read as a number later). We default
                // to 0.0 so the round-trip never panics; the order tests that
                // care about points always pre-set coords.
                .push(super::types::Point {
                    x: node.x.unwrap_or(0.0),
                    y: node.y.unwrap_or(0.0),
                });
            if node.dummy == Some(DummyKind::EdgeLabel) {
                orig_label.x = node.x;
                orig_label.y = node.y;
                orig_label.width = Some(node.width);
                orig_label.height = Some(node.height);
            }
            v = w;
            node = match graph.node(&v) {
                Some(n) => n.clone(),
                None => NodeLabel::default(),
            };
        }

        // Persist the accumulated points / label coords onto the restored edge.
        graph.set_edge_obj(&edge_obj, orig_label);
    }
}

#[cfg(test)]
mod tests;
