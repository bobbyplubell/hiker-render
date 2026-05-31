//! Nesting graph — a port of `dagre/lib/nesting-graph.ts`.
//!
//! A nesting graph creates dummy nodes for the tops and bottoms of subgraphs,
//! adds edges to ensure that all cluster nodes are placed between these
//! boundaries, and ensures the graph is connected. Through `minlen` it also
//! ensures nodes and subgraph border nodes do not land on the same rank.
//!
//! Preconditions:
//!   1. Input graph is a DAG.
//!   2. Nodes in the input graph have a `minlen` attribute (on edges).
//!
//! Postconditions:
//!   1. Input graph is connected.
//!   2. Dummy nodes are added for the tops and bottoms of subgraphs.
//!   3. The `minlen` attribute for edges is adjusted to ensure nodes do not get
//!      placed on the same rank as subgraph border nodes.
//!
//! The nesting graph idea comes from Sander, "Layout of Compound Directed
//! Graphs."

use std::collections::HashMap;

use super::types::{DagreGraph, DummyKind, EdgeLabel, NodeLabel};
use super::util::{add_border_node, add_dummy_node, apply_with_chunking_max, GRAPH_NODE};

/// `run(graph)` — build the nesting graph.
pub fn run(graph: &mut DagreGraph) {
    let root: String = add_dummy_node(graph, DummyKind::Root, NodeLabel::default(), "_root");
    let depths: HashMap<String, i32> = tree_depths(graph);
    let depths_arr: Vec<i32> = depths.values().copied().collect();
    // Note: depths is an Object not an array. height = max(depths) - 1.
    let height: i32 = apply_with_chunking_max(&depths_arr).saturating_sub(1);
    let node_sep: i32 = 2 * height + 1;

    if let Some(g) = graph.graph_mut() {
        g.nesting_root = Some(root.clone());
    }

    // Multiply minlen by nodeSep to align nodes on non-border ranks.
    for e in graph.edges() {
        if let Some(label) = graph.edge_mut(&e.v, &e.w, e.name.as_deref()) {
            let cur = label.minlen.unwrap_or(0);
            label.minlen = Some(cur * node_sep);
        }
    }

    // Calculate a weight that is sufficient to keep subgraphs vertically compact.
    let weight: f64 = sum_weights(graph) + 1.0;

    // Create border nodes and link them up.
    for child in graph.children(GRAPH_NODE) {
        dfs(graph, &root, node_sep, weight, height, &depths, &child);
    }

    // Save the multiplier for node layers for later removal of empty border
    // layers.
    if let Some(g) = graph.graph_mut() {
        g.node_rank_factor = Some(node_sep);
    }
}

#[allow(clippy::too_many_arguments)]
fn dfs(
    graph: &mut DagreGraph,
    root: &str,
    node_sep: i32,
    weight: f64,
    height: i32,
    depths: &HashMap<String, i32>,
    v: &str,
) {
    let children: Vec<String> = graph.children(v);
    if children.is_empty() {
        if v != root {
            graph.set_edge(
                root,
                v,
                EdgeLabel {
                    weight: Some(0.0),
                    minlen: Some(node_sep),
                    ..Default::default()
                },
                None,
            );
        }
        return;
    }

    let top: String = add_border_node(graph, "_bt", None, None);
    let bottom: String = add_border_node(graph, "_bb", None, None);

    graph.set_parent(top.clone(), v);
    if let Some(label) = graph.node_mut(v) {
        label.border_top = Some(top.clone());
    }
    graph.set_parent(bottom.clone(), v);
    if let Some(label) = graph.node_mut(v) {
        label.border_bottom = Some(bottom.clone());
    }

    for child in &children {
        dfs(graph, root, node_sep, weight, height, depths, child);

        let child_node = graph.node(child).cloned().unwrap_or_default();
        let child_top: String = child_node.border_top.clone().unwrap_or_else(|| child.clone());
        let child_bottom: String = child_node
            .border_bottom
            .clone()
            .unwrap_or_else(|| child.clone());
        let this_weight: f64 = if child_node.border_top.is_some() {
            weight
        } else {
            2.0 * weight
        };
        let minlen: i32 = if child_top != child_bottom {
            1
        } else {
            height - depths.get(v).copied().unwrap_or(0) + 1
        };

        graph.set_edge(
            top.clone(),
            child_top,
            EdgeLabel {
                weight: Some(this_weight),
                minlen: Some(minlen),
                nesting_edge: Some(true),
                ..Default::default()
            },
            None,
        );

        graph.set_edge(
            child_bottom,
            bottom.clone(),
            EdgeLabel {
                weight: Some(this_weight),
                minlen: Some(minlen),
                nesting_edge: Some(true),
                ..Default::default()
            },
            None,
        );
    }

    if graph.parent(v).is_none() {
        graph.set_edge(
            root,
            top.clone(),
            EdgeLabel {
                weight: Some(0.0),
                minlen: Some(height + depths.get(v).copied().unwrap_or(0)),
                ..Default::default()
            },
            None,
        );
    }
}

fn tree_depths(graph: &DagreGraph) -> HashMap<String, i32> {
    let mut depths: HashMap<String, i32> = HashMap::new();

    fn dfs(graph: &DagreGraph, depths: &mut HashMap<String, i32>, v: &str, depth: i32) {
        let children = graph.children(v);
        if !children.is_empty() {
            for child in &children {
                dfs(graph, depths, child, depth + 1);
            }
        }
        depths.insert(v.to_string(), depth);
    }

    for v in graph.children(GRAPH_NODE) {
        dfs(graph, &mut depths, &v, 1);
    }
    depths
}

fn sum_weights(graph: &DagreGraph) -> f64 {
    graph.edges().iter().fold(0.0, |acc, e| {
        acc + graph.edge_by_obj(e).and_then(|l| l.weight).unwrap_or(0.0)
    })
}

/// `cleanup(graph)` — remove the nesting root and all nesting edges.
pub fn cleanup(graph: &mut DagreGraph) {
    let nesting_root = graph.graph().and_then(|g| g.nesting_root.clone());
    if let Some(root) = nesting_root {
        graph.remove_node(&root);
    }
    if let Some(g) = graph.graph_mut() {
        g.nesting_root = None;
    }
    for e in graph.edges() {
        let is_nesting = graph
            .edge_by_obj(&e)
            .and_then(|l| l.nesting_edge)
            .unwrap_or(false);
        if is_nesting {
            graph.remove_edge_obj(&e);
        }
    }
}

#[cfg(test)]
mod tests;
