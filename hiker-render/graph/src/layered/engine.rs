//! [`LayeredEngine`] — the dagre layered (Sugiyama) layout behind the
//! crate-level [`LayoutEngine`](crate::LayoutEngine) trait.
//!
//! This adapts the generic [`GraphInput`](crate::GraphInput) /
//! [`LayoutOutput`](crate::LayoutOutput) contract onto a [`DagreGraph`] and the
//! full dagre [`layout`](super::layout::layout) pipeline, so the (future)
//! mermaid renderer can pick a layered backend uniformly alongside
//! `ForceEngine`/`TreeEngine`.

use super::graph::{Edge, Graph, GraphOptions};
use super::layout;
use super::types::{DagreGraph, EdgeLabel, GraphLabel, LabelPos, NodeLabel, RankDir};
use crate::{GraphInput, LayoutEngine, LayoutOutput, Vec2};

/// Dagre layered (Sugiyama) layout behind the [`LayoutEngine`] trait.
///
/// Layered layout is inherently directed, so [`GraphInput::directed`] is
/// treated as advisory: the underlying dagre layout graph is always built
/// directed (a directed compound multigraph, matching dagre's
/// `buildLayoutGraph`).
pub struct LayeredEngine {
    /// Layout direction (`rankdir`). Default [`RankDir::Tb`].
    pub rankdir: RankDir,
    /// Separation between ranks (`ranksep`). Default `50.0`.
    pub ranksep: f32,
    /// Separation between adjacent nodes in a rank (`nodesep`). Default `50.0`.
    pub nodesep: f32,
    /// Separation between adjacent edges in a rank (`edgesep`). Default `20.0`.
    pub edgesep: f32,
    /// Node size used when [`GraphInput::node_sizes`] is `None`.
    /// Default `{50, 50}`.
    pub default_node_size: Vec2,
}

impl Default for LayeredEngine {
    fn default() -> Self {
        Self {
            rankdir: RankDir::Tb,
            ranksep: 50.0,
            nodesep: 50.0,
            edgesep: 20.0,
            default_node_size: Vec2::new(50.0, 50.0),
        }
    }
}

impl LayoutEngine for LayeredEngine {
    fn layout(&self, input: &GraphInput<'_>) -> LayoutOutput {
        let n = input.node_count;
        if n == 0 {
            return LayoutOutput {
                positions: Vec::new(),
                edge_routes: Vec::new(),
                edge_label_positions: Vec::new(),
                size: Vec2::ZERO,
            };
        }

        // dagre's layout graph is a directed compound multigraph; layered
        // layout is inherently directed so `input.directed` is advisory and we
        // always build directed.
        let mut g: DagreGraph = Graph::new(GraphOptions {
            directed: true,
            multigraph: true,
            compound: true,
        });

        g.set_graph(GraphLabel {
            rankdir: Some(self.rankdir),
            ranksep: Some(self.ranksep as f64),
            nodesep: Some(self.nodesep as f64),
            edgesep: Some(self.edgesep as f64),
            ..Default::default()
        });

        // Nodes get string ids "0".."{n-1}" so we can map back positionally.
        for i in 0..n {
            let (w, h) = match input.node_sizes {
                Some(sizes) if i < sizes.len() => (sizes[i].x as f64, sizes[i].y as f64),
                _ => (
                    self.default_node_size.x as f64,
                    self.default_node_size.y as f64,
                ),
            };
            g.set_node(
                i.to_string(),
                NodeLabel {
                    width: w,
                    height: h,
                    ..Default::default()
                },
            );
        }

        // Each edge gets a unique `name` (its index) so duplicate and self
        // edges remain distinct in the multigraph and can be read back in the
        // exact order of `input.edges`. dagre's buildLayoutGraph applies
        // minlen/weight defaults itself, so a default EdgeLabel is fine.
        let mut edge_objs: Vec<Edge> = Vec::with_capacity(input.edges.len());
        for (idx, &(v, w)) in input.edges.iter().enumerate() {
            let name = idx.to_string();
            // When a label size is supplied, set the edge label's width/height
            // (centered) so dagre reserves a gap for it between ranks and
            // positions it (an edge-label dummy that gets ordered apart from
            // siblings — which separates bidirectional/parallel labels).
            let label = match input.edge_label_sizes {
                Some(sizes) => match sizes.get(idx).copied().flatten() {
                    Some(sz) if sz.x > 0.0 && sz.y > 0.0 => EdgeLabel {
                        width: Some(sz.x as f64),
                        height: Some(sz.y as f64),
                        label_pos: Some(LabelPos::C),
                        ..Default::default()
                    },
                    _ => EdgeLabel::default(),
                },
                None => EdgeLabel::default(),
            };
            g.set_edge(v.to_string(), w.to_string(), label, Some(&name));
            edge_objs.push(Edge::new(v.to_string(), w.to_string(), Some(name)));
        }

        layout::layout(&mut g);

        // Read back node positions (f64 -> f32; 0 if unset, which should not
        // happen for real nodes).
        let positions: Vec<Vec2> = (0..n)
            .map(|i| match g.node(&i.to_string()) {
                Some(nl) => Vec2::new(
                    nl.x.unwrap_or(0.0) as f32,
                    nl.y.unwrap_or(0.0) as f32,
                ),
                None => Vec2::ZERO,
            })
            .collect();

        // Read back edge routes aligned to `input.edges` order.
        let edge_routes: Vec<Vec<Vec2>> = edge_objs
            .iter()
            .map(|e| {
                g.edge_by_obj(e)
                    .and_then(|el| el.points.as_ref())
                    .map(|pts| {
                        pts.iter()
                            .map(|p| Vec2::new(p.x as f32, p.y as f32))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();

        // Read back where dagre placed each edge's label (its center), when the
        // edge had a label size. Aligned to `input.edges` order.
        let edge_label_positions: Vec<Option<Vec2>> = edge_objs
            .iter()
            .map(|e| {
                g.edge_by_obj(e).and_then(|el| match (el.x, el.y) {
                    (Some(x), Some(y)) => Some(Vec2::new(x as f32, y as f32)),
                    _ => None,
                })
            })
            .collect();

        let size = g
            .graph()
            .map(|gl| {
                Vec2::new(
                    gl.width.unwrap_or(0.0) as f32,
                    gl.height.unwrap_or(0.0) as f32,
                )
            })
            .unwrap_or(Vec2::ZERO);

        LayoutOutput {
            positions,
            edge_routes,
            edge_label_positions,
            size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finite(v: Vec2) -> bool {
        v.x.is_finite() && v.y.is_finite()
    }

    #[test]
    fn reexported_at_crate_root_as_layout_engine() {
        fn assert_engine<T: crate::LayoutEngine>() {}
        // Resolves via the crate root (`crate::LayeredEngine`) and satisfies
        // the crate-root `LayoutEngine` bound.
        assert_engine::<crate::LayeredEngine>();
    }

    #[test]
    fn chain_ranks_increase() {
        let edges = [(0u32, 1u32), (1, 2)];
        let input = GraphInput {
            node_count: 3,
            edges: &edges,
            node_sizes: None,
            edge_label_sizes: None,
            directed: true,
        };
        let out = LayeredEngine::default().layout(&input);

        assert_eq!(out.positions.len(), 3);
        assert!(out.positions.iter().all(|&p| finite(p)));
        // TB: y increases down the chain.
        assert!(out.positions[0].y < out.positions[1].y);
        assert!(out.positions[1].y < out.positions[2].y);
        assert!(out.size.x > 0.0 && out.size.y > 0.0);
        assert_eq!(out.edge_routes.len(), edges.len());
        assert!(out.edge_routes.iter().all(|r| !r.is_empty()));
    }

    #[test]
    fn diamond_top_above_bottom() {
        let edges = [(0u32, 1u32), (0, 2), (1, 3), (2, 3)];
        let input = GraphInput {
            node_count: 4,
            edges: &edges,
            node_sizes: None,
            edge_label_sizes: None,
            directed: true,
        };
        let out = LayeredEngine::default().layout(&input);

        assert_eq!(out.positions.len(), 4);
        assert!(out.positions.iter().all(|&p| finite(p)));
        // Node 0 is the source, node 3 the sink: 0 above 3 for TB.
        assert!(out.positions[0].y < out.positions[3].y);
        assert!(out.size.x > 0.0 && out.size.y > 0.0);
        assert_eq!(out.edge_routes.len(), edges.len());
    }

    #[test]
    fn empty_graph() {
        let input = GraphInput {
            node_count: 0,
            edges: &[],
            node_sizes: None,
            edge_label_sizes: None,
            directed: true,
        };
        let out = LayeredEngine::default().layout(&input);
        assert!(out.positions.is_empty());
        assert!(out.edge_routes.is_empty());
        assert_eq!(out.size, Vec2::ZERO);
    }

    #[test]
    fn deterministic() {
        let edges = [(0u32, 1u32), (0, 2), (1, 3), (2, 3), (0, 0)];
        let input = GraphInput {
            node_count: 4,
            edges: &edges,
            node_sizes: None,
            edge_label_sizes: None,
            directed: true,
        };
        let eng = LayeredEngine::default();
        let a = eng.layout(&input);
        let b = eng.layout(&input);
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.edge_routes, b.edge_routes);
        assert_eq!(a.size, b.size);
    }

    #[test]
    fn node_sizes_respected() {
        let sizes = [
            Vec2::new(400.0, 300.0),
            Vec2::new(50.0, 50.0),
        ];
        let edges = [(0u32, 1u32)];
        let input = GraphInput {
            node_count: 2,
            edges: &edges,
            node_sizes: Some(&sizes),
            edge_label_sizes: None,
            directed: true,
        };
        let out = LayeredEngine::default().layout(&input);
        // The bounding size must be at least as big as the large node.
        assert!(out.size.x >= 400.0);
        assert!(out.size.y >= 300.0);
    }

    #[test]
    fn duplicate_edges_map_positionally() {
        // Two parallel 0->1 edges plus a self-edge: routes stay aligned.
        let edges = [(0u32, 1u32), (0, 1), (1, 1)];
        let input = GraphInput {
            node_count: 2,
            edges: &edges,
            node_sizes: None,
            edge_label_sizes: None,
            directed: true,
        };
        let out = LayeredEngine::default().layout(&input);
        assert_eq!(out.edge_routes.len(), 3);
    }
}
