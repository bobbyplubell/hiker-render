//! Stage 4: draw a [`PositionedDiagram`] → self-contained SVG. Upstream: the
//! SVG-out / resvg conventions shared with the `hiker-render` math engine.
//!
//! Emits node shapes, edge polylines (with arrowhead markers), and `<text>`
//! labels into one `<svg>` document. Built with `std::fmt::Write` into a
//! `String`, mirroring the math engine's `rgb(...)` / `*-opacity` color idiom
//! (`src/math/svg.rs`). `<text>` uses the SVG font family + system fonts, so the
//! consumer's resvg pipeline (with a loaded fontdb) rasterizes the labels.

use std::fmt::Write as _;

use crate::MermaidOptions;
use crate::model::{EdgeKind, NodeShape, PositionedDiagram, PositionedEdge, PositionedNode};

/// Node/edge stroke width, px.
const STROKE_W: f32 = 1.5;
/// Thick-edge stroke width, px.
const THICK_W: f32 = 3.0;
/// Rounded-rect corner radius, px.
const CORNER_R: f32 = 6.0;
/// Arrowhead marker triangle length, px (also the path pullback so the tip lands
/// on the node border rather than past it).
const ARROW_LEN: f32 = 9.0;
/// Arrowhead half-width, px.
const ARROW_HALF: f32 = 4.0;
/// Per-char advance / line-height used to size edge-label backgrounds. Kept in
/// sync with `measure`'s heuristic (font-free v1).
const CHAR_ADVANCE_EM: f32 = 0.6;
const LINE_HEIGHT_EM: f32 = 1.2;

/// Emit a complete, self-contained SVG document for `diagram`.
pub fn draw_svg(diagram: &PositionedDiagram, opts: &MermaidOptions) -> String {
    // Round the canvas up and add 1px so nothing clips at the edges.
    let w = (diagram.width.ceil() + 1.0).max(1.0);
    let h = (diagram.height.ceil() + 1.0).max(1.0);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    emit_defs(&mut svg, opts);

    // Edges first so node fills paint on top of the line ends.
    for edge in &diagram.edges {
        emit_edge(&mut svg, edge, opts);
    }
    for edge in &diagram.edges {
        emit_edge_label(&mut svg, edge, opts);
    }

    // Nodes (and their labels) on top.
    for node in &diagram.nodes {
        emit_node(&mut svg, node, opts);
    }

    svg.push_str("</svg>");
    svg
}

/// `<defs>` with the shared arrowhead markers (end + start variants). Drawn in
/// the edge color, sized in user-space units so the triangle is stroke-width
/// independent (`markerUnits="userSpaceOnUse"`).
fn emit_defs(svg: &mut String, opts: &MermaidOptions) {
    let (fill, fo) = fill_attrs(opts.edge_stroke);
    // End marker: tip points to +x (orient="auto" rotates it along the path).
    // refX sits at the tip so the line's last point is the arrow tip.
    let _ = write!(
        svg,
        "<defs>\
         <marker id=\"arrow\" markerWidth=\"{len}\" markerHeight=\"{w}\" \
         refX=\"{len}\" refY=\"{half}\" orient=\"auto\" markerUnits=\"userSpaceOnUse\">\
         <path d=\"M0,0 L{len},{half} L0,{w} Z\" fill=\"{fill}\"{fo}/></marker>\
         <marker id=\"arrow-start\" markerWidth=\"{len}\" markerHeight=\"{w}\" \
         refX=\"0\" refY=\"{half}\" orient=\"auto\" markerUnits=\"userSpaceOnUse\">\
         <path d=\"M{len},0 L0,{half} L{len},{w} Z\" fill=\"{fill}\"{fo}/></marker>\
         </defs>",
        len = ARROW_LEN,
        w = ARROW_HALF * 2.0,
        half = ARROW_HALF,
    );
}

/// One edge polyline. Points are already clipped to node borders; when an
/// arrowhead is present we pull the terminal segment back by `ARROW_LEN` so the
/// triangle's tip (not its base) lands on the border.
fn emit_edge(svg: &mut String, edge: &PositionedEdge, opts: &MermaidOptions) {
    if edge.points.len() < 2 {
        return;
    }
    let mut pts = edge.points.clone();
    if edge.arrow_end {
        pullback(&mut pts, true, ARROW_LEN);
    }
    if edge.arrow_start {
        pullback(&mut pts, false, ARROW_LEN);
    }

    let mut d = String::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        let cmd = if i == 0 { 'M' } else { 'L' };
        let _ = write!(d, "{cmd}{x:.2},{y:.2} ");
    }

    let (stroke, so) = stroke_attrs(opts.edge_stroke);
    let width = match edge.kind {
        EdgeKind::Thick => THICK_W,
        _ => STROKE_W,
    };
    let dash = match edge.kind {
        EdgeKind::Dotted => " stroke-dasharray=\"4 3\"",
        _ => "",
    };
    let marker_end = if edge.arrow_end { " marker-end=\"url(#arrow)\"" } else { "" };
    let marker_start = if edge.arrow_start { " marker-start=\"url(#arrow-start)\"" } else { "" };

    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{width}\"{so}{dash}{marker_start}{marker_end}/>",
        d.trim_end(),
    );
}

/// Shorten the polyline at one end by `amount` px along its terminal segment so
/// an arrowhead marker's tip sits on the node border. `end=true` trims the last
/// point, `end=false` the first.
fn pullback(pts: &mut [(f32, f32)], end: bool, amount: f32) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let (tip_i, prev_i) = if end { (n - 1, n - 2) } else { (0, 1) };
    let (tx, ty) = pts[tip_i];
    let (px, py) = pts[prev_i];
    let (dx, dy) = (tx - px, ty - py);
    let len = dx.hypot(dy);
    if len <= amount || len == 0.0 {
        return; // segment too short to pull back without inverting it
    }
    let t = (len - amount) / len;
    pts[tip_i] = (px + dx * t, py + dy * t);
}

/// Background rect + centered text for an edge label, when both label text and a
/// position are present. The background is sized from the same font-free metric
/// `measure` uses, so it covers the glyphs (heuristically).
fn emit_edge_label(svg: &mut String, edge: &PositionedEdge, opts: &MermaidOptions) {
    let (Some(label), Some((cx, cy))) = (edge.label.as_deref(), edge.label_pos) else {
        return;
    };
    if label.is_empty() {
        return;
    }
    let fs = opts.font_size_px;
    let chars = label.chars().count() as f32;
    let tw = chars * fs * CHAR_ADVANCE_EM;
    let th = fs * LINE_HEIGHT_EM;
    let pad = 2.0;
    let bw = tw + 2.0 * pad;
    let bh = th + 2.0 * pad;

    // Light background so the label reads over the edge line.
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{bw:.2}\" height=\"{bh:.2}\" fill=\"rgb(255,255,255)\" fill-opacity=\"0.85\"/>",
        x = cx - bw / 2.0,
        y = cy - bh / 2.0,
    );
    emit_text(svg, label, cx, cy, opts);
}

/// One node: its shape outline then its (possibly multi-line) label.
fn emit_node(svg: &mut String, node: &PositionedNode, opts: &MermaidOptions) {
    let PositionedNode { cx, cy, w, h, shape, label, .. } = node;
    let (cx, cy, w, h) = (*cx, *cy, *w, *h);
    let (x, y) = (cx - w / 2.0, cy - h / 2.0);
    let (fill, fo) = fill_attrs(opts.node_fill);
    let (stroke, so) = stroke_attrs(opts.node_stroke);
    let style = format!("fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"");

    match shape {
        NodeShape::Rect => {
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" {style}/>",
            );
        }
        NodeShape::RoundRect => {
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" rx=\"{r}\" ry=\"{r}\" {style}/>",
                r = CORNER_R,
            );
        }
        NodeShape::Stadium => {
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" rx=\"{r:.2}\" ry=\"{r:.2}\" {style}/>",
                r = h / 2.0,
            );
        }
        NodeShape::Circle => {
            let _ = write!(
                svg,
                "<ellipse cx=\"{cx:.2}\" cy=\"{cy:.2}\" rx=\"{rx:.2}\" ry=\"{ry:.2}\" {style}/>",
                rx = w / 2.0,
                ry = h / 2.0,
            );
        }
        NodeShape::Diamond => {
            // top, right, bottom, left
            let _ = write!(
                svg,
                "<polygon points=\"{cx:.2},{t:.2} {r:.2},{cy:.2} {cx:.2},{b:.2} {l:.2},{cy:.2}\" {style}/>",
                t = cy - h / 2.0,
                b = cy + h / 2.0,
                l = cx - w / 2.0,
                r = cx + w / 2.0,
            );
        }
        NodeShape::Hexagon => {
            // Slant the left/right ends inward by h/4. Points clockwise from the
            // top-left flat edge.
            let s = h / 4.0;
            let l = x;
            let r = x + w;
            let top = y;
            let bot = y + h;
            let _ = write!(
                svg,
                "<polygon points=\"{a:.2},{top:.2} {b:.2},{top:.2} {r:.2},{cy:.2} {b:.2},{bot:.2} {a:.2},{bot:.2} {l:.2},{cy:.2}\" {style}/>",
                a = l + s,
                b = r - s,
            );
        }
    }

    emit_text(svg, label, cx, cy, opts);
}

/// A centered `<text>` at `(cx, cy)`; multi-line labels (`\n`) become `<tspan>`
/// rows vertically centered around `cy`.
fn emit_text(svg: &mut String, label: &str, cx: f32, cy: f32, opts: &MermaidOptions) {
    if label.is_empty() {
        return;
    }
    let (fill, fo) = fill_attrs(opts.text_color);
    let family = escape(&opts.font_family);
    let fs = opts.font_size_px;

    let lines: Vec<&str> = label.split('\n').collect();
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}>",
    );
    if lines.len() == 1 {
        let _ = write!(svg, "{}", escape(lines[0]));
    } else {
        let line_h = fs * LINE_HEIGHT_EM;
        // First baseline shifted up so the block is vertically centered on cy.
        let first_dy = -(line_h * (lines.len() as f32 - 1.0)) / 2.0;
        for (i, line) in lines.iter().enumerate() {
            let dy = if i == 0 { first_dy } else { line_h };
            let _ = write!(
                svg,
                "<tspan x=\"{cx:.2}\" dy=\"{dy:.2}\">{}</tspan>",
                escape(line),
            );
        }
    }
    svg.push_str("</text>");
}

/// Build `fill="rgb(r,g,b)"` plus an optional ` fill-opacity="…"` from straight
/// RGBA (alpha < 255 emits the opacity). Mirrors the math engine's idiom.
fn fill_attrs(color: [u8; 4]) -> (String, String) {
    let [r, g, b, a] = color;
    let fill = format!("rgb({r},{g},{b})");
    let opacity = if a < 255 {
        format!(" fill-opacity=\"{:.4}\"", a as f32 / 255.0)
    } else {
        String::new()
    };
    (fill, opacity)
}

/// Same as [`fill_attrs`] but the opacity attribute is `stroke-opacity`.
fn stroke_attrs(color: [u8; 4]) -> (String, String) {
    let [r, g, b, a] = color;
    let stroke = format!("rgb({r},{g},{b})");
    let opacity = if a < 255 {
        format!(" stroke-opacity=\"{:.4}\"", a as f32 / 255.0)
    } else {
        String::new()
    };
    (stroke, opacity)
}

/// XML-escape text for use as `<text>`/`<tspan>` content or an attribute value.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, shape: NodeShape, cx: f32, cy: f32) -> PositionedNode {
        PositionedNode {
            id: id.to_string(),
            label: id.to_string(),
            shape,
            cx,
            cy,
            w: 60.0,
            h: 30.0,
                style: Default::default(),
            }
    }

    fn small_diagram() -> PositionedDiagram {
        PositionedDiagram {
            nodes: vec![
                node("A", NodeShape::Rect, 50.0, 30.0),
                node("B", NodeShape::Diamond, 50.0, 120.0),
            ],
            edges: vec![PositionedEdge {
                points: vec![(50.0, 45.0), (50.0, 105.0)],
                label: Some("yes".to_string()),
                label_pos: Some((50.0, 75.0)),
                kind: EdgeKind::Normal,
                arrow_start: false,
                arrow_end: true,
                style: Default::default(),
            }],
            width: 100.0,
            height: 150.0,
        }
    }

    #[test]
    fn wraps_in_svg_element() {
        let svg = draw_svg(&small_diagram(), &MermaidOptions::default());
        assert!(svg.starts_with("<svg"), "got: {}", &svg[..svg.len().min(40)]);
        assert!(svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains("viewBox="));
    }

    #[test]
    fn has_marker_and_defs() {
        let svg = draw_svg(&small_diagram(), &MermaidOptions::default());
        assert!(svg.contains("<defs>"));
        assert!(svg.contains("<marker id=\"arrow\""));
    }

    #[test]
    fn one_path_per_edge() {
        let svg = draw_svg(&small_diagram(), &MermaidOptions::default());
        // Edge polyline only (markers use <path> inside <defs> — count those too).
        let edge_paths = svg.matches("fill=\"none\"").count();
        assert_eq!(edge_paths, 1, "expected exactly one edge path");
    }

    #[test]
    fn shapes_per_node() {
        let svg = draw_svg(&small_diagram(), &MermaidOptions::default());
        // One <rect> node (A) + one edge-label background rect.
        assert_eq!(svg.matches("<rect").count(), 2);
        // One <polygon> for the diamond node (B).
        assert_eq!(svg.matches("<polygon").count(), 1);
    }

    #[test]
    fn ellipse_for_circle() {
        let d = PositionedDiagram {
            nodes: vec![node("C", NodeShape::Circle, 40.0, 40.0)],
            edges: vec![],
            width: 80.0,
            height: 80.0,
        };
        let svg = draw_svg(&d, &MermaidOptions::default());
        assert_eq!(svg.matches("<ellipse").count(), 1);
    }

    #[test]
    fn text_per_label() {
        let svg = draw_svg(&small_diagram(), &MermaidOptions::default());
        // Two node labels + one edge label.
        assert_eq!(svg.matches("<text").count(), 3);
    }

    #[test]
    fn dotted_and_thick_styles() {
        let mut d = small_diagram();
        d.edges[0].kind = EdgeKind::Dotted;
        let svg = draw_svg(&d, &MermaidOptions::default());
        assert!(svg.contains("stroke-dasharray=\"4 3\""));

        d.edges[0].kind = EdgeKind::Thick;
        let svg = draw_svg(&d, &MermaidOptions::default());
        assert!(svg.contains("stroke-width=\"3"));
    }

    #[test]
    fn xml_escapes_label() {
        let mut d = small_diagram();
        d.nodes[0].label = "a & b < c > \"d\"".to_string();
        let svg = draw_svg(&d, &MermaidOptions::default());
        assert!(svg.contains("a &amp; b &lt; c &gt; &quot;d&quot;"), "got: {svg}");
        assert!(!svg.contains("a & b"));
    }

    #[test]
    fn multiline_uses_tspans() {
        let mut d = small_diagram();
        d.nodes[0].label = "line one\nline two".to_string();
        let svg = draw_svg(&d, &MermaidOptions::default());
        assert_eq!(svg.matches("<tspan").count(), 2);
    }

    #[test]
    fn markers_referenced_only_when_arrows_present() {
        let mut d = small_diagram();
        d.edges[0].arrow_end = false;
        d.edges[0].arrow_start = false;
        let svg = draw_svg(&d, &MermaidOptions::default());
        assert!(!svg.contains("marker-end="));
        assert!(!svg.contains("marker-start="));

        d.edges[0].arrow_end = true;
        d.edges[0].arrow_start = true;
        let svg = draw_svg(&d, &MermaidOptions::default());
        assert!(svg.contains("marker-end=\"url(#arrow)\""));
        assert!(svg.contains("marker-start=\"url(#arrow-start)\""));
    }
}
