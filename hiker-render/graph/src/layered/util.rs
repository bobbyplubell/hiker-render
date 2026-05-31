//! dagre layout helpers — a port of `dagre/lib/util.ts`.
//!
//! Test names in the `#[cfg(test)]` module mirror `dagre/test/util-test.ts` so
//! a reviewer can diff against the original oracle.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use super::graph::{Graph, GraphOptions, NodeId};
use super::types::{
    DummyKind, EdgeLabel, GraphLabel, NodeLabel, PartitionResult, Point,
};

/// Module-global id counter, mirroring dagre's `let idCounter = 0`. Sequential
/// and deterministic across a run (no randomness / time).
static ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// `uniqueId(prefix)` — `prefix + (++idCounter)`.
pub fn unique_id(prefix: &str) -> String {
    let id = ID_COUNTER.fetch_add(1, AtomicOrdering::SeqCst) + 1;
    format!("{prefix}{id}")
}

/// Adds a dummy node to the graph and returns its id.
///
/// Mirrors dagre's `addDummyNode`: the requested `name` is used directly if
/// free, otherwise `uniqueId(name)` is appended-numbered until a free id is
/// found. The `dummy` field of `attrs` is set to `kind`.
pub fn add_dummy_node(
    graph: &mut Graph<GraphLabel, NodeLabel, EdgeLabel>,
    kind: DummyKind,
    mut attrs: NodeLabel,
    name: &str,
) -> NodeId {
    let mut v = name.to_string();
    while graph.has_node(&v) {
        v = unique_id(name);
    }
    attrs.dummy = Some(kind);
    graph.set_node(v.clone(), attrs);
    v
}

/// Returns a new (simple, non-multigraph, directed) graph with only simple
/// edges. Multi-edges are aggregated: weights summed, `minlen` maxed.
pub fn simplify(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
) -> Graph<GraphLabel, NodeLabel, EdgeLabel> {
    let mut simplified: Graph<GraphLabel, NodeLabel, EdgeLabel> =
        Graph::new(GraphOptions::default());
    if let Some(g) = graph.graph() {
        simplified.set_graph(g.clone());
    }
    for v in graph.nodes() {
        match graph.node(&v) {
            Some(label) => {
                simplified.set_node(v, label.clone());
            }
            None => {
                simplified.set_node_none(v);
            }
        }
    }
    for e in graph.edges() {
        let existing = simplified.edge(&e.v, &e.w, None);
        let (sw, sm) = match existing {
            Some(l) => (l.weight.unwrap_or(0.0), l.minlen.unwrap_or(1)),
            None => (0.0, 1),
        };
        let label = graph.edge_by_obj(&e).cloned().unwrap_or_default();
        let merged = EdgeLabel {
            weight: Some(sw + label.weight.unwrap_or(0.0)),
            minlen: Some(sm.max(label.minlen.unwrap_or(1))),
            ..Default::default()
        };
        simplified.set_edge(e.v.clone(), e.w.clone(), merged, None);
    }
    simplified
}

/// Returns a copy of `graph` with the compound structure stripped: only leaf
/// nodes (those with no children) are copied; all edges are copied. The
/// multigraph flag is preserved.
pub fn as_non_compound_graph(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
) -> Graph<GraphLabel, NodeLabel, EdgeLabel> {
    let mut simplified: Graph<GraphLabel, NodeLabel, EdgeLabel> = Graph::new(GraphOptions {
        directed: true,
        multigraph: graph.is_multigraph(),
        compound: false,
    });
    if let Some(g) = graph.graph() {
        simplified.set_graph(g.clone());
    }
    for v in graph.nodes() {
        if graph.children(&v).is_empty() {
            match graph.node(&v) {
                Some(label) => {
                    simplified.set_node(v, label.clone());
                }
                None => {
                    simplified.set_node_none(v);
                }
            }
        }
    }
    for e in graph.edges() {
        match graph.edge_by_obj(&e) {
            Some(label) => {
                simplified.set_edge(e.v.clone(), e.w.clone(), label.clone(), e.name.as_deref());
            }
            None => {
                simplified.set_edge_none(e.v.clone(), e.w.clone(), e.name.as_deref());
            }
        }
    }
    simplified
}

/// Maps each node to its successors with summed edge weights.
pub fn successor_weights(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
) -> HashMap<NodeId, HashMap<NodeId, f64>> {
    let mut out: HashMap<NodeId, HashMap<NodeId, f64>> = HashMap::new();
    for v in graph.nodes() {
        let mut sucs: HashMap<NodeId, f64> = HashMap::new();
        if let Some(edges) = graph.out_edges(&v, None) {
            for e in edges {
                let w = graph.edge_by_obj(&e).and_then(|l| l.weight).unwrap_or(0.0);
                *sucs.entry(e.w.clone()).or_insert(0.0) += w;
            }
        }
        out.insert(v, sucs);
    }
    out
}

/// Maps each node to its predecessors with summed edge weights.
pub fn predecessor_weights(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
) -> HashMap<NodeId, HashMap<NodeId, f64>> {
    let mut out: HashMap<NodeId, HashMap<NodeId, f64>> = HashMap::new();
    for v in graph.nodes() {
        let mut preds: HashMap<NodeId, f64> = HashMap::new();
        if let Some(edges) = graph.in_edges(&v, None) {
            for e in edges {
                let w = graph.edge_by_obj(&e).and_then(|l| l.weight).unwrap_or(0.0);
                *preds.entry(e.v.clone()).or_insert(0.0) += w;
            }
        }
        out.insert(v, preds);
    }
    out
}

/// A rectangle for [`intersect_rect`] — center `(x, y)` with `width`/`height`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Finds where a line from `point` toward the rectangle's center crosses the
/// rectangle's border.
///
/// # Panics
/// Panics if `point` is exactly at the rectangle's center (matching dagre's
/// thrown error).
pub fn intersect_rect(rect: &Rect, point: &Point) -> Point {
    let x = rect.x;
    let y = rect.y;

    let dx = point.x - x;
    let dy = point.y - y;
    let mut w = rect.width / 2.0;
    let mut h = rect.height / 2.0;

    if dx == 0.0 && dy == 0.0 {
        panic!("Not possible to find intersection inside of the rectangle");
    }

    let (sx, sy);
    if dy.abs() * w > dx.abs() * h {
        // Intersection is top or bottom of rect.
        if dy < 0.0 {
            h = -h;
        }
        sx = h * dx / dy;
        sy = h;
    } else {
        // Intersection is left or right of rect.
        if dx < 0.0 {
            w = -w;
        }
        sx = w;
        sy = w * dy / dx;
    }

    Point::new(x + sx, y + sy)
}

/// `intersectRect` taking a [`NodeLabel`] (whose `x`/`y`/`width`/`height` are
/// used) as the rect, matching the TS signature `intersectRect(rect:
/// NodeLabel, point)`.
pub fn intersect_rect_node(rect: &NodeLabel, point: &Point) -> Point {
    intersect_rect(
        &Rect {
            x: rect.x.expect("intersectRect: rect.x is undefined"),
            y: rect.y.expect("intersectRect: rect.y is undefined"),
            width: rect.width,
            height: rect.height,
        },
        point,
    )
}

/// Given nodes assigned `rank` and `order`, produces a matrix of node ids
/// indexed `[rank][order]`.
pub fn build_layer_matrix(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
) -> Vec<Vec<NodeId>> {
    let layer_count = (max_rank(graph) + 1).max(0) as usize;
    let mut layering: Vec<Vec<NodeId>> = vec![Vec::new(); layer_count];
    for v in graph.nodes() {
        if let Some(node) = graph.node(&v) {
            if let Some(rank) = node.rank {
                if rank >= 0 {
                    let rank = rank as usize;
                    if rank >= layering.len() {
                        layering.resize(rank + 1, Vec::new());
                    }
                    let order = node.order.unwrap_or(0);
                    if order >= layering[rank].len() {
                        layering[rank].resize(order + 1, String::new());
                    }
                    layering[rank][order] = v;
                }
            }
        }
    }
    layering
}

/// Adjusts ranks so all are `>= 0` and at least one is `0`.
pub fn normalize_ranks(graph: &mut Graph<GraphLabel, NodeLabel, EdgeLabel>) {
    let ranks: Vec<i32> = graph
        .nodes()
        .iter()
        .map(|v| graph.node(v).and_then(|n| n.rank).unwrap_or(i32::MAX))
        .collect();
    let min = apply_with_chunking_min(&ranks);
    for v in graph.nodes() {
        if let Some(node) = graph.node_mut(&v) {
            if let Some(r) = node.rank {
                node.rank = Some(r - min);
            }
        }
    }
}

/// Removes border ranks that contain no nodes, compacting per `nodeRankFactor`.
pub fn remove_empty_ranks(graph: &mut Graph<GraphLabel, NodeLabel, EdgeLabel>) {
    let ranks: Vec<i32> = graph
        .nodes()
        .iter()
        .filter_map(|v| graph.node(v).and_then(|n| n.rank))
        .collect();
    let offset = apply_with_chunking_min(&ranks);

    // layers[rank - offset] = Some(vec of node ids); None means "no node here".
    let mut layers: Vec<Option<Vec<NodeId>>> = Vec::new();
    for v in graph.nodes() {
        let rank = match graph.node(&v).and_then(|n| n.rank) {
            Some(r) => r - offset,
            None => continue,
        };
        let idx = rank as usize;
        if idx >= layers.len() {
            layers.resize(idx + 1, None);
        }
        layers[idx].get_or_insert_with(Vec::new).push(v);
    }

    let node_rank_factor = graph
        .graph()
        .and_then(|g| g.node_rank_factor)
        .unwrap_or(0);

    let mut delta: i32 = 0;
    for (i, vs) in layers.iter().enumerate() {
        match vs {
            None if node_rank_factor != 0 && (i as i32) % node_rank_factor != 0 => {
                delta -= 1;
            }
            Some(vs) if delta != 0 => {
                for v in vs {
                    if let Some(node) = graph.node_mut(v) {
                        if let Some(r) = node.rank {
                            node.rank = Some(r + delta);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Adds a border node (a 0-size dummy with `dummy = border`). When both `rank`
/// and `order` are supplied they are set on the node.
pub fn add_border_node(
    graph: &mut Graph<GraphLabel, NodeLabel, EdgeLabel>,
    prefix: &str,
    rank: Option<i32>,
    order: Option<usize>,
) -> NodeId {
    let mut node = NodeLabel {
        width: 0.0,
        height: 0.0,
        ..Default::default()
    };
    if rank.is_some() && order.is_some() {
        node.rank = rank;
        node.order = order;
    }
    add_dummy_node(graph, DummyKind::Border, node, prefix)
}

// dagre splits arrays above this threshold to avoid JS's argument-spread limit
// (`fn(...args)`). Rust's iterator `fold` has no such limit, so chunking is a
// no-op for us; the threshold is retained only as documentation of intent.
#[allow(dead_code)]
const CHUNKING_THRESHOLD: usize = 65535;

/// `applyWithChunking(Math.min, ...)`. Empty input yields `i32::MAX` (matching
/// JS `Math.min()` → `Infinity`, used as the identity for `min`).
pub fn apply_with_chunking_min(args: &[i32]) -> i32 {
    args.iter().copied().fold(i32::MAX, i32::min)
}

/// `applyWithChunking(Math.max, ...)`. Empty input yields `i32::MIN` (matching
/// JS `Math.max()` → `-Infinity`, used as the identity for `max`).
pub fn apply_with_chunking_max(args: &[i32]) -> i32 {
    args.iter().copied().fold(i32::MIN, i32::max)
}

/// The maximum `rank` over all nodes. Nodes without a rank contribute the
/// minimum (matching the TS `Number.MIN_VALUE` placeholder, which is the
/// smallest positive value — here we follow the algorithmic intent and ignore
/// rank-less nodes by giving them a sentinel that never wins).
pub fn max_rank(graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>) -> i32 {
    // Note: dagre uses Number.MIN_VALUE (≈ 5e-324, a tiny *positive* number) as
    // the placeholder for rank-less nodes. With integer ranks the only effect
    // is on an all-rankless graph, where dagre returns that tiny positive value
    // and `buildLayerMatrix` then allocates a single empty layer. We mirror
    // that single-empty-layer behaviour by treating the placeholder as 0 here
    // (so an all-rankless graph yields maxRank 0, hence one layer), while any
    // present rank dominates.
    let ranks: Vec<i32> = graph
        .nodes()
        .iter()
        .map(|v| graph.node(v).and_then(|n| n.rank).unwrap_or(0))
        .collect();
    if ranks.is_empty() {
        // Math.max() === -Infinity; buildLayerMatrix(+1) → 0 layers.
        return i32::MIN;
    }
    apply_with_chunking_max(&ranks)
}

/// Partitions `collection` into `lhs` (predicate true) and `rhs` (false).
pub fn partition<T, F: Fn(&T) -> bool>(collection: Vec<T>, f: F) -> PartitionResult<T> {
    let mut result = PartitionResult {
        lhs: Vec::new(),
        rhs: Vec::new(),
    };
    for value in collection {
        if f(&value) {
            result.lhs.push(value);
        } else {
            result.rhs.push(value);
        }
    }
    result
}

/// `time(name, fn)` — runs `fn` and returns its value. The timing log is a
/// no-op here (deterministic, no wall-clock output in the layout pipeline).
pub fn time<T, F: FnOnce() -> T>(_name: &str, f: F) -> T {
    f()
}

/// `notime(name, fn)` — runs `fn` and returns its value.
pub fn notime<T, F: FnOnce() -> T>(_name: &str, f: F) -> T {
    f()
}

/// `range(limit)` — `[0, limit)`.
pub fn range(limit: i32) -> Vec<i32> {
    range_step(0, limit, 1)
}

/// `range(start, limit)` — `[start, limit)` with step 1.
pub fn range_from(start: i32, limit: i32) -> Vec<i32> {
    range_step(start, limit, 1)
}

/// `range(start, limit, step)`.
pub fn range_step(start: i32, limit: i32, step: i32) -> Vec<i32> {
    let mut out = Vec::new();
    if step == 0 {
        return out;
    }
    let mut i = start;
    if step < 0 {
        while limit < i {
            out.push(i);
            i += step;
        }
    } else {
        while i < limit {
            out.push(i);
            i += step;
        }
    }
    out
}

/// `pick(source, keys)` — selects entries of `source` whose key is in `keys`.
pub fn pick<V: Clone>(source: &HashMap<String, V>, keys: &[&str]) -> HashMap<String, V> {
    let mut dest = HashMap::new();
    for key in keys {
        if let Some(v) = source.get(*key) {
            dest.insert((*key).to_string(), v.clone());
        }
    }
    dest
}

/// `mapValues(obj, func)` — maps each value of `obj` through `func(value, key)`.
pub fn map_values<T, R, F: Fn(&T, &str) -> R>(
    obj: &HashMap<String, T>,
    func: F,
) -> HashMap<String, R> {
    obj.iter()
        .map(|(k, v)| (k.clone(), func(v, k)))
        .collect()
}

/// `zipObject(props, values)` — builds a map from parallel key/value slices.
pub fn zip_object<V: Clone>(props: &[NodeId], values: &[V]) -> HashMap<NodeId, V> {
    props
        .iter()
        .zip(values.iter())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// graphlib's compound-root sentinel id, re-exported for dagre modules.
pub const GRAPH_NODE: &str = "\u{0}";

#[cfg(test)]
mod tests;
