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
pub mod svgutil;
// Additional diagram types. Each is self-contained (its own parse + draw) and
// exposes a `render_*` entry point. Some use the `hiker_graph` layered (dagre)
// layout (state/er/class) or tree/radial layout (mindmap); the rest self-lay-out.
pub mod architecture;
pub mod block;
pub mod c4;
pub mod class;
pub mod cynefin;
pub mod eventmodeling;
pub mod info;
pub mod ishikawa;
pub mod railroad;
pub mod treeview;
pub mod wardley;
pub mod er;
pub mod gantt;
pub mod gitgraph;
pub mod journey;
pub mod kanban;
pub mod mindmap;
pub mod packet;
pub mod pie;
pub mod quadrant;
pub mod radar;
pub mod rough;
pub mod requirement;
pub mod sankey;
pub mod font;
pub mod label;
pub mod sequence;
pub mod state;
pub mod theme;
pub mod timeline;
pub mod treemap;
pub mod venn;
pub mod xychart;

pub use model::*;
pub use theme::MermaidTheme;

/// Visual style for shapes — mermaid's `look` config.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Look {
    /// Clean geometric shapes.
    #[default]
    Classic,
    /// Hand-drawn / sketchy outlines (mermaid's `look: handDrawn`, roughjs).
    HandDrawn,
}
pub use architecture::render_architecture;
pub use cynefin::render_cynefin;
pub use eventmodeling::render_eventmodeling;
pub use info::render_info;
pub use ishikawa::render_ishikawa;
pub use railroad::render_railroad;
pub use treeview::render_treeview;
pub use wardley::render_wardley;
pub use block::render_block;
pub use c4::render_c4;
pub use class::{render_class, render_class_with_regions};
pub use er::{render_er, render_er_with_regions};
pub use gantt::render_gantt;
pub use gitgraph::render_gitgraph;
pub use journey::render_journey;
pub use kanban::render_kanban;
pub use mindmap::render_mindmap;
pub use packet::render_packet;
pub use pie::render_pie;
pub use quadrant::render_quadrant;
pub use radar::render_radar;
pub use requirement::render_requirement;
pub use sankey::render_sankey;
pub use sequence::render_sequence;
pub use state::{render_state, render_state_with_regions};
pub use timeline::render_timeline;
pub use treemap::render_treemap;
pub use venn::render_venn;
pub use xychart::render_xychart;

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
    /// Canvas background as straight RGBA. Painted as a full-bleed rect behind
    /// the diagram (when alpha > 0). Set by the theme.
    pub background: [u8; 4],
    /// Node fill / stroke as straight RGBA.
    pub node_fill: [u8; 4],
    pub node_stroke: [u8; 4],
    /// Edge line color.
    pub edge_stroke: [u8; 4],
    /// Label text color.
    pub text_color: [u8; 4],
    /// Categorical palette for multi-series diagrams (pie slices, chart bars,
    /// sankey nodes, …), cycled by series/category index. Set by the theme.
    pub series_palette: Vec<[u8; 4]>,
    /// Spacing between ranks / between nodes in a rank (dagre ranksep/nodesep), px.
    pub rank_sep: f32,
    pub node_sep: f32,
    /// Shape look (classic vs hand-drawn).
    pub look: Look,
}

impl Default for MermaidOptions {
    fn default() -> Self {
        let mut o = MermaidOptions {
            font_size_px: 16.0,
            font_family: "sans-serif".to_string(),
            node_padding_x: 14.0,
            node_padding_y: 8.0,
            background: [255, 255, 255, 255],
            node_fill: [236, 236, 255, 255],
            node_stroke: [147, 112, 219, 255],
            edge_stroke: [51, 51, 51, 255],
            text_color: [51, 51, 51, 255],
            series_palette: Vec::new(),
            rank_sep: 50.0,
            node_sep: 50.0,
            look: Look::Classic,
        };
        theme::apply(&mut o, MermaidTheme::Default);
        o
    }
}

impl MermaidOptions {
    /// Options for a built-in [`MermaidTheme`] (default fonts/sizes).
    pub fn theme(theme: MermaidTheme) -> Self {
        Self::default().with_theme(theme)
    }

    /// Apply a [`MermaidTheme`]'s colors, keeping fonts/sizes/spacing.
    pub fn with_theme(mut self, theme: MermaidTheme) -> Self {
        theme::apply(&mut self, theme);
        self
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
    // Honor `---` frontmatter and `%%{init}%%` config (theme / look / fontFamily
    // / fontSize), and strip them so the diagram parsers see a clean source.
    let (clean, owned) = preprocess_opts(src, opts);
    let opts: &MermaidOptions = owned.as_ref().unwrap_or(opts);
    let mut rendered = dispatch(&clean, opts)?;
    finish_svg(&mut rendered, opts);
    Ok(rendered)
}

/// Apply `---` frontmatter / `%%{init}%%` config (theme / look / fontFamily /
/// fontSize) to `opts`, returning the cleaned source plus an owned options clone
/// when any config was present (else `None`, so the caller reuses the borrow).
/// Shared by [`render`] and [`render_with_regions`].
fn preprocess_opts(src: &str, opts: &MermaidOptions) -> (String, Option<MermaidOptions>) {
    let (clean, cfg) = theme::preprocess(src);
    if cfg.theme.is_some()
        || cfg.look.is_some()
        || cfg.font_family.is_some()
        || cfg.font_size.is_some()
    {
        let mut o = opts.clone();
        if let Some(t) = cfg.theme {
            o = o.with_theme(t);
        }
        if let Some(l) = cfg.look {
            o.look = l;
        }
        if let Some(f) = cfg.font_family {
            o.font_family = f;
        }
        if let Some(s) = cfg.font_size {
            o.font_size_px = s;
        }
        (clean, Some(o))
    } else {
        (clean, None)
    }
}

/// Common SVG post-processing: inject the background rect and, for the
/// hand-drawn look, roughen shapes into sketchy paths. Shared by [`render`] and
/// [`render_with_regions`].
fn finish_svg(rendered: &mut MermaidRender, opts: &MermaidOptions) {
    inject_background(rendered, opts.background);
    if opts.look == Look::HandDrawn {
        rough::roughen(&mut rendered.svg);
    }
}

/// A clickable/hoverable region of a rendered diagram, in diagram-px coords
/// (the same space as the SVG `viewBox`). An interactive host hit-tests the
/// pointer against these and responds — open `link`, invoke `callback`, or show
/// `tooltip` — the static-SVG-friendly way to do mermaid's `click` directive.
#[derive(Clone, Debug, PartialEq)]
pub struct HitRegion {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub link: Option<String>,
    pub callback: Option<String>,
    pub tooltip: Option<String>,
}

/// Like [`render`], but also returns per-element hit regions for interaction.
/// Currently flowchart node regions (with their `click`/tooltip data); other
/// diagram types return an empty region list (the SVG still renders).
pub fn render_with_regions(
    src: &str,
    opts: &MermaidOptions,
) -> Result<(MermaidRender, Vec<HitRegion>), MermaidError> {
    let (clean, owned) = preprocess_opts(src, opts);
    let opts: &MermaidOptions = owned.as_ref().unwrap_or(opts);

    // Flowcharts: build the positioned diagram once so we can both draw the SVG
    // and derive hit regions from the very same node boxes.
    match diagram_keyword(&clean).as_deref() {
        Some("graph") | Some("flowchart") => {
            let chart = parse::parse_flowchart(&clean).map_err(MermaidError::Parse)?;
            if chart.nodes.is_empty() {
                return Err(MermaidError::Empty);
            }
            let sizes: Vec<(f32, f32)> = chart
                .nodes
                .iter()
                .map(|n| measure::measure_node(&n.label, n.shape, opts))
                .collect();
            let diagram = layout::layout_flowchart(&chart, &sizes, opts);
            let regions: Vec<HitRegion> = diagram
                .nodes
                .iter()
                .map(|n| HitRegion {
                    id: n.id.clone(),
                    x: n.cx - n.w / 2.0,
                    y: n.cy - n.h / 2.0,
                    w: n.w,
                    h: n.h,
                    link: n.link.clone(),
                    callback: n.callback.clone(),
                    tooltip: n.tooltip.clone(),
                })
                .collect();
            let mut rendered = MermaidRender {
                svg: draw::draw_svg(&diagram, opts),
                width_px: diagram.width,
                height_px: diagram.height,
            };
            finish_svg(&mut rendered, opts);
            Ok((rendered, regions))
        }
        // Class / state / ER diagrams: their renderers build per-node boxes for
        // drawing, so they can return the matching hit regions (mirrors how
        // `dispatch` recognizes these keywords).
        Some("classDiagram") => {
            let (mut rendered, regions) = class::render_class_with_regions(&clean, opts)?;
            finish_svg(&mut rendered, opts);
            Ok((rendered, regions))
        }
        Some("stateDiagram") | Some("stateDiagram-v2") => {
            let (mut rendered, regions) = state::render_state_with_regions(&clean, opts)?;
            finish_svg(&mut rendered, opts);
            Ok((rendered, regions))
        }
        Some("erDiagram") => {
            let (mut rendered, regions) = er::render_er_with_regions(&clean, opts)?;
            finish_svg(&mut rendered, opts);
            Ok((rendered, regions))
        }
        // Other diagram types (pie, sequence, gantt, gitGraph, …) have no mermaid
        // interaction model — gitGraph has no `click` on commits, and the rest no
        // `click` directive at all — so they render normally with empty regions.
        _ => {
            let mut rendered = dispatch(&clean, opts)?;
            finish_svg(&mut rendered, opts);
            Ok((rendered, Vec::new()))
        }
    }
}

/// Insert a full-bleed background `<rect>` just after the opening `<svg …>` tag
/// (so it paints behind the diagram), when the background is not transparent.
fn inject_background(rendered: &mut MermaidRender, bg: [u8; 4]) {
    if bg[3] == 0 {
        return;
    }
    let Some(gt) = rendered.svg.find('>') else {
        return;
    };
    let op = if bg[3] < 255 {
        format!(" fill-opacity=\"{:.4}\"", bg[3] as f32 / 255.0)
    } else {
        String::new()
    };
    let rect = format!(
        "<rect x=\"0\" y=\"0\" width=\"{w:.0}\" height=\"{h:.0}\" fill=\"rgb({r},{g},{b})\"{op}/>",
        w = rendered.width_px.ceil() + 2.0,
        h = rendered.height_px.ceil() + 2.0,
        r = bg[0],
        g = bg[1],
        b = bg[2],
    );
    rendered.svg.insert_str(gt + 1, &rect);
}

/// Dispatch a (already-preprocessed) source to its diagram renderer by header.
fn dispatch(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    match diagram_keyword(src).as_deref() {
        Some("graph") | Some("flowchart") => render_flowchart(src, opts),
        Some("pie") => render_pie(src, opts),
        Some("sequenceDiagram") => render_sequence(src, opts),
        Some("stateDiagram") | Some("stateDiagram-v2") => render_state(src, opts),
        Some("classDiagram") => render_class(src, opts),
        Some("erDiagram") => render_er(src, opts),
        Some("gantt") => render_gantt(src, opts),
        Some("journey") => render_journey(src, opts),
        Some("quadrantChart") => render_quadrant(src, opts),
        Some("mindmap") => render_mindmap(src, opts),
        Some("requirementDiagram") | Some("requirement") => render_requirement(src, opts),
        Some("gitGraph") => render_gitgraph(src, opts),
        Some("xychart-beta") | Some("xychart") => render_xychart(src, opts),
        Some("radar-beta") | Some("radar") => render_radar(src, opts),
        Some("timeline") => render_timeline(src, opts),
        Some("kanban") => render_kanban(src, opts),
        Some("sankey-beta") | Some("sankey") => render_sankey(src, opts),
        Some("treemap-beta") | Some("treemap") => render_treemap(src, opts),
        Some("packet-beta") | Some("packet") => render_packet(src, opts),
        Some("block-beta") | Some("block") => render_block(src, opts),
        Some("venn-beta") | Some("venn") => render_venn(src, opts),
        Some("C4Context") | Some("C4Container") | Some("C4Component") | Some("C4Dynamic")
        | Some("C4Deployment") => render_c4(src, opts),
        Some("architecture-beta") | Some("architecture") => render_architecture(src, opts),
        Some("cynefin-beta") | Some("cynefin") => render_cynefin(src, opts),
        Some("eventmodeling") => render_eventmodeling(src, opts),
        Some("info") => render_info(src, opts),
        Some("ishikawa") | Some("fishbone") => render_ishikawa(src, opts),
        Some("treeView-beta") | Some("treeView") | Some("treeview") => render_treeview(src, opts),
        Some("wardley-beta") | Some("wardley") => render_wardley(src, opts),
        Some("railroad-diagram") | Some("railroad-peg") | Some("railroad-ebnf")
        | Some("railroad-abnf") | Some("railroad") => render_railroad(src, opts),
        Some(other) => Err(MermaidError::Parse(format!("unknown diagram type: {other:?}"))),
        None => Err(MermaidError::Parse("empty input / no diagram header".to_string())),
    }
}

/// The first whitespace-delimited token of the first non-blank, non-`%%`-comment
/// line — the diagram-type keyword (a trailing `:` is stripped, e.g. `gitGraph:`).
fn diagram_keyword(src: &str) -> Option<String> {
    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        return line
            .split_whitespace()
            .next()
            .map(|t| t.trim_end_matches(':').to_string());
    }
    None
}

#[cfg(test)]
mod region_tests {
    use super::*;

    #[test]
    fn flowchart_regions_carry_click_data() {
        let opts = MermaidOptions::default();
        let (render, regions) = render_with_regions(
            "graph TD\n A[Start]\n click A \"https://x\" \"go\"",
            &opts,
        )
        .expect("render ok");

        // Valid render produced.
        assert!(render.svg.starts_with("<svg"));
        assert!(render.width_px > 0.0 && render.height_px > 0.0);

        // One region per node; A carries link + tooltip.
        let a = regions.iter().find(|r| r.id == "A").expect("region for A");
        assert_eq!(a.link.as_deref(), Some("https://x"));
        assert_eq!(a.tooltip.as_deref(), Some("go"));
        assert!(a.callback.is_none());

        // Rect roughly matches a node box: positive size, within the diagram.
        assert!(a.w > 0.0 && a.h > 0.0);
        assert!(a.x >= 0.0 && a.y >= 0.0);
        assert!(a.x + a.w <= render.width_px + 1.0);
        assert!(a.y + a.h <= render.height_px + 1.0);
    }

    #[test]
    fn region_count_matches_node_count() {
        let opts = MermaidOptions::default();
        let (_render, regions) =
            render_with_regions("graph TD\n A --> B --> C", &opts).expect("render ok");
        let ids: std::collections::HashSet<&str> =
            regions.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["A", "B", "C"].into_iter().collect());
    }

    #[test]
    fn non_flowchart_returns_empty_regions() {
        let opts = MermaidOptions::default();
        let (render, regions) =
            render_with_regions("pie\n \"A\" : 10\n \"B\" : 20", &opts).expect("render ok");
        assert!(regions.is_empty());
        assert!(render.svg.starts_with("<svg"));
        assert!(render.width_px > 0.0);
    }

    #[test]
    fn render_with_regions_svg_matches_render_for_click_free() {
        let opts = MermaidOptions::default();
        let src = "graph TD\n A[Start] --> B{OK?}";
        let plain = render(src, &opts).expect("render ok");
        let (with_regions, _) = render_with_regions(src, &opts).expect("render ok");
        assert_eq!(plain.svg, with_regions.svg);
        assert_eq!(plain.width_px, with_regions.width_px);
        assert_eq!(plain.height_px, with_regions.height_px);
    }

    #[test]
    fn class_diagram_regions_via_render_with_regions() {
        let opts = MermaidOptions::default();
        let src = "classDiagram\n class Animal\n class Dog\n Animal <|-- Dog\n click Animal \"https://x\" \"tip\"";
        let (render, regions) = render_with_regions(src, &opts).expect("render ok");
        let a = regions.iter().find(|r| r.id == "Animal").expect("region for Animal");
        assert_eq!(a.link.as_deref(), Some("https://x"));
        assert_eq!(a.tooltip.as_deref(), Some("tip"));
        assert!(a.w > 0.0 && a.h > 0.0);
        // finish_svg applied (background rect injected).
        assert!(render.svg.contains("<rect"));
    }

    #[test]
    fn state_diagram_regions_via_render_with_regions() {
        let opts = MermaidOptions::default();
        let src = "stateDiagram-v2\n s1 --> s2\n click s1 \"https://x\" \"tip\"";
        let (_render, regions) = render_with_regions(src, &opts).expect("render ok");
        let r = regions.iter().find(|r| r.id == "s1").expect("region for s1");
        assert_eq!(r.link.as_deref(), Some("https://x"));
        assert_eq!(r.tooltip.as_deref(), Some("tip"));
        assert!(r.w > 0.0 && r.h > 0.0);
    }

    #[test]
    fn er_diagram_regions_via_render_with_regions() {
        let opts = MermaidOptions::default();
        let src = "erDiagram\n CUSTOMER ||--o{ ORDER : places\n click CUSTOMER \"https://x\" \"tip\"";
        let (_render, regions) = render_with_regions(src, &opts).expect("render ok");
        let c = regions.iter().find(|r| r.id == "CUSTOMER").expect("region for CUSTOMER");
        assert_eq!(c.link.as_deref(), Some("https://x"));
        assert_eq!(c.tooltip.as_deref(), Some("tip"));
        assert!(c.w > 0.0 && c.h > 0.0);
    }
}
