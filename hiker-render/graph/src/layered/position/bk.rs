//! Brandes–Köpf horizontal coordinate assignment — a port of
//! `dagre/lib/position/bk.ts`.
//!
//! This is the hardest dagre module: it computes x-coordinates by building four
//! extreme alignments (up/down × left/right), compacting each into a block
//! graph, then balancing them. The conformance tests (`bk-test.ts`) pin exact
//! coordinates and conflict sets, so the index arithmetic, median selection,
//! and tie-handling are ported verbatim.
//!
//! # Internal representations
//!
//! * [`Conflicts`] mirrors the TS nested map `{ [v]: { [w]: true } }`, with keys
//!   normalized so the lexicographically-smaller endpoint is the outer key
//!   (exactly as `addConflict`/`hasConflict` do).
//! * Alignment results (`root` / `align`) and x-position maps are
//!   `HashMap<NodeId, NodeId>` / `HashMap<NodeId, f64>`.
//! * The four alignments are held in [`Xss`] (fields `ul`/`ur`/`dl`/`dr`),
//!   iterated in that fixed order to preserve dagre's insertion-order
//!   determinism (`findSmallestWidthAlignment` picks the first strict minimum).

use std::collections::HashMap;

use super::super::graph::{Graph, GraphOptions, NodeId};
use super::super::types::{BorderType, EdgeLabel, GraphLabel, LabelPos, NodeLabel};
use super::super::util;

/// Nested conflict map: `conflicts[v][w] = true`, keyed with `v <= w`.
pub(crate) type Conflicts = HashMap<NodeId, HashMap<NodeId, bool>>;

/// `root` / `align` maps from `verticalAlignment`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct AlignmentResult {
    pub root: HashMap<NodeId, NodeId>,
    pub align: HashMap<NodeId, NodeId>,
}

/// The four extreme alignments, keyed by the fixed `(vert, horiz)` order
/// `ul, ur, dl, dr` to preserve dagre's iteration determinism.
#[derive(Clone, Debug, Default)]
pub(crate) struct Xss {
    pub ul: HashMap<NodeId, f64>,
    pub ur: HashMap<NodeId, f64>,
    pub dl: HashMap<NodeId, f64>,
    pub dr: HashMap<NodeId, f64>,
}

impl Xss {
    /// The four maps in dagre's insertion order (`ul, ur, dl, dr`).
    fn iter_ordered(&self) -> [(&'static str, &HashMap<NodeId, f64>); 4] {
        [
            ("ul", &self.ul),
            ("ur", &self.ur),
            ("dl", &self.dl),
            ("dr", &self.dr),
        ]
    }
}

// ── conflicts ──────────────────────────────────────────────────────────────

/// `addConflict(conflicts, v, w)` — store under the normalized key (smaller of
/// `v`/`w` outer).
pub(crate) fn add_conflict(conflicts: &mut Conflicts, v: &str, w: &str) {
    let (v, w) = if v > w { (w, v) } else { (v, w) };
    conflicts
        .entry(v.to_string())
        .or_default()
        .insert(w.to_string(), true);
}

/// `hasConflict(conflicts, v, w)` — undirected lookup.
pub(crate) fn has_conflict(conflicts: &Conflicts, v: &str, w: &str) -> bool {
    let (v, w) = if v > w { (w, v) } else { (v, w) };
    conflicts
        .get(v)
        .map(|inner| inner.contains_key(w))
        .unwrap_or(false)
}

/// `findOtherInnerSegmentNode(graph, v)` — if `v` is a dummy, the first dummy
/// predecessor.
pub(crate) fn find_other_inner_segment_node(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
    v: &str,
) -> Option<NodeId> {
    if graph.node(v).map(|n| n.dummy.is_some()).unwrap_or(false) {
        if let Some(preds) = graph.predecessors(v) {
            return preds
                .into_iter()
                .find(|u| graph.node(u).map(|n| n.dummy.is_some()).unwrap_or(false));
        }
    }
    None
}

/// `findType1Conflicts(graph, layering)`.
pub(crate) fn find_type1_conflicts(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
    layering: &[Vec<NodeId>],
) -> Conflicts {
    let mut conflicts: Conflicts = HashMap::new();

    let visit_layer = |conflicts: &mut Conflicts, prev_layer: &[NodeId], layer: &[NodeId]| {
        let mut k0: usize = 0;
        let mut scan_pos: usize = 0;
        let prev_layer_length = prev_layer.len();
        let last_node = layer.last().cloned();

        for (i, v) in layer.iter().enumerate() {
            let w = find_other_inner_segment_node(graph, v);
            let k1: usize = match &w {
                Some(w) => graph.node(w).and_then(|n| n.order).unwrap(),
                None => prev_layer_length,
            };

            if w.is_some() || Some(v.clone()) == last_node {
                for scan_node in &layer[scan_pos..=i] {
                    if let Some(preds) = graph.predecessors(scan_node) {
                        for u in preds {
                            let u_label = graph.node(&u).unwrap();
                            let u_pos = u_label.order.unwrap();
                            let u_dummy = u_label.dummy.is_some();
                            let scan_dummy =
                                graph.node(scan_node).map(|n| n.dummy.is_some()).unwrap_or(false);
                            if (u_pos < k0 || k1 < u_pos) && !(u_dummy && scan_dummy) {
                                add_conflict(conflicts, &u, scan_node);
                            }
                        }
                    }
                }
                scan_pos = i + 1;
                k0 = k1;
            }
        }
    };

    if !layering.is_empty() {
        // layering.reduce(visitLayer): accumulator starts as layering[0], each
        // step calls visitLayer(prev, cur) and returns cur.
        for idx in 1..layering.len() {
            visit_layer(&mut conflicts, &layering[idx - 1], &layering[idx]);
        }
    }

    conflicts
}

/// `findType2Conflicts(graph, layering)`.
pub(crate) fn find_type2_conflicts(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
    layering: &[Vec<NodeId>],
) -> Conflicts {
    let mut conflicts: Conflicts = HashMap::new();

    fn scan(
        graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
        conflicts: &mut Conflicts,
        south: &[NodeId],
        south_pos: usize,
        south_end: usize,
        prev_north_border: i64,
        next_north_border: i64,
    ) {
        for i in util::range_from(south_pos as i32, south_end as i32) {
            let i = i as usize;
            let v = match south.get(i) {
                Some(v) => v,
                None => continue,
            };
            if graph.node(v).map(|n| n.dummy.is_some()).unwrap_or(false) {
                if let Some(preds) = graph.predecessors(v) {
                    for u in preds {
                        let u_node = graph.node(&u).unwrap();
                        if u_node.dummy.is_some() {
                            let u_order = u_node.order.unwrap() as i64;
                            if u_order < prev_north_border || u_order > next_north_border {
                                add_conflict(conflicts, &u, v);
                            }
                        }
                    }
                }
            }
        }
    }

    let visit_layer = |conflicts: &mut Conflicts, north: &[NodeId], south: &[NodeId]| {
        let mut prev_north_pos: i64 = -1;
        let mut next_north_pos: i64 = -1;
        let mut south_pos: usize = 0;

        for (south_lookahead, v) in south.iter().enumerate() {
            if graph.node(v).and_then(|n| n.dummy) == Some(super::super::types::DummyKind::Border) {
                if let Some(predecessors) = graph.predecessors(v) {
                    if !predecessors.is_empty() {
                        let first_pred = &predecessors[0];
                        next_north_pos = graph.node(first_pred).unwrap().order.unwrap() as i64;
                        scan(
                            graph,
                            conflicts,
                            south,
                            south_pos,
                            south_lookahead,
                            prev_north_pos,
                            next_north_pos,
                        );
                        south_pos = south_lookahead;
                        prev_north_pos = next_north_pos;
                    }
                }
            }
            scan(
                graph,
                conflicts,
                south,
                south_pos,
                south.len(),
                next_north_pos,
                north.len() as i64,
            );
        }
    };

    if !layering.is_empty() {
        for idx in 1..layering.len() {
            visit_layer(&mut conflicts, &layering[idx - 1], &layering[idx]);
        }
    }

    conflicts
}

// ── vertical alignment ───────────────────────────────────────────────────────

/// `verticalAlignment(graph, layering, conflicts, neighborFn)`.
pub(crate) fn vertical_alignment(
    _graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
    layering: &[Vec<NodeId>],
    conflicts: &Conflicts,
    neighbor_fn: impl Fn(&str) -> Vec<NodeId>,
) -> AlignmentResult {
    let mut root: HashMap<NodeId, NodeId> = HashMap::new();
    let mut align: HashMap<NodeId, NodeId> = HashMap::new();
    let mut pos: HashMap<NodeId, usize> = HashMap::new();

    for layer in layering {
        for (order, v) in layer.iter().enumerate() {
            root.insert(v.clone(), v.clone());
            align.insert(v.clone(), v.clone());
            pos.insert(v.clone(), order);
        }
    }

    for layer in layering {
        let mut prev_idx: i64 = -1;
        for v in layer {
            let mut ws = neighbor_fn(v);
            if !ws.is_empty() {
                // sort by pos (undefined -> 0). Rust's sort_by is stable, matching
                // JS Array.sort stability.
                ws.sort_by(|a, b| {
                    let pa = *pos.get(a).unwrap_or(&0) as i64;
                    let pb = *pos.get(b).unwrap_or(&0) as i64;
                    pa.cmp(&pb)
                });
                let mp = (ws.len() as f64 - 1.0) / 2.0;
                let i_start = mp.floor() as usize;
                let i_end = mp.ceil() as usize;
                for i in i_start..=i_end {
                    let w = match ws.get(i) {
                        Some(w) => w.clone(),
                        None => continue,
                    };
                    let pos_w = match pos.get(&w) {
                        Some(p) => *p as i64,
                        None => continue,
                    };
                    if align.get(v) == Some(v)
                        && prev_idx < pos_w
                        && !has_conflict(conflicts, v, &w)
                    {
                        if let Some(root_w) = root.get(&w).cloned() {
                            align.insert(w.clone(), v.clone());
                            align.insert(v.clone(), root_w.clone());
                            root.insert(v.clone(), root_w);
                            prev_idx = pos_w;
                        }
                    }
                }
            }
        }
    }

    AlignmentResult { root, align }
}

// ── horizontal compaction ────────────────────────────────────────────────────

/// `horizontalCompaction(graph, layering, root, align, reverseSep)`.
pub(crate) fn horizontal_compaction(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
    layering: &[Vec<NodeId>],
    root: &HashMap<NodeId, NodeId>,
    align: &HashMap<NodeId, NodeId>,
    reverse_sep: bool,
) -> HashMap<NodeId, f64> {
    let mut xs: HashMap<NodeId, f64> = HashMap::new();
    let block_g = build_block_graph(graph, layering, root, reverse_sep);
    let border_type = if reverse_sep {
        BorderType::BorderLeft
    } else {
        BorderType::BorderRight
    };

    // `iterate` in the TS is a stack walk over the block graph: each node is
    // pushed back after its `nextNodes` so it is processed once all of them are
    // done. Because the per-element closures here borrow `xs` mutably while the
    // graph is borrowed immutably, the loop is inlined below for each pass
    // rather than factored into a helper.

    // pass1: smallest coordinates. Uses block_g edge weights (f64).
    let pass1 = |block_g: &Graph<(), (), f64>, xs: &mut HashMap<NodeId, f64>, elem: &str| {
        let in_edges = block_g.in_edges(elem, None);
        match in_edges {
            Some(in_edges) => {
                let mut acc = 0.0_f64;
                for e in &in_edges {
                    let xs_v = *xs.get(&e.v).unwrap_or(&0.0);
                    let edge_weight = *block_g.edge_by_obj(e).unwrap_or(&0.0);
                    acc = acc.max(xs_v + edge_weight);
                }
                xs.insert(elem.to_string(), acc);
            }
            None => {
                xs.insert(elem.to_string(), 0.0);
            }
        }
    };

    let pass2 = |block_g: &Graph<(), (), f64>, xs: &mut HashMap<NodeId, f64>, elem: &str| {
        let out_edges = block_g.out_edges(elem, None);
        let mut min = f64::INFINITY;
        if let Some(out_edges) = out_edges {
            for e in &out_edges {
                let xs_w = *xs.get(&e.w).unwrap_or(&0.0);
                let edge_weight = *block_g.edge_by_obj(e).unwrap_or(&0.0);
                min = min.min(xs_w - edge_weight);
            }
        }
        let node_border = graph.node(elem).and_then(|n| n.border_type);
        if min != f64::INFINITY && node_border != Some(border_type) {
            let cur = *xs.get(elem).unwrap_or(&0.0);
            xs.insert(elem.to_string(), cur.max(min));
        }
    };

    // Closures capturing `xs` mutably can't be passed into `iterate` alongside
    // another borrow, so inline the iterate loop twice with the right closures.
    {
        let predecessors_wrapper =
            |bg: &Graph<(), (), f64>, elem: &str| bg.predecessors(elem).unwrap_or_default();
        let mut stack: Vec<NodeId> = block_g.nodes();
        let mut visited: HashMap<NodeId, bool> = HashMap::new();
        let mut elem = stack.pop();
        while let Some(e) = elem {
            if *visited.get(&e).unwrap_or(&false) {
                pass1(&block_g, &mut xs, &e);
            } else {
                visited.insert(e.clone(), true);
                stack.push(e.clone());
                for next_elem in predecessors_wrapper(&block_g, &e) {
                    stack.push(next_elem);
                }
            }
            elem = stack.pop();
        }
    }
    {
        let successors_wrapper =
            |bg: &Graph<(), (), f64>, elem: &str| bg.successors(elem).unwrap_or_default();
        let mut stack: Vec<NodeId> = block_g.nodes();
        let mut visited: HashMap<NodeId, bool> = HashMap::new();
        let mut elem = stack.pop();
        while let Some(e) = elem {
            if *visited.get(&e).unwrap_or(&false) {
                pass2(&block_g, &mut xs, &e);
            } else {
                visited.insert(e.clone(), true);
                stack.push(e.clone());
                for next_elem in successors_wrapper(&block_g, &e) {
                    stack.push(next_elem);
                }
            }
            elem = stack.pop();
        }
    }
    // Assign x coordinates to all nodes (iterate align keys in insertion order
    // is irrelevant — each v reads xs[root[v]]).
    for v in align.keys() {
        if let Some(root_v) = root.get(v) {
            let x = *xs.get(root_v).unwrap_or(&0.0);
            xs.insert(v.clone(), x);
        }
    }

    xs
}

/// `buildBlockGraph(graph, layering, root, reverseSep)` — a non-compound
/// directed graph whose nodes are block roots and edges carry the required
/// separation (max over consecutive pairs).
fn build_block_graph(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
    layering: &[Vec<NodeId>],
    root: &HashMap<NodeId, NodeId>,
    reverse_sep: bool,
) -> Graph<(), (), f64> {
    let mut block_graph: Graph<(), (), f64> = Graph::new(GraphOptions::default());
    let graph_label = graph.graph().expect("buildBlockGraph: graph label unset");
    let nodesep = graph_label.nodesep.unwrap_or(0.0);
    let edgesep = graph_label.edgesep.unwrap_or(0.0);

    for layer in layering {
        let mut u: Option<NodeId> = None;
        for v in layer {
            if let Some(v_root) = root.get(v) {
                block_graph.ensure_node(v_root.clone());
                if let Some(u) = &u {
                    if let Some(u_root) = root.get(u) {
                        let prev_max = block_graph.edge(u_root, v_root, None).copied();
                        let s = sep(nodesep, edgesep, reverse_sep, graph, v, u);
                        block_graph.set_edge(
                            u_root.clone(),
                            v_root.clone(),
                            s.max(prev_max.unwrap_or(0.0)),
                            None,
                        );
                    }
                }
                u = Some(v.clone());
            }
        }
    }

    block_graph
}

// ── alignment combination & balancing ────────────────────────────────────────

/// `findSmallestWidthAlignment(graph, xss)` — the alignment with the smallest
/// overall width (returns a clone). Iterates `ul, ur, dl, dr` and keeps the
/// first strict minimum.
pub(crate) fn find_smallest_width_alignment(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
    xss: &Xss,
) -> HashMap<NodeId, f64> {
    let mut current_min = f64::INFINITY;
    let mut current_xs: Option<&HashMap<NodeId, f64>> = None;

    for (_name, xs) in xss.iter_ordered() {
        let mut max = f64::NEG_INFINITY;
        let mut min = f64::INFINITY;
        for (v, x) in xs {
            let half_width = width(graph, v) / 2.0;
            max = max.max(x + half_width);
            min = min.min(x - half_width);
        }
        let new_min = max - min;
        if new_min < current_min {
            current_min = new_min;
            current_xs = Some(xs);
        }
    }

    current_xs.cloned().unwrap_or_default()
}

/// `alignCoordinates(xss, alignTo)` — shift each alignment so left-biased ones
/// share the min and right-biased ones share the max of `alignTo`.
pub(crate) fn align_coordinates(xss: &mut Xss, align_to: &HashMap<NodeId, f64>) {
    let align_to_min = min_of(align_to.values().copied());
    let align_to_max = max_of(align_to.values().copied());

    // `xs === alignTo` skip: alignTo is a clone of the smallest-width alignment,
    // so identity can't be detected by pointer. dagre's skip only avoids a
    // redundant no-op shift (delta would be 0 for that map), so emulate it by
    // checking delta != 0 — which we already do below. We therefore process all
    // four maps; the delta==0 guard reproduces the same result.
    let order: [&'static str; 4] = ["ul", "ur", "dl", "dr"];
    for name in order {
        let horiz_is_left = name.ends_with('l');
        let xs = match name {
            "ul" => &mut xss.ul,
            "ur" => &mut xss.ur,
            "dl" => &mut xss.dl,
            "dr" => &mut xss.dr,
            _ => unreachable!(),
        };
        if xs.is_empty() {
            continue;
        }
        let delta = if horiz_is_left {
            align_to_min - min_of(xs.values().copied())
        } else {
            align_to_max - max_of(xs.values().copied())
        };
        if delta != 0.0 {
            for x in xs.values_mut() {
                *x += delta;
            }
        }
    }
}

/// `balance(xss, align)` — per node, the average of the middle two of the four
/// sorted alignment positions (or the chosen alignment's value if `align` set).
pub(crate) fn balance(
    xss: &Xss,
    align: Option<super::super::types::Align>,
) -> HashMap<NodeId, f64> {
    use super::super::types::Align;
    let mut result: HashMap<NodeId, f64> = HashMap::new();

    for v in xss.ul.keys() {
        if let Some(a) = align {
            let alignment = match a {
                Align::Ul => &xss.ul,
                Align::Ur => &xss.ur,
                Align::Dl => &xss.dl,
                Align::Dr => &xss.dr,
            };
            if let Some(val) = alignment.get(v) {
                result.insert(v.clone(), *val);
                continue;
            }
        }
        let mut vals = [
            *xss.ul.get(v).unwrap_or(&0.0),
            *xss.ur.get(v).unwrap_or(&0.0),
            *xss.dl.get(v).unwrap_or(&0.0),
            *xss.dr.get(v).unwrap_or(&0.0),
        ];
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
        result.insert(v.clone(), (vals[1] + vals[2]) / 2.0);
    }

    result
}

/// `positionX(graph)` — orchestrates the four alignments and balances them.
pub(crate) fn position_x(
    graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
) -> HashMap<NodeId, f64> {
    let layering = util::build_layer_matrix(graph);

    let mut conflicts = find_type1_conflicts(graph, &layering);
    let type2 = find_type2_conflicts(graph, &layering);
    // Object.assign(type1, type2): merge type2 into type1 (inner maps replaced
    // wholesale per outer key, matching JS shallow assign).
    for (k, inner) in type2 {
        conflicts.insert(k, inner);
    }

    let mut xss = Xss::default();

    for vert in ["u", "d"] {
        // adjustedLayering for "u" is layering; for "d" it is reversed layer order.
        let base: Vec<Vec<NodeId>> = if vert == "u" {
            layering.clone()
        } else {
            let mut r = layering.clone();
            r.reverse();
            r
        };
        for horiz in ["l", "r"] {
            let adjusted: Vec<Vec<NodeId>> = if horiz == "r" {
                base.iter()
                    .map(|inner| {
                        let mut r = inner.clone();
                        r.reverse();
                        r
                    })
                    .collect()
            } else {
                base.clone()
            };

            let align_result = vertical_alignment(graph, &adjusted, &conflicts, |v| {
                let result = if vert == "u" {
                    graph.predecessors(v)
                } else {
                    graph.successors(v)
                };
                result.unwrap_or_default()
            });

            let mut xs = horizontal_compaction(
                graph,
                &adjusted,
                &align_result.root,
                &align_result.align,
                horiz == "r",
            );
            if horiz == "r" {
                for x in xs.values_mut() {
                    *x = -*x;
                }
            }
            match (vert, horiz) {
                ("u", "l") => xss.ul = xs,
                ("u", "r") => xss.ur = xs,
                ("d", "l") => xss.dl = xs,
                ("d", "r") => xss.dr = xs,
                _ => unreachable!(),
            }
        }
    }

    let smallest_width = find_smallest_width_alignment(graph, &xss);
    align_coordinates(&mut xss, &smallest_width);
    balance(&xss, graph.graph().and_then(|g| g.align))
}

// ── separation & width helpers ───────────────────────────────────────────────

/// `sep(nodeSep, edgeSep, reverseSep)(g, v, w)` — half-spacing between two
/// consecutive nodes. Dummies use `edgeSep`, real nodes `nodeSep`; `labelpos`
/// shifts the spacing and `reverseSep` flips that shift.
fn sep(
    node_sep: f64,
    edge_sep: f64,
    reverse_sep: bool,
    g: &Graph<GraphLabel, NodeLabel, EdgeLabel>,
    v: &str,
    w: &str,
) -> f64 {
    let v_label = g.node(v).unwrap();
    let w_label = g.node(w).unwrap();
    let mut sum = 0.0_f64;
    let mut delta: f64;

    sum += v_label.width / 2.0;
    delta = match v_label.label_pos {
        Some(LabelPos::L) => -v_label.width / 2.0,
        Some(LabelPos::R) => v_label.width / 2.0,
        _ => 0.0,
    };
    if delta != 0.0 {
        sum += if reverse_sep { delta } else { -delta };
    }

    sum += (if v_label.dummy.is_some() { edge_sep } else { node_sep }) / 2.0;
    sum += (if w_label.dummy.is_some() { edge_sep } else { node_sep }) / 2.0;

    sum += w_label.width / 2.0;
    delta = match w_label.label_pos {
        Some(LabelPos::L) => w_label.width / 2.0,
        Some(LabelPos::R) => -w_label.width / 2.0,
        _ => 0.0,
    };
    if delta != 0.0 {
        sum += if reverse_sep { delta } else { -delta };
    }

    sum
}

/// `width(graph, v)` — the node's width.
fn width(graph: &Graph<GraphLabel, NodeLabel, EdgeLabel>, v: &str) -> f64 {
    graph.node(v).map(|n| n.width).unwrap_or(0.0)
}

fn min_of(it: impl Iterator<Item = f64>) -> f64 {
    it.fold(f64::INFINITY, f64::min)
}

fn max_of(it: impl Iterator<Item = f64>) -> f64 {
    it.fold(f64::NEG_INFINITY, f64::max)
}
