//! Shared data model for the mermaid flowchart renderer.
//!
//! This is the contract between the four pipeline stages:
//! `parse` → `measure` → `layout` → `draw`. The parser fills a [`FlowChart`];
//! layout turns it (plus measured node sizes) into a [`PositionedDiagram`];
//! draw emits SVG from that. Keep these types stable — the stage modules depend
//! on them.

/// Flow direction. Maps to dagre's `rankdir`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Direction {
    /// `TD`/`TB` — top to bottom.
    #[default]
    TopDown,
    /// `BT` — bottom to top.
    BottomUp,
    /// `LR` — left to right.
    LeftRight,
    /// `RL` — right to left.
    RightLeft,
}

/// Node outline shape (the common flowchart shapes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NodeShape {
    /// `A[label]` — rectangle.
    #[default]
    Rect,
    /// `A(label)` — rounded rectangle.
    RoundRect,
    /// `A([label])` — stadium / pill.
    Stadium,
    /// `A((label))` — circle.
    Circle,
    /// `A{label}` — diamond / decision.
    Diamond,
    /// `A{{label}}` — hexagon.
    Hexagon,
}

/// Per-element style overrides from `classDef` / `class` / `style` / `linkStyle`
/// directives. Any `None` field falls back to the theme/options default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElemStyle {
    /// Fill color (RGBA).
    pub fill: Option<[u8; 4]>,
    /// Stroke / border color (RGBA).
    pub stroke: Option<[u8; 4]>,
    /// Stroke width in px.
    pub stroke_width: Option<f32>,
    /// Label text color (RGBA) — `color:` in a classDef.
    pub text_color: Option<[u8; 4]>,
    /// Dashed stroke (e.g. `stroke-dasharray`).
    pub dashed: bool,
}

/// A flowchart node. `id` is the source identifier (used for edge endpoints and
/// dagre node ids); `label` is the display text (defaults to `id`).
#[derive(Clone, Debug)]
pub struct FlowNode {
    pub id: String,
    pub label: String,
    pub shape: NodeShape,
    /// Per-node style overrides (from `classDef`/`class`/`style`).
    pub style: ElemStyle,
}

/// Edge line style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EdgeKind {
    /// `-->` / `---` solid.
    #[default]
    Normal,
    /// `==>` thick.
    Thick,
    /// `-.->` dotted.
    Dotted,
}

/// A flowchart edge between two node ids.
#[derive(Clone, Debug)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    /// Optional edge label (`A -->|text| B` or `A -- text --> B`).
    pub label: Option<String>,
    pub kind: EdgeKind,
    /// Arrowhead at the `from` end (e.g. `A <--> B`).
    pub arrow_start: bool,
    /// Arrowhead at the `to` end (e.g. `A --> B`; false for `A --- B`).
    pub arrow_end: bool,
    /// Per-edge style overrides (from `linkStyle`).
    pub style: ElemStyle,
}

/// A subgraph (cluster): a labeled boundary box grouping a set of nodes. Built
/// from a `subgraph <id> [Title] … end` block. `node_ids` lists the flow nodes
/// *directly* in this subgraph (a nested subgraph's members live in the nested
/// subgraph, not here); `parent` is the index into [`FlowChart::subgraphs`] of
/// the enclosing subgraph (for nesting), else `None`.
#[derive(Clone, Debug)]
pub struct Subgraph {
    pub id: String,
    pub title: String,
    pub node_ids: Vec<String>,
    pub parent: Option<usize>,
}

/// A parsed flowchart. `nodes` is in first-seen (insertion) order.
#[derive(Clone, Debug, Default)]
pub struct FlowChart {
    pub direction: Direction,
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
    /// Subgraphs (clusters), in declaration order.
    pub subgraphs: Vec<Subgraph>,
}

// ── Positioned (layout output) ──────────────────────────────────────────────

/// A node with a final center position and box size (CSS px).
#[derive(Clone, Debug)]
pub struct PositionedNode {
    pub id: String,
    pub label: String,
    pub shape: NodeShape,
    /// Center coordinates.
    pub cx: f32,
    pub cy: f32,
    /// Box width/height.
    pub w: f32,
    pub h: f32,
    /// Per-node style overrides (resolved from the FlowNode).
    pub style: ElemStyle,
}

/// A routed edge: a polyline through `points` (already clipped to node borders),
/// plus optional label placement.
#[derive(Clone, Debug)]
pub struct PositionedEdge {
    /// Polyline points, source → target (CSS px).
    pub points: Vec<(f32, f32)>,
    pub label: Option<String>,
    /// Where to center the edge label, if any.
    pub label_pos: Option<(f32, f32)>,
    pub kind: EdgeKind,
    pub arrow_start: bool,
    pub arrow_end: bool,
    /// Per-edge style overrides (resolved from the FlowEdge).
    pub style: ElemStyle,
}

/// A laid-out subgraph cluster: a boundary box (top-left `x`/`y` + size) with a
/// title drawn at its top-left.
#[derive(Clone, Debug)]
pub struct PositionedCluster {
    pub title: String,
    /// Top-left corner.
    pub x: f32,
    pub y: f32,
    /// Box width/height.
    pub w: f32,
    pub h: f32,
}

/// The laid-out diagram in a 0-origin coordinate space of size `width`×`height`.
#[derive(Clone, Debug, Default)]
pub struct PositionedDiagram {
    pub nodes: Vec<PositionedNode>,
    pub edges: Vec<PositionedEdge>,
    /// Subgraph boundary boxes, drawn behind nodes/edges.
    pub clusters: Vec<PositionedCluster>,
    pub width: f32,
    pub height: f32,
}
