//! Stage 3: lay out the flowchart. Upstream: the [`hiker_graph`] crate.
//!
//! Maps the [`FlowChart`] + measured node sizes onto hiker-graph's layered
//! (dagre) layout via [`LayeredEngine`] and reads back node centers + edge
//! routes into a [`PositionedDiagram`].

use crate::MermaidOptions;
use crate::model::{FlowChart, PositionedDiagram, PositionedEdge, PositionedNode};
use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

/// Lay out `chart`, using `sizes[i]` as node `i`'s `(width, height)` (same order
/// as `chart.nodes`). Produces a 0-origin [`PositionedDiagram`].
///
/// Coordinates are dagre's: a 0-origin space of `width`×`height`, node `(cx, cy)`
/// are box centers, and edge `points` are already clipped to the source/target
/// node borders (dagre routes endpoint-to-endpoint at the node intersections).
pub fn layout_flowchart(
    chart: &FlowChart,
    sizes: &[(f32, f32)],
    opts: &MermaidOptions,
) -> PositionedDiagram {
    if chart.nodes.is_empty() {
        return PositionedDiagram::default();
    }

    // id → node index. `nodes` is in first-seen order; later duplicate ids (if
    // any) keep the first index, which matches the parser's upsert semantics.
    let mut index_of: std::collections::HashMap<&str, u32> =
        std::collections::HashMap::with_capacity(chart.nodes.len());
    for (i, n) in chart.nodes.iter().enumerate() {
        index_of.entry(n.id.as_str()).or_insert(i as u32);
    }

    // Build the dagre edge list, skipping any edge whose endpoint is missing.
    // `kept` maps each emitted dagre edge back to its original `FlowEdge` so
    // labels/kinds line up with `out.edge_routes` (which is in the same order as
    // the edges we pass in).
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(chart.edges.len());
    let mut kept: Vec<usize> = Vec::with_capacity(chart.edges.len());
    // Per-edge label box size, aligned to `edges`, so dagre reserves a gap and
    // positions the label there (None for unlabeled edges).
    let mut label_sizes: Vec<Option<Vec2>> = Vec::with_capacity(chart.edges.len());
    for (j, e) in chart.edges.iter().enumerate() {
        if let (Some(&from), Some(&to)) =
            (index_of.get(e.from.as_str()), index_of.get(e.to.as_str()))
        {
            edges.push((from, to));
            kept.push(j);
            label_sizes.push(e.label.as_deref().map(|l| {
                let (w, h) = crate::svgutil::text_size(l, opts.font_size_px);
                Vec2::new(w + 10.0, h + 6.0)
            }));
        }
    }

    let node_sizes: Vec<Vec2> = sizes.iter().map(|&(w, h)| Vec2::new(w, h)).collect();

    let engine = LayeredEngine {
        rankdir: match chart.direction {
            crate::model::Direction::TopDown => RankDir::Tb,
            crate::model::Direction::BottomUp => RankDir::Bt,
            crate::model::Direction::LeftRight => RankDir::Lr,
            crate::model::Direction::RightLeft => RankDir::Rl,
        },
        ranksep: opts.rank_sep,
        nodesep: opts.node_sep,
        edgesep: 20.0,
        default_node_size: Vec2::new(50.0, 50.0),
    };

    let out = engine.layout(&GraphInput {
        node_count: chart.nodes.len(),
        edges: &edges,
        node_sizes: Some(&node_sizes),
        edge_label_sizes: Some(&label_sizes),
        directed: true,
    });

    let nodes: Vec<PositionedNode> = chart
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
            let (w, h) = sizes.get(i).copied().unwrap_or((0.0, 0.0));
            PositionedNode {
                id: n.id.clone(),
                label: n.label.clone(),
                shape: n.shape,
                cx: pos.x,
                cy: pos.y,
                w,
                h,
                style: chart.nodes[i].style.clone(),
            }
        })
        .collect();

    // Group kept dagre edges by their unordered endpoint-index pair so that
    // parallel / bidirectional edges between the same node pair spread their
    // labels perpendicular to the route instead of stacking at one midpoint.
    // `group[k]` = (index within group, group size) for kept edge k.
    let mut pair_members: std::collections::HashMap<(u32, u32), Vec<usize>> =
        std::collections::HashMap::new();
    for (k, &(a, b)) in edges.iter().enumerate() {
        let key = (a.min(b), a.max(b));
        pair_members.entry(key).or_default().push(k);
    }
    let mut group = vec![(0usize, 1usize); edges.len()];
    for members in pair_members.values() {
        let cnt = members.len();
        for (idx, &k) in members.iter().enumerate() {
            group[k] = (idx, cnt);
        }
    }

    let edges_out: Vec<PositionedEdge> = kept
        .iter()
        .enumerate()
        .map(|(dagre_idx, &orig_idx)| {
            let e = &chart.edges[orig_idx];
            let points: Vec<(f32, f32)> = out
                .edge_routes
                .get(dagre_idx)
                .map(|r| r.iter().map(|p| (p.x, p.y)).collect())
                .unwrap_or_default();
            let (idx, cnt) = group[dagre_idx];
            let label_pos = if e.label.is_some() {
                // Prefer dagre's reserved label center; fall back to the
                // perpendicular-nudged midpoint when dagre didn't place it.
                match out.edge_label_positions.get(dagre_idx).copied().flatten() {
                    Some(p) => Some((p.x, p.y)),
                    None => crate::svgutil::edge_label_anchor(&points, idx, cnt, opts.font_size_px),
                }
            } else {
                None
            };
            PositionedEdge {
                points,
                label: e.label.clone(),
                label_pos,
                kind: e.kind,
                arrow_start: e.arrow_start,
                arrow_end: e.arrow_end,
                style: e.style.clone(),
            }
        })
        .collect();

    PositionedDiagram {
        nodes,
        edges: edges_out,
        width: out.size.x,
        height: out.size.y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Direction, EdgeKind, FlowEdge, FlowNode, NodeShape};

    fn node(id: &str) -> FlowNode {
        FlowNode {
            id: id.to_string(),
            label: id.to_string(),
            shape: NodeShape::Rect,
            style: Default::default(),
        }
    }

    fn edge(from: &str, to: &str) -> FlowEdge {
        FlowEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: None,
            kind: EdgeKind::Normal,
            arrow_start: false,
            arrow_end: true,
            style: Default::default(),
        }
    }

    fn sizes_for(chart: &FlowChart) -> Vec<(f32, f32)> {
        chart.nodes.iter().map(|_| (60.0, 40.0)).collect()
    }

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    fn chain(dir: Direction) -> FlowChart {
        FlowChart {
            direction: dir,
            nodes: vec![node("a"), node("b"), node("c")],
            edges: vec![edge("a", "b"), edge("b", "c")],
        }
    }

    #[test]
    fn empty_chart_default() {
        let chart = FlowChart::default();
        let d = layout_flowchart(&chart, &[], &opts());
        assert!(d.nodes.is_empty());
        assert!(d.edges.is_empty());
        assert_eq!(d.width, 0.0);
        assert_eq!(d.height, 0.0);
    }

    #[test]
    fn chain_topdown_ranks_increase() {
        let chart = chain(Direction::TopDown);
        let sizes = sizes_for(&chart);
        let d = layout_flowchart(&chart, &sizes, &opts());

        assert_eq!(d.nodes.len(), 3);
        assert_eq!(d.edges.len(), 2);
        assert!(d.width > 0.0 && d.height > 0.0);

        // cy strictly increases a < b < c.
        assert!(d.nodes[0].cy < d.nodes[1].cy);
        assert!(d.nodes[1].cy < d.nodes[2].cy);

        // Distinct centers.
        let centers: Vec<(f32, f32)> = d.nodes.iter().map(|n| (n.cx, n.cy)).collect();
        assert_ne!(centers[0], centers[1]);
        assert_ne!(centers[1], centers[2]);
        assert_ne!(centers[0], centers[2]);

        // All finite, sizes preserved.
        for n in &d.nodes {
            assert!(n.cx.is_finite() && n.cy.is_finite());
            assert_eq!((n.w, n.h), (60.0, 40.0));
        }
        // Each edge has >= 2 points.
        for e in &d.edges {
            assert!(e.points.len() >= 2);
            assert!(e.points.iter().all(|p| p.0.is_finite() && p.1.is_finite()));
        }
    }

    #[test]
    fn chain_leftright_x_increases() {
        let chart = chain(Direction::LeftRight);
        let sizes = sizes_for(&chart);
        let d = layout_flowchart(&chart, &sizes, &opts());

        assert_eq!(d.nodes.len(), 3);
        assert!(d.nodes[0].cx < d.nodes[1].cx);
        assert!(d.nodes[1].cx < d.nodes[2].cx);
        assert!(d.width > 0.0 && d.height > 0.0);
    }

    #[test]
    fn diamond_top_above_bottom() {
        let chart = FlowChart {
            direction: Direction::TopDown,
            nodes: vec![node("a"), node("b"), node("c"), node("d")],
            edges: vec![
                edge("a", "b"),
                edge("a", "c"),
                edge("b", "d"),
                edge("c", "d"),
            ],
        };
        let sizes = sizes_for(&chart);
        let d = layout_flowchart(&chart, &sizes, &opts());

        assert_eq!(d.nodes.len(), 4);
        assert_eq!(d.edges.len(), 4);
        // a (index 0) above d (index 3).
        assert!(d.nodes[0].cy < d.nodes[3].cy);
        assert!(d.width > 0.0 && d.height > 0.0);
        for n in &d.nodes {
            assert!(n.cx.is_finite() && n.cy.is_finite());
        }
    }

    #[test]
    fn labeled_edge_has_label_pos() {
        let labeled = FlowEdge {
            from: "a".to_string(),
            to: "b".to_string(),
            label: Some("yes".to_string()),
            kind: EdgeKind::Normal,
            arrow_start: false,
            arrow_end: true,
            style: Default::default(),
        };
        let chart = FlowChart {
            direction: Direction::TopDown,
            nodes: vec![node("a"), node("b")],
            edges: vec![labeled],
        };
        let sizes = sizes_for(&chart);
        let d = layout_flowchart(&chart, &sizes, &opts());

        assert_eq!(d.edges.len(), 1);
        let lp = d.edges[0].label_pos.expect("labeled edge needs a label_pos");
        assert!(lp.0.is_finite() && lp.1.is_finite());
        // Unlabeled edges get None.
        let chain = chain(Direction::TopDown);
        let sizes = sizes_for(&chain);
        let d2 = layout_flowchart(&chain, &sizes, &opts());
        assert!(d2.edges.iter().all(|e| e.label_pos.is_none()));
    }

    #[test]
    fn bidirectional_labels_separated() {
        // a→b and b→a both labeled: their label anchors must not coincide.
        let ab = FlowEdge {
            from: "a".to_string(),
            to: "b".to_string(),
            label: Some("go".to_string()),
            kind: EdgeKind::Normal,
            arrow_start: false,
            arrow_end: true,
            style: Default::default(),
        };
        let ba = FlowEdge {
            from: "b".to_string(),
            to: "a".to_string(),
            label: Some("back".to_string()),
            kind: EdgeKind::Normal,
            arrow_start: false,
            arrow_end: true,
            style: Default::default(),
        };
        let chart = FlowChart {
            direction: Direction::TopDown,
            nodes: vec![node("a"), node("b")],
            edges: vec![ab, ba],
        };
        let sizes = sizes_for(&chart);
        let d = layout_flowchart(&chart, &sizes, &opts());
        assert_eq!(d.edges.len(), 2);
        let p0 = d.edges[0].label_pos.expect("edge 0 label_pos");
        let p1 = d.edges[1].label_pos.expect("edge 1 label_pos");
        assert_ne!(p0, p1, "bidirectional labels must be at distinct anchors");
    }

    #[test]
    fn deterministic() {
        let chart = FlowChart {
            direction: Direction::TopDown,
            nodes: vec![node("a"), node("b"), node("c"), node("d")],
            edges: vec![
                edge("a", "b"),
                edge("a", "c"),
                edge("b", "d"),
                edge("c", "d"),
            ],
        };
        let sizes = sizes_for(&chart);
        let a = layout_flowchart(&chart, &sizes, &opts());
        let b = layout_flowchart(&chart, &sizes, &opts());

        assert_eq!(a.width, b.width);
        assert_eq!(a.height, b.height);
        assert_eq!(a.nodes.len(), b.nodes.len());
        for (na, nb) in a.nodes.iter().zip(&b.nodes) {
            assert_eq!((na.cx, na.cy), (nb.cx, nb.cy));
        }
        for (ea, eb) in a.edges.iter().zip(&b.edges) {
            assert_eq!(ea.points, eb.points);
            assert_eq!(ea.label_pos, eb.label_pos);
        }
    }

    #[test]
    fn edge_with_missing_endpoint_skipped() {
        let chart = FlowChart {
            direction: Direction::TopDown,
            nodes: vec![node("a"), node("b")],
            edges: vec![edge("a", "b"), edge("a", "ghost")],
        };
        let sizes = sizes_for(&chart);
        let d = layout_flowchart(&chart, &sizes, &opts());
        // Only the valid edge survives; nodes are untouched.
        assert_eq!(d.nodes.len(), 2);
        assert_eq!(d.edges.len(), 1);
    }
}
