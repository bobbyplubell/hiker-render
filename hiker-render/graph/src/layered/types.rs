//! dagre label types — a port of `dagre/lib/types.ts`.
//!
//! dagre's node/edge/graph labels are large structs of mostly-optional fields
//! that the layout pipeline reads and writes as it runs. They are modelled here
//! as Rust structs with `Option<T>` fields (`undefined` → `None`). Where the TS
//! uses string-literal unions that the pipeline `switch`es on (rankdir,
//! labelpos, dummy kinds, …) we use small Rust enums; free-form string fields
//! (`class`, `shape`, `label`, …) stay `String`.
//!
//! The canonical concrete graph type for the rest of the dagre pipeline is
//! [`DagreGraph`] = `Graph<GraphLabel, NodeLabel, EdgeLabel>`.

use super::graph::{Edge, Graph};

/// A 2-D point — dagre's `Point { x, y }`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Kind of dummy node — TS `dummy: 'edge' | 'border' | 'edge-label' |
/// 'edge-proxy' | 'selfedge' | 'root'`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DummyKind {
    Edge,
    Border,
    EdgeLabel,
    EdgeProxy,
    SelfEdge,
    Root,
}

/// Which side a border node belongs to — TS `borderType: 'borderLeft' |
/// 'borderRight'`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderType {
    BorderLeft,
    BorderRight,
}

/// Label position — TS `labelpos: 'l' | 'c' | 'r'`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelPos {
    L,
    C,
    R,
}

/// Layout direction — TS `rankdir: 'TB' | 'BT' | 'LR' | 'RL'`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RankDir {
    Tb,
    Bt,
    Lr,
    Rl,
}

/// Coordinate-assignment alignment — TS `align: 'UL' | 'UR' | 'DL' | 'DR'`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Ul,
    Ur,
    Dl,
    Dr,
}

/// Acyclicer strategy — TS `acyclicer: 'greedy'`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acyclicer {
    Greedy,
}

/// Ranking algorithm — TS `ranker: 'network-simplex' | 'tight-tree' |
/// 'longest-path'`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ranker {
    NetworkSimplex,
    TightTree,
    LongestPath,
}

/// Node label — port of TS `NodeLabel`.
///
/// `width`/`height` are non-optional in the TS interface but are routinely
/// constructed empty by the pipeline (e.g. dummy/border nodes start `{width:
/// 0, height: 0}`), so they are kept as plain `f64` defaulting to `0.0`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeLabel {
    pub width: f64,
    pub height: f64,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub rank: Option<i32>,
    pub order: Option<usize>,
    pub e: Option<f64>,
    pub dummy: Option<DummyKind>,
    pub border_type: Option<BorderType>,
    pub border_top: Option<String>,
    pub border_bottom: Option<String>,
    pub border_left: Option<Vec<String>>,
    pub border_right: Option<Vec<String>>,
    pub min_rank: Option<i32>,
    pub max_rank: Option<i32>,
    pub label: Option<String>,
    pub label_pos: Option<LabelPos>,
    pub class: Option<String>,
    pub padding: Option<f64>,
    pub padding_x: Option<f64>,
    pub padding_y: Option<f64>,
    pub rx: Option<f64>,
    pub ry: Option<f64>,
    pub shape: Option<String>,
    pub edge_label: Option<Box<EdgeLabel>>,
    pub edge_obj: Option<Edge>,
}

/// Edge label — port of TS `EdgeLabel`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeLabel {
    pub points: Option<Vec<Point>>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub minlen: Option<i32>,
    pub weight: Option<f64>,
    pub label_pos: Option<LabelPos>,
    pub label_offset: Option<f64>,
    pub label_rank: Option<i32>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub e: Option<f64>,
    pub reversed: Option<bool>,
    pub forward_name: Option<String>,
    pub self_edge: Option<bool>,
    pub nesting_edge: Option<bool>,
    pub cutvalue: Option<f64>,
    pub lim: Option<i32>,
    pub low: Option<i32>,
    pub parent: Option<String>,
    pub edge_label: Option<Box<EdgeLabel>>,
    pub edge_obj: Option<Edge>,
}

/// Graph label — port of TS `GraphLabel`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphLabel {
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub compound: Option<bool>,
    pub rankdir: Option<RankDir>,
    pub align: Option<Align>,
    pub nodesep: Option<f64>,
    pub edgesep: Option<f64>,
    pub ranksep: Option<f64>,
    pub marginx: Option<f64>,
    pub marginy: Option<f64>,
    pub acyclicer: Option<Acyclicer>,
    pub ranker: Option<Ranker>,
    pub rank_align: Option<RankAlign>,
    pub nesting_root: Option<String>,
    pub node_rank_factor: Option<i32>,
    pub dummy_chains: Option<Vec<String>>,
    /// Max rank, populated during ranking (`maxRank`).
    pub max_rank: Option<i32>,
}

/// Rank alignment — TS `rankalign: 'top' | 'center' | 'bottom'`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RankAlign {
    Top,
    Center,
    Bottom,
}

/// An order constraint — TS `OrderConstraint { left, right }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderConstraint {
    pub left: String,
    pub right: String,
}

/// Result of [`partition`](super::util::partition) — TS `PartitionResult<T>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartitionResult<T> {
    pub lhs: Vec<T>,
    pub rhs: Vec<T>,
}

/// The canonical concrete dagre graph: `Graph<GraphLabel, NodeLabel,
/// EdgeLabel>`.
pub type DagreGraph = Graph<GraphLabel, NodeLabel, EdgeLabel>;
