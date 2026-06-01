//! `hiker-graph` — egui-agnostic graph layout for hiker-render/hiker-core:
//! tree + ForceAtlas2 layouts and a dagre (layered/Sugiyama) port; pure-Rust,
//! std-only, SVG/positions out.
//!
//! Graph layout engines — **egui-agnostic**, std-only.
//!
//! Three families of layout live here:
//!
//! * [`layered`] — Sugiyama / dagre port. The compound multigraph data
//!   structure that dagre runs on lives in [`layered::graph`].
//! * [`tree`] — pure, deterministic radial / vertical / horizontal tree
//!   layouts over a [`tree::LayoutTree`].
//! * [`force`] — ForceAtlas2 force-directed layout (sync entry points
//!   plus a native-only background worker).
//!
//! The crate carries its own [`vec2::Vec2`] rather than depending on
//! `eframe::egui::Vec2`, keeping the layout engines free of any graphics
//! dependency.
//!
//! [`LayoutEngine`] is the forward-looking abstraction unifying these
//! engines behind a single `GraphInput → LayoutOutput` call, so callers
//! (and the planned mermaid renderer) can swap layouts without knowing
//! their internals.

pub mod force;
pub mod layered;
pub mod tree;
pub mod vec2;

pub use force::{force_layout, force_to_convergence, LayoutParams};
pub use layered::LayeredEngine;
#[cfg(not(target_arch = "wasm32"))]
pub use force::LayoutWorker;
pub use tree::{
    bfs_tree, dfs_tree, horizontal_tree_positions, radial_positions, vertical_tree_positions,
    LayoutKind, LayoutTree,
};
pub use vec2::Vec2;

/// Input to a [`LayoutEngine`]: a graph described by its node count and
/// edge list, plus optional per-node sizes and a directedness hint.
pub struct GraphInput<'a> {
    pub node_count: usize,
    pub edges: &'a [(u32, u32)],
    pub node_sizes: Option<&'a [Vec2]>,
    /// Optional per-edge label box sizes (same order/length as `edges`;
    /// `None` for an unlabeled edge). A size-aware layered engine reserves
    /// space for these labels between ranks (so they don't overlap nodes or
    /// each other) and reports where it placed them via
    /// [`LayoutOutput::edge_label_positions`]. Point engines ignore this.
    pub edge_label_sizes: Option<&'a [Option<Vec2>]>,
    /// Optional per-node parent index (cluster/subgraph membership): `Some(p)`
    /// means node `i` is inside container node `p`; `None` = top-level. A node
    /// that is some other node's parent becomes a **cluster** — the layered
    /// engine sizes/positions it around its children and reports its rectangle
    /// via [`LayoutOutput::node_sizes`] + `positions`. Point engines ignore this.
    pub node_parents: Option<&'a [Option<usize>]>,
    pub directed: bool,
}

/// Result of running a [`LayoutEngine`]: node positions, optional
/// poly-line edge routes (empty when the engine only places nodes), and
/// the bounding size of the laid-out graph.
pub struct LayoutOutput {
    pub positions: Vec<Vec2>,
    pub edge_routes: Vec<Vec<Vec2>>,
    /// Where the engine placed each edge's label (its center), aligned to the
    /// input `edges` order. `Some` only for edges that had a size in
    /// [`GraphInput::edge_label_sizes`] and were positioned; `None` otherwise.
    /// Empty when the engine doesn't place edge labels.
    pub edge_label_positions: Vec<Option<Vec2>>,
    /// Final per-node size `(width, height)`, aligned to node index. For leaf
    /// nodes this echoes the input size; for **cluster** nodes (parents of other
    /// nodes) it is the engine-computed bounding rectangle. Empty when the engine
    /// doesn't compute sizes.
    pub node_sizes: Vec<Vec2>,
    pub size: Vec2,
}

/// A pluggable graph layout algorithm.
pub trait LayoutEngine {
    fn layout(&self, input: &GraphInput<'_>) -> LayoutOutput;
}

/// Bounding-box size of a position set (max − min per axis). Returns
/// [`Vec2::ZERO`] for an empty set.
fn bbox_size(positions: &[Vec2]) -> Vec2 {
    if positions.is_empty() {
        return Vec2::ZERO;
    }
    let mut min = positions[0];
    let mut max = positions[0];
    for &p in positions.iter().skip(1) {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    Vec2::new(max.x - min.x, max.y - min.y)
}

/// ForceAtlas2 layout behind the [`LayoutEngine`] trait. Seeds nodes on
/// a deterministic circle (no randomness) when `seed` is `None`.
#[derive(Default)]
pub struct ForceEngine {
    pub params: LayoutParams,
    pub seed: Option<Vec<Vec2>>,
}

impl LayoutEngine for ForceEngine {
    fn layout(&self, input: &GraphInput<'_>) -> LayoutOutput {
        let n = input.node_count;
        // Deterministic circle seed — no RNG, so the same input always
        // produces the same layout.
        let initial = self.seed.clone().unwrap_or_else(|| {
            (0..n)
                .map(|i| {
                    let a = if n == 0 {
                        0.0
                    } else {
                        i as f32 / n as f32 * std::f32::consts::TAU
                    };
                    Vec2::new(a.cos() * 100.0, a.sin() * 100.0)
                })
                .collect()
        });
        let positions = force_layout(initial, input.edges, &self.params);
        let size = bbox_size(&positions);
        LayoutOutput {
            positions,
            edge_routes: Vec::new(),
            edge_label_positions: Vec::new(),
            node_sizes: Vec::new(),
            size,
        }
    }
}

/// Tree layout (radial / vertical / horizontal) behind the
/// [`LayoutEngine`] trait. A non-tree graph is flattened to a spanning
/// tree rooted at node 0: BFS for [`LayoutKind::Radial`], DFS otherwise.
pub struct TreeEngine {
    pub kind: LayoutKind,
    pub area: f32,
}

impl Default for TreeEngine {
    fn default() -> Self {
        Self {
            kind: LayoutKind::VerticalTree,
            area: 1.0,
        }
    }
}

impl LayoutEngine for TreeEngine {
    fn layout(&self, input: &GraphInput<'_>) -> LayoutOutput {
        let n = input.node_count;
        let tree = match self.kind {
            LayoutKind::Radial => bfs_tree(n, input.edges, 0),
            _ => dfs_tree(n, input.edges, 0),
        };
        let positions = match self.kind {
            LayoutKind::Radial => radial_positions(&tree, self.area),
            LayoutKind::HorizontalTree => horizontal_tree_positions(&tree, self.area),
            // ForceDirected has no tree placement of its own; fall back
            // to the vertical tidy tree.
            LayoutKind::VerticalTree | LayoutKind::ForceDirected => {
                vertical_tree_positions(&tree, self.area)
            }
        };
        let size = bbox_size(&positions);
        LayoutOutput {
            positions,
            edge_routes: Vec::new(),
            edge_label_positions: Vec::new(),
            node_sizes: Vec::new(),
            size,
        }
    }
}
