//! The layout orchestrator — a port of `dagre/lib/layout.ts`.
//!
//! [`layout`] is the public entry point: it builds a fresh *layout graph* from
//! the input graph (copying only whitelisted attributes and applying dagre's
//! defaults), runs the 26-pass Sugiyama pipeline ([`run_layout`]) on it, then
//! copies the final coordinates back onto the input graph
//! ([`update_input_graph`]).
//!
//! # Fidelity notes
//!
//! * **`canonicalize`** (the TS attr-key lowercaser) is a no-op here: our
//!   labels are typed structs, not string-keyed objects, so there are no
//!   mixed-case keys to normalise. The behaviour the case-insensitivity test
//!   exercises (`nodesep` honoured) is intrinsic to the typed fields.
//! * **`rank(asNonCompoundGraph(g))`**: in dagre the non-compound graph shares
//!   node-label object *references* with `g`, so ranks written there appear on
//!   `g`. Our [`super::util::as_non_compound_graph`] *copies* labels, so we rank
//!   the copy and then copy the resulting `rank` back onto the matching nodes of
//!   `g` (see [`rank_pass`]).
//! * **self-edges** are stashed in a local `HashMap` (keyed by node id) between
//!   [`remove_self_edges`] and [`insert_self_edges`] rather than on a
//!   `selfEdges` field of the node label, since the stash only needs to live
//!   across `run_layout`.
//! * **edge-label proxies / self-edge dummies** carry the original [`Edge`] in
//!   `NodeLabel::edge_obj` and (for self-edges) the [`EdgeLabel`] in
//!   `NodeLabel::edge_label`.

use std::collections::HashMap;

use super::graph::{Edge, Graph, GraphOptions};
use super::types::{
    DagreGraph, DummyKind, EdgeLabel, GraphLabel, LabelPos, NodeLabel, Point, RankDir,
};
use super::util::{self, Rect};
use super::{
    acyclic, add_border_segments, coordinate_system, nesting_graph, normalize, order,
    parent_dummy_chains, position, rank,
};

/// Options for [`layout`] — TS `LayoutOptions` (the subset the pipeline reads).
#[derive(Clone, Debug, Default)]
pub struct LayoutOptions {
    /// `debugTiming` — emit per-pass timing. A no-op here (timing is reduced to
    /// direct calls), kept for signature fidelity.
    pub debug_timing: bool,
    /// `disableOptimalOrderHeuristic` — forwarded to [`order::order`].
    pub disable_optimal_order_heuristic: bool,
}

/// `layout(g)` — lay out the graph in place, with default options.
///
/// After this returns, every node label carries `x`/`y` (and `width`/`height`
/// for compound nodes), every edge label carries `points` (and `x`/`y` when it
/// has a label), and the graph label carries `width`/`height`.
pub fn layout(graph: &mut DagreGraph) {
    layout_with_opts(graph, &LayoutOptions::default());
}

/// `layout(g, opts)` — lay out the graph in place with explicit options.
pub fn layout_with_opts(input_graph: &mut DagreGraph, opts: &LayoutOptions) {
    let mut layout_graph = build_layout_graph(input_graph);
    run_layout(&mut layout_graph, opts);
    update_input_graph(input_graph, &layout_graph);
}

/// The 26-pass pipeline, in the exact order of `dagre`'s `runLayout`.
fn run_layout(g: &mut DagreGraph, opts: &LayoutOptions) {
    make_space_for_edge_labels(g);
    let self_edges = remove_self_edges(g);
    acyclic::run(g);
    nesting_graph::run(g);
    rank_pass(g);
    inject_edge_label_proxies(g);
    util::remove_empty_ranks(g);
    nesting_graph::cleanup(g);
    util::normalize_ranks(g);
    assign_rank_min_max(g);
    remove_edge_label_proxies(g);
    normalize::run(g);
    parent_dummy_chains::parent_dummy_chains(g);
    add_border_segments::add_border_segments(g);
    order::order(
        g,
        &order::OrderOptions {
            disable_optimal_order_heuristic: opts.disable_optimal_order_heuristic,
            constraints: Vec::new(),
        },
    );
    insert_self_edges(g, self_edges);
    coordinate_system::adjust(g);
    position::position(g);
    position_self_edges(g);
    remove_border_nodes(g);
    normalize::undo(g);
    fixup_edge_label_coords(g);
    coordinate_system::undo(g);
    translate_graph(g);
    assign_node_intersects(g);
    reverse_points_for_reversed_edges(g);
    acyclic::undo(g);
}

/// Copies final layout information from the layout graph back to the input
/// graph (the whitelisted output attributes).
fn update_input_graph(input_graph: &mut DagreGraph, layout_graph: &DagreGraph) {
    for v in input_graph.nodes() {
        let layout_label = match layout_graph.node(&v) {
            Some(l) => l.clone(),
            None => continue,
        };
        let has_children = !layout_graph.children(&v).is_empty();
        if let Some(input_label) = input_graph.node_mut(&v) {
            input_label.x = layout_label.x;
            input_label.y = layout_label.y;
            input_label.order = layout_label.order;
            input_label.rank = layout_label.rank;
            if has_children {
                input_label.width = layout_label.width;
                input_label.height = layout_label.height;
            }
        }
    }

    for e in input_graph.edges() {
        let layout_label = match layout_graph.edge_by_obj(&e) {
            Some(l) => l.clone(),
            None => continue,
        };
        if let Some(input_label) = input_graph.edge_mut(&e.v, &e.w, e.name.as_deref()) {
            input_label.points = layout_label.points.clone();
            if layout_label.x.is_some() {
                input_label.x = layout_label.x;
                input_label.y = layout_label.y;
            }
        }
    }

    let (w, h) = layout_graph
        .graph()
        .map(|g| (g.width, g.height))
        .unwrap_or((None, None));
    if let Some(g) = input_graph.graph_mut() {
        g.width = w;
        g.height = h;
    }
}

// ── buildLayoutGraph ───────────────────────────────────────────────────────

/// Graph-label defaults: `ranksep 50`, `edgesep 20`, `nodesep 50`, `rankdir TB`.
/// (`rankalign: "center"` in TS — represented by leaving `rank_align` as the
/// pipeline's own default, which the ranker reads.)
fn build_layout_graph(input_graph: &DagreGraph) -> DagreGraph {
    let mut g: DagreGraph = Graph::new(GraphOptions {
        directed: true,
        multigraph: true,
        compound: true,
    });

    // Graph label: defaults overlaid by the whitelisted attrs from the input.
    let src = input_graph.graph();
    let label = GraphLabel {
        // graphNumAttrs: nodesep, edgesep, ranksep, marginx, marginy
        nodesep: Some(src.and_then(|s| s.nodesep).unwrap_or(50.0)),
        edgesep: Some(src.and_then(|s| s.edgesep).unwrap_or(20.0)),
        ranksep: Some(src.and_then(|s| s.ranksep).unwrap_or(50.0)),
        marginx: src.and_then(|s| s.marginx),
        marginy: src.and_then(|s| s.marginy),
        // graphAttrs: acyclicer, ranker, rankdir, align, rankalign
        rankdir: Some(src.and_then(|s| s.rankdir).unwrap_or(RankDir::Tb)),
        align: src.and_then(|s| s.align),
        rank_align: src.and_then(|s| s.rank_align),
        acyclicer: src.and_then(|s| s.acyclicer),
        ranker: src.and_then(|s| s.ranker),
        ..Default::default()
    };
    g.set_graph(label);

    // Nodes: nodeNumAttrs (width, height, rank) + defaults width/height 0.
    for v in input_graph.nodes() {
        let src = input_graph.node(&v);
        let new_node = NodeLabel {
            width: src.map(|n| n.width).unwrap_or(0.0),
            height: src.map(|n| n.height).unwrap_or(0.0),
            rank: src.and_then(|n| n.rank),
            ..Default::default()
        };
        g.set_node(v.clone(), new_node);
        if let Some(parent) = input_graph.parent(&v) {
            g.set_parent(v, parent);
        }
    }

    // Edges: edgeNumAttrs (minlen, weight, width, height, labeloffset) +
    // edgeAttrs (labelpos) + defaults.
    for e in input_graph.edges() {
        let src = input_graph.edge_by_obj(&e);
        let new_edge = EdgeLabel {
            minlen: Some(src.and_then(|x| x.minlen).unwrap_or(1)),
            weight: Some(src.and_then(|x| x.weight).unwrap_or(1.0)),
            width: Some(src.and_then(|x| x.width).unwrap_or(0.0)),
            height: Some(src.and_then(|x| x.height).unwrap_or(0.0)),
            label_offset: Some(src.and_then(|x| x.label_offset).unwrap_or(10.0)),
            label_pos: Some(src.and_then(|x| x.label_pos).unwrap_or(LabelPos::R)),
            ..Default::default()
        };
        g.set_edge(e.v.clone(), e.w.clone(), new_edge, e.name.as_deref());
    }

    g
}

// ── makeSpaceForEdgeLabels ───────────────────────────────────────────────────

fn make_space_for_edge_labels(g: &mut DagreGraph) {
    let rankdir = g.graph().and_then(|gl| gl.rankdir);
    if let Some(gl) = g.graph_mut() {
        gl.ranksep = Some(gl.ranksep.unwrap_or(0.0) / 2.0);
    }
    let is_vertical = matches!(rankdir, Some(RankDir::Tb) | Some(RankDir::Bt));
    for e in g.edges() {
        if let Some(edge) = g.edge_mut(&e.v, &e.w, e.name.as_deref()) {
            edge.minlen = Some(edge.minlen.unwrap_or(1) * 2);
            if edge.label_pos != Some(LabelPos::C) {
                let offset = edge.label_offset.unwrap_or(0.0);
                if is_vertical {
                    edge.width = Some(edge.width.unwrap_or(0.0) + offset);
                } else {
                    edge.height = Some(edge.height.unwrap_or(0.0) + offset);
                }
            }
        }
    }
}

// ── rank (the asNonCompoundGraph ranks write-back) ──────────────────────────

/// `rank(asNonCompoundGraph(g))`. We rank a non-compound *copy* (label copies,
/// not shared refs), then copy each ranked node's `rank` back onto `g`.
fn rank_pass(g: &mut DagreGraph) {
    let mut nc = util::as_non_compound_graph(g);
    rank::rank(&mut nc);
    for v in nc.nodes() {
        if let Some(rank) = nc.node(&v).and_then(|n| n.rank) {
            if let Some(node) = g.node_mut(&v) {
                node.rank = Some(rank);
            }
        }
    }
}

// ── injectEdgeLabelProxies / removeEdgeLabelProxies ─────────────────────────

fn inject_edge_label_proxies(g: &mut DagreGraph) {
    for e in g.edges() {
        let edge = g.edge_by_obj(&e);
        let has_label = edge
            .map(|x| x.width.unwrap_or(0.0) != 0.0 && x.height.unwrap_or(0.0) != 0.0)
            .unwrap_or(false);
        if has_label {
            let v_rank = g.node(&e.v).and_then(|n| n.rank).unwrap_or(0);
            let w_rank = g.node(&e.w).and_then(|n| n.rank).unwrap_or(0);
            let label = NodeLabel {
                rank: Some((w_rank - v_rank) / 2 + v_rank),
                edge_obj: Some(e.clone()),
                ..Default::default()
            };
            util::add_dummy_node(g, DummyKind::EdgeProxy, label, "_ep");
        }
    }
}

fn assign_rank_min_max(g: &mut DagreGraph) {
    let mut max_rank = 0;
    // Collect first to avoid borrow conflicts; the values come from border
    // nodes whose ranks are already assigned.
    let mut updates: Vec<(String, i32, i32)> = Vec::new();
    for v in g.nodes() {
        if let Some(node) = g.node(&v) {
            if let Some(border_top) = node.border_top.clone() {
                let min_rank = g.node(&border_top).and_then(|n| n.rank).unwrap_or(0);
                let border_bottom = node.border_bottom.clone().unwrap();
                let max_r = g.node(&border_bottom).and_then(|n| n.rank).unwrap_or(0);
                max_rank = max_rank.max(max_r);
                updates.push((v.clone(), min_rank, max_r));
            }
        }
    }
    for (v, min_rank, max_r) in updates {
        if let Some(node) = g.node_mut(&v) {
            node.min_rank = Some(min_rank);
            node.max_rank = Some(max_r);
        }
    }
    if let Some(gl) = g.graph_mut() {
        gl.max_rank = Some(max_rank);
    }
}

fn remove_edge_label_proxies(g: &mut DagreGraph) {
    let mut to_remove: Vec<(String, Edge, i32)> = Vec::new();
    for v in g.nodes() {
        if let Some(node) = g.node(&v) {
            if node.dummy == Some(DummyKind::EdgeProxy) {
                let edge_obj = node.edge_obj.clone().expect("edge-proxy node has no edge");
                let rank = node.rank.unwrap_or(0);
                to_remove.push((v.clone(), edge_obj, rank));
            }
        }
    }
    for (v, edge_obj, rank) in to_remove {
        if let Some(edge) = g.edge_mut(&edge_obj.v, &edge_obj.w, edge_obj.name.as_deref()) {
            edge.label_rank = Some(rank);
        }
        g.remove_node(&v);
    }
}

// ── translateGraph ──────────────────────────────────────────────────────────

fn translate_graph(g: &mut DagreGraph) {
    let mut min_x = f64::INFINITY;
    let mut max_x = 0.0_f64;
    let mut min_y = f64::INFINITY;
    let mut max_y = 0.0_f64;
    let margin_x = g.graph().and_then(|gl| gl.marginx).unwrap_or(0.0);
    let margin_y = g.graph().and_then(|gl| gl.marginy).unwrap_or(0.0);

    let mut get_extremes = |x: f64, y: f64, w: f64, h: f64| {
        min_x = min_x.min(x - w / 2.0);
        max_x = max_x.max(x + w / 2.0);
        min_y = min_y.min(y - h / 2.0);
        max_y = max_y.max(y + h / 2.0);
    };

    for v in g.nodes() {
        if let Some(node) = g.node(&v) {
            get_extremes(
                node.x.unwrap_or(0.0),
                node.y.unwrap_or(0.0),
                node.width,
                node.height,
            );
        }
    }
    for e in g.edges() {
        if let Some(edge) = g.edge_by_obj(&e) {
            if edge.x.is_some() {
                get_extremes(
                    edge.x.unwrap_or(0.0),
                    edge.y.unwrap_or(0.0),
                    edge.width.unwrap_or(0.0),
                    edge.height.unwrap_or(0.0),
                );
            }
        }
    }

    min_x -= margin_x;
    min_y -= margin_y;

    for v in g.nodes() {
        if let Some(node) = g.node_mut(&v) {
            node.x = node.x.map(|x| x - min_x);
            node.y = node.y.map(|y| y - min_y);
        }
    }

    for e in g.edges() {
        if let Some(edge) = g.edge_mut(&e.v, &e.w, e.name.as_deref()) {
            if let Some(points) = edge.points.as_mut() {
                for p in points.iter_mut() {
                    p.x -= min_x;
                    p.y -= min_y;
                }
            }
            if edge.x.is_some() {
                edge.x = edge.x.map(|x| x - min_x);
            }
            if edge.y.is_some() {
                edge.y = edge.y.map(|y| y - min_y);
            }
        }
    }

    if let Some(gl) = g.graph_mut() {
        gl.width = Some(max_x - min_x + margin_x);
        gl.height = Some(max_y - min_y + margin_y);
    }
}

// ── assignNodeIntersects ─────────────────────────────────────────────────────

fn assign_node_intersects(g: &mut DagreGraph) {
    for e in g.edges() {
        let node_v = g.node(&e.v).cloned();
        let node_w = g.node(&e.w).cloned();
        let (node_v, node_w) = match (node_v, node_w) {
            (Some(v), Some(w)) => (v, w),
            _ => continue,
        };

        let (p1, p2, mut points) = {
            let edge = g.edge_by_obj(&e);
            match edge.and_then(|x| x.points.clone()) {
                Some(pts) if !pts.is_empty() => {
                    let p1 = pts[0];
                    let p2 = pts[pts.len() - 1];
                    (p1, p2, pts)
                }
                _ => {
                    // No points yet: p1 = nodeW, p2 = nodeV (as Points).
                    let p1 = Point::new(node_w.x.unwrap_or(0.0), node_w.y.unwrap_or(0.0));
                    let p2 = Point::new(node_v.x.unwrap_or(0.0), node_v.y.unwrap_or(0.0));
                    (p1, p2, Vec::new())
                }
            }
        };

        let start = util::intersect_rect(&rect_of(&node_v), &p1);
        let end = util::intersect_rect(&rect_of(&node_w), &p2);
        points.insert(0, start);
        points.push(end);

        if let Some(edge) = g.edge_mut(&e.v, &e.w, e.name.as_deref()) {
            edge.points = Some(points);
        }
    }
}

fn rect_of(node: &NodeLabel) -> Rect {
    Rect {
        x: node.x.unwrap_or(0.0),
        y: node.y.unwrap_or(0.0),
        width: node.width,
        height: node.height,
    }
}

// ── fixupEdgeLabelCoords ─────────────────────────────────────────────────────

fn fixup_edge_label_coords(g: &mut DagreGraph) {
    for e in g.edges() {
        if let Some(edge) = g.edge_mut(&e.v, &e.w, e.name.as_deref()) {
            if edge.x.is_some() {
                if edge.label_pos == Some(LabelPos::L) || edge.label_pos == Some(LabelPos::R) {
                    edge.width = Some(edge.width.unwrap_or(0.0) - edge.label_offset.unwrap_or(0.0));
                }
                let half_w = edge.width.unwrap_or(0.0) / 2.0;
                let offset = edge.label_offset.unwrap_or(0.0);
                match edge.label_pos {
                    Some(LabelPos::L) => {
                        edge.x = edge.x.map(|x| x - half_w - offset);
                    }
                    Some(LabelPos::R) => {
                        edge.x = edge.x.map(|x| x + half_w + offset);
                    }
                    _ => {}
                }
            }
        }
    }
}

// ── reversePointsForReversedEdges ────────────────────────────────────────────

fn reverse_points_for_reversed_edges(g: &mut DagreGraph) {
    for e in g.edges() {
        if let Some(edge) = g.edge_mut(&e.v, &e.w, e.name.as_deref()) {
            if edge.reversed == Some(true) {
                if let Some(points) = edge.points.as_mut() {
                    points.reverse();
                }
            }
        }
    }
}

// ── removeBorderNodes ────────────────────────────────────────────────────────

fn remove_border_nodes(g: &mut DagreGraph) {
    let mut updates: Vec<(String, f64, f64, f64, f64)> = Vec::new();
    for v in g.nodes() {
        if g.children(&v).is_empty() {
            continue;
        }
        let node = match g.node(&v) {
            Some(n) => n,
            None => continue,
        };
        let border_top = node.border_top.clone().unwrap();
        let border_bottom = node.border_bottom.clone().unwrap();
        let border_left = node.border_left.clone().unwrap();
        let border_right = node.border_right.clone().unwrap();
        let t = g.node(&border_top).unwrap();
        let b = g.node(&border_bottom).unwrap();
        let l = g.node(&border_left[border_left.len() - 1]).unwrap();
        let r = g.node(&border_right[border_right.len() - 1]).unwrap();

        let width = (r.x.unwrap_or(0.0) - l.x.unwrap_or(0.0)).abs();
        let height = (b.y.unwrap_or(0.0) - t.y.unwrap_or(0.0)).abs();
        let x = l.x.unwrap_or(0.0) + width / 2.0;
        let y = t.y.unwrap_or(0.0) + height / 2.0;
        updates.push((v.clone(), width, height, x, y));
    }
    for (v, width, height, x, y) in updates {
        if let Some(node) = g.node_mut(&v) {
            node.width = width;
            node.height = height;
            node.x = Some(x);
            node.y = Some(y);
        }
    }

    let to_remove: Vec<String> = g
        .nodes()
        .into_iter()
        .filter(|v| g.node(v).map(|n| n.dummy == Some(DummyKind::Border)).unwrap_or(false))
        .collect();
    for v in to_remove {
        g.remove_node(&v);
    }
}

// ── self edges ───────────────────────────────────────────────────────────────

/// `removeSelfEdges` — stash each self-edge (its `Edge` + label) keyed by node,
/// and remove it from the graph. Returns the stash for [`insert_self_edges`].
fn remove_self_edges(g: &mut DagreGraph) -> HashMap<String, Vec<(Edge, EdgeLabel)>> {
    let mut stash: HashMap<String, Vec<(Edge, EdgeLabel)>> = HashMap::new();
    for e in g.edges() {
        if e.v == e.w {
            let label = g.edge_by_obj(&e).cloned().unwrap_or_default();
            stash.entry(e.v.clone()).or_default().push((e.clone(), label));
            g.remove_edge_obj(&e);
        }
    }
    stash
}

/// `insertSelfEdges` — re-add a dummy self-edge node per stashed self-edge,
/// interleaved into each layer's order.
fn insert_self_edges(g: &mut DagreGraph, mut stash: HashMap<String, Vec<(Edge, EdgeLabel)>>) {
    let layers = util::build_layer_matrix(g);
    for layer in layers {
        let mut order_shift = 0usize;
        for (i, v) in layer.iter().enumerate() {
            if let Some(node) = g.node_mut(v) {
                node.order = Some(i + order_shift);
            }
            let rank = g.node(v).and_then(|n| n.rank);
            if let Some(self_edges) = stash.remove(v) {
                for (edge_obj, label) in self_edges {
                    order_shift += 1;
                    let width = label.width.unwrap_or(0.0);
                    let height = label.height.unwrap_or(0.0);
                    let dummy = NodeLabel {
                        width,
                        height,
                        rank,
                        order: Some(i + order_shift),
                        edge_obj: Some(edge_obj),
                        edge_label: Some(Box::new(label)),
                        ..Default::default()
                    };
                    util::add_dummy_node(g, DummyKind::SelfEdge, dummy, "_se");
                }
            }
        }
    }
}

/// `positionSelfEdges` — turn each self-edge dummy into the 5-point loop on its
/// owning node and remove the dummy.
fn position_self_edges(g: &mut DagreGraph) {
    let self_nodes: Vec<String> = g
        .nodes()
        .into_iter()
        .filter(|v| g.node(v).map(|n| n.dummy == Some(DummyKind::SelfEdge)).unwrap_or(false))
        .collect();

    for v in self_nodes {
        let node = g.node(&v).cloned().unwrap();
        let edge_obj = node.edge_obj.clone().unwrap();
        let label = node.edge_label.clone().map(|b| *b).unwrap_or_default();

        let self_node = g.node(&edge_obj.v).cloned().unwrap();
        let x = self_node.x.unwrap_or(0.0) + self_node.width / 2.0;
        let y = self_node.y.unwrap_or(0.0);
        let dx = node.x.unwrap_or(0.0) - x;
        let dy = self_node.height / 2.0;

        let mut label = label;
        label.points = Some(vec![
            Point::new(x + 2.0 * dx / 3.0, y - dy),
            Point::new(x + 5.0 * dx / 6.0, y - dy),
            Point::new(x + dx, y),
            Point::new(x + 5.0 * dx / 6.0, y + dy),
            Point::new(x + 2.0 * dx / 3.0, y + dy),
        ]);
        label.x = node.x;
        label.y = node.y;

        g.set_edge_obj(&edge_obj, label);
        g.remove_node(&v);
    }
}

#[cfg(test)]
mod tests;
