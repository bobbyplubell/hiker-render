//! Greedy feedback-arc-set heuristic — a port of `dagre/lib/greedy-fas.ts`.
//!
//! The Eades–Lin–Smyth greedy heuristic (P. Eades, X. Lin, W. F. Smyth, "A fast
//! and effective heuristic for the feedback arc set problem"), adjusted to allow
//! weighted edges. A feedback arc set is a set of edges that can be removed to
//! make a graph acyclic.
//!
//! # Structure (mirrors the TS)
//!
//! `build_state` constructs an auxiliary `fasGraph` whose node labels are
//! per-node FAS entries (`{v, in, out}`) and whose edge labels are the summed
//! weights of the (possibly multi-) edges between two nodes. The entries are
//! bucketed by `out - in` degree into a row of intrusive lists (see
//! [`super::list`]): bucket `0` = sinks (`out == 0`), bucket `len-1` = sources
//! (`in == 0`), the rest indexed `out - in + zeroIdx`. `do_greedy_fas` then
//! repeatedly drains sinks and sources, and otherwise pulls the max-`out-in`
//! node from the middle, collecting its in-edges as part of the FAS.
//!
//! The returned `Vec<Edge>` re-expands multi-edges via `out_edges(v, w)` on the
//! *original* graph, exactly as the TS `flatMap` does.

use super::graph::{Edge, Graph, GraphOptions};
use super::list::{EntryId, List, ListArena};

/// dagre's `DEFAULT_WEIGHT_FN = () => 1`.
fn default_weight(_e: &Edge) -> f64 {
    1.0
}

/// A FAS entry — the per-node bookkeeping (`{v, in, out}`) that the TS stores as
/// the fasGraph node label and threads through the bucket lists.
#[derive(Clone, Debug)]
struct FasEntry {
    v: String,
    in_deg: f64,
    out_deg: f64,
}

/// The auxiliary fasGraph: node label = arena id of that node's [`FasEntry`],
/// edge label = summed weight between the pair.
type FasGraph = Graph<(), EntryId, f64>;

struct FasState {
    graph: FasGraph,
    arena: ListArena<FasEntry>,
    buckets: Vec<List>,
    zero_idx: i64,
}

/// `greedyFAS(graph, weightFn?)` — returns the set of edges to remove to make
/// `graph` acyclic. `weight_fn` defaults to `1` per edge (pass `None`).
pub fn greedy_fas<G, N, E>(
    graph: &Graph<G, N, E>,
    weight_fn: Option<&dyn Fn(&Edge) -> f64>,
) -> Vec<Edge> {
    if graph.node_count() <= 1 {
        return Vec::new();
    }
    let wf: &dyn Fn(&Edge) -> f64 = weight_fn.unwrap_or(&default_weight);
    let mut state = build_state(graph, wf);
    let results = do_greedy_fas(&mut state);

    // Expand multi-edges against the original graph.
    let mut out = Vec::new();
    for edge in results {
        if let Some(es) = graph.out_edges(&edge.v, Some(&edge.w)) {
            out.extend(es);
        }
    }
    out
}

fn do_greedy_fas(state: &mut FasState) -> Vec<Edge> {
    let mut results: Vec<Edge> = Vec::new();
    let sources = *state.buckets.last().unwrap();
    let sinks = state.buckets[0];

    while state.graph.node_count() > 0 {
        while let Some(entry) = state.arena.dequeue(sinks) {
            remove_node(state, entry, false, &mut results);
        }
        while let Some(entry) = state.arena.dequeue(sources) {
            remove_node(state, entry, false, &mut results);
        }
        if state.graph.node_count() > 0 {
            let mut i = state.buckets.len() as i64 - 2;
            while i > 0 {
                let bucket = state.buckets[i as usize];
                if let Some(entry) = state.arena.dequeue(bucket) {
                    remove_node(state, entry, true, &mut results);
                    break;
                }
                i -= 1;
            }
        }
    }

    results
}

/// `removeNode(graph, buckets, zeroIdx, entry, collectPredecessors?)`.
///
/// When `collect_predecessors`, each in-edge `(u, v)` is pushed onto `results`
/// (these become part of the FAS). The neighbour entries' degrees are updated
/// and re-bucketed, then the node is removed from the fasGraph.
fn remove_node(state: &mut FasState, entry: EntryId, collect_predecessors: bool, results: &mut Vec<Edge>) {
    let v = state.arena.payload(entry).v.clone();

    // In-edges: decrement predecessors' out-degree.
    if let Some(in_edges) = state.graph.in_edges(&v, None) {
        for edge in in_edges {
            let weight = *state.graph.edge_by_obj(&edge).unwrap();
            if collect_predecessors {
                results.push(Edge::new(edge.v.clone(), edge.w.clone(), None));
            }
            let u_entry = *state.graph.node(&edge.v).unwrap();
            state.arena.payload_mut(u_entry).out_deg -= weight;
            assign_bucket(&mut state.arena, &state.buckets, state.zero_idx, u_entry);
        }
    }

    // Out-edges: decrement successors' in-degree.
    if let Some(out_edges) = state.graph.out_edges(&v, None) {
        for edge in out_edges {
            let weight = *state.graph.edge_by_obj(&edge).unwrap();
            let w_entry = *state.graph.node(&edge.w).unwrap();
            state.arena.payload_mut(w_entry).in_deg -= weight;
            assign_bucket(&mut state.arena, &state.buckets, state.zero_idx, w_entry);
        }
    }

    state.graph.remove_node(&v);
}

fn build_state<G, N, E>(graph: &Graph<G, N, E>, weight_fn: &dyn Fn(&Edge) -> f64) -> FasState {
    let mut fas_graph: FasGraph = Graph::new(GraphOptions::default());
    let mut arena: ListArena<FasEntry> = ListArena::new();
    let mut max_in = 0.0_f64;
    let mut max_out = 0.0_f64;

    for v in graph.nodes() {
        let id = arena.new_entry(FasEntry {
            v: v.clone(),
            in_deg: 0.0,
            out_deg: 0.0,
        });
        fas_graph.set_node(v, id);
    }

    // Aggregate weights on nodes, summing multi-edges into a single fasGraph edge.
    for edge in graph.edges() {
        let prev_weight = fas_graph.edge(&edge.v, &edge.w, None).copied().unwrap_or(0.0);
        let weight = weight_fn(&edge);
        let edge_weight = prev_weight + weight;
        fas_graph.set_edge(edge.v.clone(), edge.w.clone(), edge_weight, None);

        let v_entry = *fas_graph.node(&edge.v).unwrap();
        let w_entry = *fas_graph.node(&edge.w).unwrap();
        {
            let p = arena.payload_mut(v_entry);
            p.out_deg += weight;
            max_out = max_out.max(p.out_deg);
        }
        {
            let p = arena.payload_mut(w_entry);
            p.in_deg += weight;
            max_in = max_in.max(p.in_deg);
        }
    }

    // buckets: range(maxOut + maxIn + 3) lists; zeroIdx = maxIn + 1.
    let bucket_count = (max_out + max_in + 3.0) as usize;
    let mut buckets: Vec<List> = Vec::with_capacity(bucket_count);
    for _ in 0..bucket_count {
        buckets.push(arena.new_list());
    }
    let zero_idx = max_in as i64 + 1;

    for v in fas_graph.nodes() {
        let entry = *fas_graph.node(&v).unwrap();
        assign_bucket(&mut arena, &buckets, zero_idx, entry);
    }

    FasState {
        graph: fas_graph,
        arena,
        buckets,
        zero_idx,
    }
}

/// `assignBucket(buckets, zeroIdx, entry)`.
fn assign_bucket(arena: &mut ListArena<FasEntry>, buckets: &[List], zero_idx: i64, entry: EntryId) {
    let (in_deg, out_deg) = {
        let p = arena.payload(entry);
        (p.in_deg, p.out_deg)
    };
    if out_deg == 0.0 {
        arena.enqueue(buckets[0], entry);
    } else if in_deg == 0.0 {
        arena.enqueue(buckets[buckets.len() - 1], entry);
    } else {
        let idx = (out_deg - in_deg) as i64 + zero_idx;
        arena.enqueue(buckets[idx as usize], entry);
    }
}

#[cfg(test)]
mod tests;
