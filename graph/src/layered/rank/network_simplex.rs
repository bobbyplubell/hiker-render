//! Network simplex ranking — a port of `dagre/lib/rank/network-simplex.ts`.
//!
//! The Gansner network-simplex assigns ranks and iteratively improves them to
//! shorten edges. Sketch: `simplify` → [`longest_path`] initial ranks →
//! [`feasible_tree`] tight tree → [`init_low_lim_values`] → [`init_cut_values`]
//! → while [`leave_edge`] finds a tree edge with a negative cut value:
//! [`enter_edge`] (min-slack non-tree edge crossing the cut) and
//! [`exchange_edges`].
//!
//! The internal helpers (`init_low_lim_values`, `init_cut_values`,
//! `calc_cut_value`, `leave_edge`, `enter_edge`, `exchange_edges`) are exposed
//! `pub(crate)` because the conformance test (`network-simplex-test.ts`) reaches
//! into them directly — dagre attaches them as properties on `networkSimplex`.
//!
//! Test names in the `#[cfg(test)]` module mirror
//! `dagre/test/rank/network-simplex-test.ts`.

use std::collections::HashSet;

use crate::layered::graph::Edge;
use crate::layered::types::DagreGraph;
use crate::layered::util::simplify;

use super::feasible_tree::{feasible_tree, TreeGraph, TreeNodeLabel};
use super::util::{longest_path as init_rank, slack};

/// `networkSimplex(graph)` — assign optimized ranks to `graph` in place.
///
/// Operates on a simplified copy (multigraph edges aggregated), then copies the
/// resulting ranks back onto the input graph's nodes.
pub fn network_simplex(graph: &mut DagreGraph) {
    let mut g = simplify(graph);
    init_rank(&mut g);

    let mut t = feasible_tree(&mut g);
    init_low_lim_values(&mut t, None);
    init_cut_values(&mut t, &g);

    while let Some(e) = leave_edge(&t) {
        let f = enter_edge(&t, &g, &e);
        exchange_edges(&mut t, &mut g, &e, &f);
    }

    // Copy ranks from the simplified graph back to the input graph.
    for v in graph.nodes() {
        if let Some(rank) = g.node(&v).and_then(|n| n.rank) {
            if let Some(node) = graph.node_mut(&v) {
                node.rank = Some(rank);
            }
        }
    }
}

// ── pre/post-order DFS over the (undirected) tree ──────────────────────────
//
// Mirrors graphlib's `alg.preorder` / `alg.postorder`: for an undirected graph
// navigation uses `neighbors`.

fn dfs_order(tree: &TreeGraph, roots: &[String], postorder: bool) -> Vec<String> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut acc: Vec<String> = Vec::new();
    for v in roots {
        do_reduce(tree, v, postorder, &mut visited, &mut acc);
    }
    acc
}

fn do_reduce(
    tree: &TreeGraph,
    v: &str,
    postorder: bool,
    visited: &mut HashSet<String>,
    acc: &mut Vec<String>,
) {
    if visited.contains(v) {
        return;
    }
    visited.insert(v.to_string());
    if !postorder {
        acc.push(v.to_string());
    }
    for w in tree.neighbors(v).unwrap_or_default() {
        do_reduce(tree, &w, postorder, visited, acc);
    }
    if postorder {
        acc.push(v.to_string());
    }
}

fn preorder(tree: &TreeGraph, roots: &[String]) -> Vec<String> {
    dfs_order(tree, roots, false)
}

fn postorder(tree: &TreeGraph, roots: &[String]) -> Vec<String> {
    dfs_order(tree, roots, true)
}

// ── cut values ─────────────────────────────────────────────────────────────

/// Initializes cut values for all edges in `tree`.
pub(crate) fn init_cut_values(tree: &mut TreeGraph, graph: &DagreGraph) {
    let mut visited_nodes = postorder(tree, &tree.nodes());
    // Drop the last node (the root has no parent edge).
    visited_nodes.pop();
    for v in visited_nodes {
        assign_cut_value(tree, graph, &v);
    }
}

fn assign_cut_value(tree: &mut TreeGraph, graph: &DagreGraph, child: &str) {
    let parent = tree
        .node(child)
        .and_then(|n| n.parent.clone())
        .expect("assignCutValue: child has no parent");
    let cutvalue = calc_cut_value(tree, graph, child);
    if let Some(edge) = tree.edge_mut(child, &parent, None) {
        edge.cutvalue = Some(cutvalue);
    }
}

/// Given the tight tree, its graph, and a `child`, calculate the cut value of
/// the edge between `child` and its parent.
pub(crate) fn calc_cut_value(tree: &TreeGraph, graph: &DagreGraph, child: &str) -> f64 {
    let parent = tree
        .node(child)
        .and_then(|n| n.parent.clone())
        .expect("calcCutValue: child has no parent");
    // True if the child is on the tail end of the edge in the directed graph.
    let mut child_is_tail = true;
    // The graph's view of the tree edge we're inspecting.
    let mut graph_edge = graph.edge(child, &parent, None);
    if graph_edge.is_none() {
        child_is_tail = false;
        graph_edge = graph.edge(&parent, child, None);
    }
    let mut cut_value = graph_edge
        .and_then(|e| e.weight)
        .expect("calcCutValue: missing tree edge in graph");

    let node_edges = graph.node_edges(child, None).unwrap_or_default();
    for edge in node_edges {
        let is_out_edge = edge.v == child;
        let other = if is_out_edge { edge.w.clone() } else { edge.v.clone() };

        if other != parent {
            let points_to_head = is_out_edge == child_is_tail;
            let other_weight = graph.edge_by_obj(&edge).and_then(|e| e.weight).unwrap_or(0.0);

            cut_value += if points_to_head { other_weight } else { -other_weight };
            if is_tree_edge(tree, child, &other) {
                let other_cut_value = tree
                    .edge(child, &other, None)
                    .and_then(|e| e.cutvalue)
                    .unwrap_or(0.0);
                cut_value += if points_to_head { -other_cut_value } else { other_cut_value };
            }
        }
    }

    cut_value
}

// ── low / lim values ─────────────────────────────────────────────────────────

/// Assigns `low`/`lim`/`parent` to each tree node via a DFS. `root` defaults to
/// the first node.
pub(crate) fn init_low_lim_values(tree: &mut TreeGraph, root: Option<&str>) {
    let root = root
        .map(|r| r.to_string())
        .unwrap_or_else(|| tree.nodes()[0].clone());
    let mut visited: HashSet<String> = HashSet::new();
    dfs_assign_low_lim(tree, &mut visited, 1, &root, None);
}

fn dfs_assign_low_lim(
    tree: &mut TreeGraph,
    visited: &mut HashSet<String>,
    mut next_lim: i32,
    v: &str,
    parent: Option<&str>,
) -> i32 {
    let low = next_lim;
    visited.insert(v.to_string());

    for w in tree.neighbors(v).unwrap_or_default() {
        if !visited.contains(&w) {
            next_lim = dfs_assign_low_lim(tree, visited, next_lim, &w, Some(v));
        }
    }

    let lim = next_lim;
    next_lim += 1;
    if let Some(label) = tree.node_mut(v) {
        label.low = Some(low);
        label.lim = Some(lim);
        label.parent = parent.map(|p| p.to_string());
    }

    next_lim
}

// ── leave / enter / exchange ────────────────────────────────────────────────

/// Returns the first tree edge with a negative cut value, or `None`.
pub(crate) fn leave_edge(tree: &TreeGraph) -> Option<Edge> {
    tree.edges().into_iter().find(|e| {
        tree.edge_by_obj(e)
            .and_then(|edge| edge.cutvalue)
            .map(|c| c < 0.0)
            .unwrap_or(false)
    })
}

/// Finds the min-slack non-tree edge that, when added, replaces `edge`.
pub(crate) fn enter_edge(tree: &TreeGraph, graph: &DagreGraph, edge: &Edge) -> Edge {
    let mut v = edge.v.clone();
    let mut w = edge.w.clone();

    // Assume v is the tail and w is the head; flip if necessary to match the
    // directed graph's orientation.
    if !graph.has_edge(&v, &w, None) {
        v = edge.w.clone();
        w = edge.v.clone();
    }

    let v_lim = tree.node(&v).and_then(|n| n.lim).expect("enterEdge: v has no lim");
    let w_lim = tree.node(&w).and_then(|n| n.lim).expect("enterEdge: w has no lim");

    // tailLabel = vLabel by default; if root is in the tail of the edge, flip.
    let (tail_label, flip) = if v_lim > w_lim {
        (tree.node(&w).cloned().unwrap(), true)
    } else {
        (tree.node(&v).cloned().unwrap(), false)
    };

    let candidates: Vec<Edge> = graph
        .edges()
        .into_iter()
        .filter(|e| {
            let ev = tree.node(&e.v).cloned().unwrap_or_default();
            let ew = tree.node(&e.w).cloned().unwrap_or_default();
            flip == is_descendant(&ev, &tail_label) && flip != is_descendant(&ew, &tail_label)
        })
        .collect();

    // candidates.reduce((acc, e) => slack(e) < slack(acc) ? e : acc) with the
    // first element as the initial accumulator.
    let mut iter = candidates.into_iter();
    let mut acc = iter.next().expect("enterEdge: no candidate edge");
    for e in iter {
        if slack(graph, &e) < slack(graph, &acc) {
            acc = e;
        }
    }
    acc
}

/// Removes tree edge `e`, adds `f`, and recomputes low/lim, cut values, ranks.
pub(crate) fn exchange_edges(tree: &mut TreeGraph, graph: &mut DagreGraph, e: &Edge, f: &Edge) {
    tree.remove_edge(&e.v, &e.w, None);
    tree.set_edge(f.v.clone(), f.w.clone(), Default::default(), None);
    init_low_lim_values(tree, None);
    init_cut_values(tree, graph);
    update_ranks(tree, graph);
}

fn update_ranks(tree: &TreeGraph, graph: &mut DagreGraph) {
    let root = tree
        .nodes()
        .into_iter()
        .find(|v| tree.node(v).map(|n| n.parent.is_none()).unwrap_or(false));
    let root = match root {
        Some(r) => r,
        None => return,
    };

    let mut vs = preorder(tree, &[root]);
    vs.remove(0);
    for v in vs {
        let parent = tree
            .node(&v)
            .and_then(|n| n.parent.clone())
            .expect("updateRanks: node has no parent");

        let (minlen, flipped) = match graph.edge(&v, &parent, None) {
            Some(e) => (e.minlen.unwrap_or(1), false),
            None => {
                let e = graph
                    .edge(&parent, &v, None)
                    .expect("updateRanks: missing tree edge in graph");
                (e.minlen.unwrap_or(1), true)
            }
        };
        let parent_rank = graph.node(&parent).and_then(|n| n.rank).unwrap_or(0);
        let new_rank = parent_rank + if flipped { minlen } else { -minlen };
        if let Some(node) = graph.node_mut(&v) {
            node.rank = Some(new_rank);
        }
    }
}

/// True if `(u, v)` is an edge of the tree.
fn is_tree_edge(tree: &TreeGraph, u: &str, v: &str) -> bool {
    tree.has_edge(u, v, None)
}

/// True if `v_label` is a descendant of `root_label` per the low/lim numbering.
fn is_descendant(v_label: &TreeNodeLabel, root_label: &TreeNodeLabel) -> bool {
    let low = root_label.low.unwrap_or(i32::MIN);
    let lim = root_label.lim.unwrap_or(i32::MAX);
    let v_lim = v_label.lim.unwrap_or(i32::MIN);
    low <= v_lim && v_lim <= lim
}

#[cfg(test)]
mod tests;
