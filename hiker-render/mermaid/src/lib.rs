//! `hiker-mermaid` — pure-Rust mermaid diagram renderer → SVG.
//!
//! Egui-agnostic, SVG-string out (same contract as the `hiker-render` math
//! engine), so callers rasterize with their existing resvg→texture pipeline.
//! **Flowcharts** are the first (and currently only) supported diagram; they
//! use the layered (dagre) layout from the [`hiker_graph`] crate.
//!
//! ## Pipeline
//! `source → [parse] → FlowChart → [measure node sizes] → [layout via
//! hiker-graph LayeredEngine/dagre] → PositionedDiagram → [draw] → SVG`.
//!
//! Each stage is its own module so they can evolve independently:
//! - [`parse`] — text → [`model::FlowChart`] (no upstream deps).
//! - [`measure`] — label + shape → box size (text metrics).
//! - [`layout`] — chart + sizes → [`model::PositionedDiagram`] (uses `hiker_graph`).
//! - [`draw`] — positioned diagram → SVG document.

pub mod draw;
pub mod layout;
pub mod measure;
pub mod model;
pub mod parse;
// Additional diagram types. Each is self-contained (its own parse + draw, no
// dagre layout) and exposes a `render_*` entry point.
pub mod pie;
pub mod sequence;

pub use model::*;
pub use pie::render_pie;
pub use sequence::render_sequence;

/// Rendering inputs (sizes, colors, fonts). Defaults approximate mermaid's
/// light/default flowchart theme.
#[derive(Clone, Debug)]
pub struct MermaidOptions {
    /// Label font size in CSS px.
    pub font_size_px: f32,
    /// SVG `font-family` used for `<text>` (and assumed by `measure`).
    pub font_family: String,
    /// Horizontal/vertical padding around a node's label, px.
    pub node_padding_x: f32,
    pub node_padding_y: f32,
    /// Node fill / stroke as straight RGBA.
    pub node_fill: [u8; 4],
    pub node_stroke: [u8; 4],
    /// Edge line color.
    pub edge_stroke: [u8; 4],
    /// Label text color.
    pub text_color: [u8; 4],
    /// Spacing between ranks / between nodes in a rank (dagre ranksep/nodesep), px.
    pub rank_sep: f32,
    pub node_sep: f32,
}

impl Default for MermaidOptions {
    fn default() -> Self {
        MermaidOptions {
            font_size_px: 16.0,
            font_family: "sans-serif".to_string(),
            node_padding_x: 14.0,
            node_padding_y: 8.0,
            node_fill: [236, 236, 255, 255],
            node_stroke: [147, 112, 219, 255],
            edge_stroke: [51, 51, 51, 255],
            text_color: [51, 51, 51, 255],
            rank_sep: 50.0,
            node_sep: 50.0,
        }
    }
}

/// A rendered diagram: a self-contained SVG document plus its pixel size.
#[derive(Clone, Debug, PartialEq)]
pub struct MermaidRender {
    pub svg: String,
    pub width_px: f32,
    pub height_px: f32,
}

/// Errors from [`render_flowchart`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MermaidError {
    /// The source could not be parsed as a flowchart.
    Parse(String),
    /// Parsed OK but the diagram has no nodes to render.
    Empty,
}

/// Render mermaid **flowchart** source to an SVG document.
///
/// Orchestrates the four stages; each stage lives in its own module. Returns
/// [`MermaidError::Empty`] when the chart has no nodes, or
/// [`MermaidError::Parse`] on a syntax error.
pub fn render_flowchart(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let chart = parse::parse_flowchart(src).map_err(MermaidError::Parse)?;
    if chart.nodes.is_empty() {
        return Err(MermaidError::Empty);
    }
    let sizes: Vec<(f32, f32)> = chart
        .nodes
        .iter()
        .map(|n| measure::measure_node(&n.label, n.shape, opts))
        .collect();
    let diagram = layout::layout_flowchart(&chart, &sizes, opts);
    let svg = draw::draw_svg(&diagram, opts);
    Ok(MermaidRender {
        svg,
        width_px: diagram.width,
        height_px: diagram.height,
    })
}

/// Render any supported mermaid diagram, auto-detecting the type from the source
/// header (`graph`/`flowchart` → flowchart, `pie` → pie chart, `sequenceDiagram`
/// → sequence diagram). Returns [`MermaidError::Parse`] for an unknown/missing
/// header.
pub fn render(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    match diagram_keyword(src).as_deref() {
        Some("graph") | Some("flowchart") => render_flowchart(src, opts),
        Some("pie") => render_pie(src, opts),
        Some("sequenceDiagram") => render_sequence(src, opts),
        Some(other) => Err(MermaidError::Parse(format!("unknown diagram type: {other:?}"))),
        None => Err(MermaidError::Parse("empty input / no diagram header".to_string())),
    }
}

/// The first whitespace-delimited token of the first non-blank, non-`%%`-comment
/// line — the diagram-type keyword.
fn diagram_keyword(src: &str) -> Option<String> {
    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        return line.split_whitespace().next().map(str::to_string);
    }
    None
}
