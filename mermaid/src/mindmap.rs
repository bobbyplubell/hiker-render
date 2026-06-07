//! `mindmap` diagram (self-contained: parse + draw). Tree layout via the
//! `hiker_graph` horizontal-tidy-tree engine — *not* dagre.
//!
//! Mermaid mindmap syntax (the subset we support):
//! ```text
//! mindmap
//!   root((mindmap))
//!     Origins
//!       Long history
//!       Popularisation
//!     Research
//!       On effectiveness[On effectiveness and feasibility]
//! ```
//! The header line is `mindmap`. **Indentation defines hierarchy**: each
//! non-blank line after the header is one node, and its leading-whitespace
//! depth (tabs normalised to a fixed width) relative to its predecessors picks
//! its parent via an indentation stack. The first node is the single root.
//!
//! Node shape comes from bracket syntax around the label (like flowchart):
//! - `id[Square]` → rect
//! - `id(Rounded)` → rounded rect
//! - `id((Circle))` → circle
//! - `id{{Hexagon}}` → hexagon
//! - bare `Text` → rounded "bubble" whose label is the whole trimmed line.
//!
//! Skipped (noted, not rendered): `::icon(...)`, `:::class` styling, and
//! markdown emphasis inside labels — these are stripped/ignored.
//!
//! Layout: a parent array is fed to [`hiker_graph::LayoutTree::from_parents`]
//! and positioned with [`hiker_graph::horizontal_tree_positions`] (root left,
//! children fanning rightward — the classic mindmap look). The tidy-tree fn
//! emits its own world-unit scale; we then scale x/y by spacing factors derived
//! from the largest node so boxes never overlap, compute the bbox over the node
//! rectangles, and translate everything into a margined SVG canvas. Connectors
//! are quadratic curves from each parent's centre to its child's centre.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/mindmap/` for the
//! upstream parser/renderer this mirrors.

use std::fmt::Write as _;

use hiker_graph::{horizontal_tree_positions, LayoutTree, Vec2};

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// The shape a mindmap node is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Default / bare text: a rounded bubble.
    Rounded,
    /// `[...]` square corners.
    Rect,
    /// `((...))` circle.
    Circle,
    /// `{{...}}` hexagon.
    Hexagon,
}

/// One parsed mindmap node: its display label, shape, and indentation depth.
#[derive(Clone, Debug, PartialEq)]
struct Node {
    label: String,
    shape: Shape,
    /// Leading-whitespace depth (tabs normalised), used during parse only.
    depth: usize,
}

/// A parsed mindmap: nodes in source order plus a parent array (`None` = root).
#[derive(Clone, Debug, PartialEq)]
struct Mindmap {
    nodes: Vec<Node>,
    parents: Vec<Option<usize>>,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// A tab counts as this many columns of indentation.
const TAB_WIDTH: usize = 2;

/// Count leading-whitespace columns of a raw line (spaces = 1, tab = `TAB_WIDTH`).
fn indent_of(raw: &str) -> usize {
    let mut col = 0usize;
    for c in raw.chars() {
        match c {
            ' ' => col += 1,
            '\t' => col += TAB_WIDTH,
            _ => break,
        }
    }
    col
}

/// Parse a node's content (already trimmed, comment-stripped) into a label and
/// shape. Recognises the bracket forms; everything else is a bare rounded node.
///
/// A leading id token before the bracket (e.g. `id[Text]`) is dropped — only the
/// bracket content becomes the label. For bare text the whole string is the
/// label.
fn parse_node_text(content: &str) -> (String, Shape) {
    let content = content.trim();
    // Strip trailing `:::class` styling and `::icon(...)` decorations.
    let content = strip_decorations(content);
    let content = content.trim();

    // Try the bracket forms, longest delimiters first so `((` / `{{` win over
    // `(` / `{`. We search for the *first* opening delimiter so an optional id
    // prefix is skipped.
    for (open, close, shape) in [
        ("((", "))", Shape::Circle),
        ("{{", "}}", Shape::Hexagon),
        ("[", "]", Shape::Rect),
        ("(", ")", Shape::Rounded),
    ] {
        if let Some(start) = content.find(open) {
            let after = &content[start + open.len()..];
            if let Some(end) = after.rfind(close) {
                let label = after[..end].trim().to_string();
                if !label.is_empty() {
                    return (label, shape);
                }
            }
        }
    }

    // No brackets → bare rounded bubble whose label is the whole trimmed text.
    (content.to_string(), Shape::Rounded)
}

/// Remove `:::class` (trailing styling) and `::icon(...)` decorations.
fn strip_decorations(s: &str) -> String {
    let mut out = s.to_string();
    // `::icon(fa fa-book)` anywhere → drop it.
    while let Some(i) = out.find("::icon(") {
        if let Some(rel_close) = out[i..].find(')') {
            out.replace_range(i..i + rel_close + 1, "");
        } else {
            out.truncate(i);
            break;
        }
    }
    // Trailing `:::class` styling.
    if let Some(i) = out.find(":::") {
        out.truncate(i);
    }
    out
}

/// Parse mindmap source into a [`Mindmap`]. Returns `Err(message)` when the
/// `mindmap` header is missing.
fn parse_mindmap(src: &str) -> Result<Mindmap, String> {
    let mut saw_header = false;
    let mut nodes: Vec<Node> = Vec::new();
    // Indentation stack of (depth, node_index) for parent resolution.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut parents: Vec<Option<usize>> = Vec::new();

    for raw in src.lines() {
        // Strip `%%` comments. Indentation is measured on the comment-stripped,
        // but not-yet-trimmed, line so leading whitespace still counts.
        let no_comment = raw.split("%%").next().unwrap_or("");
        let trimmed = no_comment.trim();
        if trimmed.is_empty() {
            continue;
        }

        if !saw_header {
            // Header must be exactly the `mindmap` keyword (optionally with
            // trailing tokens we ignore).
            let first = trimmed.split_whitespace().next().unwrap_or("");
            if first != "mindmap" {
                return Err(format!("expected 'mindmap' header, got: {trimmed:?}"));
            }
            saw_header = true;
            continue;
        }

        let depth = indent_of(no_comment);
        let (label, shape) = parse_node_text(trimmed);
        let idx = nodes.len();

        // Resolve the parent: pop the stack until the top has a strictly smaller
        // depth; that top is the parent. Empty stack → this is a root.
        while let Some(&(d, _)) = stack.last() {
            if d >= depth {
                stack.pop();
            } else {
                break;
            }
        }
        let parent = stack.last().map(|&(_, i)| i);
        parents.push(parent);
        nodes.push(Node { label, shape, depth });
        stack.push((depth, idx));
    }

    if !saw_header {
        return Err("empty input / no 'mindmap' header".to_string());
    }
    Ok(Mindmap { nodes, parents })
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

/// Margin around the whole drawing, px.
const MARGIN: f32 = 24.0;
/// Node border / connector stroke width, px.
const STROKE_W: f32 = 1.5;
/// Corner radius for rounded rects, px.
const CORNER_R: f32 = 12.0;

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Per-node measured half-extents (so circles/hexes can square up).
struct Sized {
    w: f32,
    h: f32,
}

/// Measure a node box from its label + shape.
fn measure(node: &Node, opts: &MermaidOptions) -> Sized {
    let (tw, th) = text_size(&node.label, opts.font_size_px);
    let mut w = tw + 2.0 * opts.node_padding_x;
    let mut h = th + 2.0 * opts.node_padding_y;
    match node.shape {
        Shape::Circle => {
            // Square it up to fit the text's diagonal-ish extent.
            let d = w.max(h);
            w = d;
            h = d;
        }
        Shape::Hexagon => {
            // Hexagons need a little extra horizontal room for the points.
            w += opts.font_size_px;
        }
        _ => {}
    }
    Sized { w: w.max(1.0), h: h.max(1.0) }
}

/// Render mermaid mindmap source to an SVG document.
pub fn render_mindmap(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let map = parse_mindmap(src).map_err(MermaidError::Parse)?;
    if map.nodes.is_empty() {
        return Err(MermaidError::Empty);
    }

    let sizes: Vec<Sized> = map.nodes.iter().map(|n| measure(n, opts)).collect();

    // Tree placement (root left, children fanning right). The tidy-tree fn emits
    // world units on its own scale; we rescale by spacing factors derived from
    // the largest node so boxes never overlap.
    let tree = LayoutTree::from_parents(&map.parents);
    let raw = horizontal_tree_positions(&tree, 1.0);

    // Largest node drives spacing. horizontal_tree_positions uses an internal
    // X_STEP=60 along the y-axis-of-vertical (now our x: depth) and Y_STEP=110
    // (now our y: sibling spread). We scale so the per-step spacing is at least
    // the largest box dimension plus the configured separations.
    let max_w = sizes.iter().fold(0.0f32, |m, s| m.max(s.w));
    let max_h = sizes.iter().fold(0.0f32, |m, s| m.max(s.h));
    // horizontal: x grows with depth (came from Y_STEP=110), y spreads siblings
    // (came from X_STEP=60).
    let x_scale = ((max_w + opts.rank_sep) / 110.0).max(0.0);
    let y_scale = ((max_h + opts.node_sep) / 60.0).max(0.0);

    let centers: Vec<Vec2> = raw
        .iter()
        .map(|p| Vec2::new(p.x * x_scale, p.y * y_scale))
        .collect();

    // Bounding box over node rectangles (centre ± half-size).
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (c, s) in centers.iter().zip(&sizes) {
        min_x = min_x.min(c.x - s.w / 2.0);
        min_y = min_y.min(c.y - s.h / 2.0);
        max_x = max_x.max(c.x + s.w / 2.0);
        max_y = max_y.max(c.y + s.h / 2.0);
    }
    if !min_x.is_finite() {
        min_x = 0.0;
        min_y = 0.0;
        max_x = 0.0;
        max_y = 0.0;
    }

    // Translate so the bbox min sits at MARGIN.
    let off_x = MARGIN - min_x;
    let off_y = MARGIN - min_y;
    let centers: Vec<Vec2> = centers
        .iter()
        .map(|c| Vec2::new(c.x + off_x, c.y + off_y))
        .collect();

    let width = (max_x - min_x) + 2.0 * MARGIN;
    let height = (max_y - min_y) + 2.0 * MARGIN;
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Connectors first (drawn under the nodes): one quadratic curve per non-root
    // node, parent centre → child centre.
    for (i, parent) in map.parents.iter().enumerate() {
        let Some(p) = *parent else { continue };
        let a = centers[p];
        let b = centers[i];
        // Control point: midpoint nudged so the curve bows pleasantly. Use the
        // parent's y at the horizontal midpoint for a smooth "branch" feel.
        let cx = (a.x + b.x) / 2.0;
        let cy = a.y;
        let _ = write!(
            svg,
            "<path d=\"M {ax:.2},{ay:.2} Q {cx:.2},{cy:.2} {bx:.2},{by:.2}\" \
             fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
            ax = a.x,
            ay = a.y,
            bx = b.x,
            by = b.y,
            stroke = rgb(opts.edge_stroke),
            so = opacity_attr("stroke-opacity", opts.edge_stroke),
        );
    }

    // Nodes.
    for (i, node) in map.nodes.iter().enumerate() {
        draw_node(&mut svg, node, &sizes[i], centers[i], i == 0, opts);
    }

    svg.push_str("</svg>");

    Ok(MermaidRender { svg, width_px: w, height_px: h })
}

/// Draw one node's shape + centered label.
fn draw_node(
    svg: &mut String,
    node: &Node,
    size: &Sized,
    center: Vec2,
    is_root: bool,
    opts: &MermaidOptions,
) {
    let (cx, cy) = (center.x, center.y);
    let (w, h) = (size.w, size.h);
    let x = cx - w / 2.0;
    let y = cy - h / 2.0;

    let fill = rgb(opts.node_fill);
    let fo = opacity_attr("fill-opacity", opts.node_fill);
    let stroke = rgb(opts.node_stroke);
    let so = opacity_attr("stroke-opacity", opts.node_stroke);
    // Root is emphasised with a slightly thicker border.
    let sw = if is_root { STROKE_W * 2.0 } else { STROKE_W };

    match node.shape {
        Shape::Rect => {
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
                 fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
            );
        }
        Shape::Rounded => {
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
                 rx=\"{CORNER_R}\" ry=\"{CORNER_R}\" \
                 fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
            );
        }
        Shape::Circle => {
            let r = w.max(h) / 2.0;
            let _ = write!(
                svg,
                "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" \
                 fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
            );
        }
        Shape::Hexagon => {
            // Pointy-left/right hexagon: notch is 1/4 of the width.
            let notch = w / 4.0;
            let _ = write!(
                svg,
                "<polygon points=\"{x0:.2},{cy:.2} {x1:.2},{y:.2} {x2:.2},{y:.2} \
                 {x3:.2},{cy:.2} {x2:.2},{yb:.2} {x1:.2},{yb:.2}\" \
                 fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
                x0 = x,
                x1 = x + notch,
                x2 = x + w - notch,
                x3 = x + w,
                yb = y + h,
            );
        }
    }

    // Centered label.
    let [tr, tg, tb, _] = opts.text_color;
    let lines: Vec<&str> = node.label.split('\n').collect();
    let line_h = opts.font_size_px * 1.2;
    let total = line_h * lines.len() as f32;
    let mut ty = cy - total / 2.0 + line_h / 2.0;
    for line in lines {
        let _ = write!(
            svg,
            "<text x=\"{cx:.2}\" y=\"{ty:.2}\" text-anchor=\"middle\" \
             dominant-baseline=\"central\" font-family=\"{family}\" \
             font-size=\"{fs}\" fill=\"rgb({tr},{tg},{tb})\">{txt}</text>",
            family = escape(&opts.font_family),
            fs = opts.font_size_px,
            txt = escape(line),
        );
        ty += line_h;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "mindmap
  root((Root))
    Origins
      History
    Research
";

    #[test]
    fn parses_indentation_into_parents() {
        let map = parse_mindmap(SAMPLE).expect("parse");
        // root, Origins, History, Research
        assert_eq!(map.nodes.len(), 4);
        assert_eq!(map.nodes[0].label, "Root");
        assert_eq!(map.nodes[1].label, "Origins");
        assert_eq!(map.nodes[2].label, "History");
        assert_eq!(map.nodes[3].label, "Research");
        // root is the sole root.
        assert_eq!(map.parents[0], None);
        // Origins & Research are children of root.
        assert_eq!(map.parents[1], Some(0));
        assert_eq!(map.parents[3], Some(0));
        // History is a child of Origins (the grandchild).
        assert_eq!(map.parents[2], Some(1));
    }

    #[test]
    fn depths_track_indentation() {
        let map = parse_mindmap(SAMPLE).expect("parse");
        assert!(map.nodes[0].depth < map.nodes[1].depth);
        assert!(map.nodes[1].depth < map.nodes[2].depth);
        // Research is a sibling of Origins → same depth.
        assert_eq!(map.nodes[1].depth, map.nodes[3].depth);
    }

    #[test]
    fn single_root_only() {
        let map = parse_mindmap(SAMPLE).expect("parse");
        let roots = map.parents.iter().filter(|p| p.is_none()).count();
        assert_eq!(roots, 1);
    }

    #[test]
    fn detects_shapes() {
        assert_eq!(parse_node_text("id[Square]"), ("Square".to_string(), Shape::Rect));
        assert_eq!(parse_node_text("id(Rounded)"), ("Rounded".to_string(), Shape::Rounded));
        assert_eq!(parse_node_text("id((Circle))"), ("Circle".to_string(), Shape::Circle));
        assert_eq!(parse_node_text("id{{Hex}}"), ("Hex".to_string(), Shape::Hexagon));
    }

    #[test]
    fn bare_text_is_rounded_whole_line() {
        let (label, shape) = parse_node_text("Just some text");
        assert_eq!(label, "Just some text");
        assert_eq!(shape, Shape::Rounded);
    }

    #[test]
    fn circle_beats_rounded() {
        // `((...))` must be detected as a circle, not the inner `(...)`.
        let (label, shape) = parse_node_text("x((mindmap))");
        assert_eq!(label, "mindmap");
        assert_eq!(shape, Shape::Circle);
    }

    #[test]
    fn strips_class_and_icon() {
        let (label, _) = parse_node_text("Idea:::myClass");
        assert_eq!(label, "Idea");
        let (label2, _) = parse_node_text("Book ::icon(fa fa-book)");
        assert_eq!(label2.trim(), "Book");
    }

    #[test]
    fn tabs_count_as_indentation() {
        let src = "mindmap\nroot\n\tchild\n";
        let map = parse_mindmap(src).expect("parse");
        assert_eq!(map.nodes.len(), 2);
        assert_eq!(map.parents[1], Some(0));
    }

    #[test]
    fn render_well_formed_svg() {
        let r = render_mindmap(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"), "got: {}", &r.svg[..40.min(r.svg.len())]);
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_has_shape_and_label_per_node() {
        let r = render_mindmap(SAMPLE, &MermaidOptions::default()).expect("render");
        // 4 nodes: root is a circle, the rest are rounded rects.
        let shapes = r.svg.matches("<rect").count() + r.svg.matches("<circle").count();
        assert_eq!(shapes, 4, "one shape per node; svg={}", r.svg);
        // 4 labels (no multi-line here).
        assert_eq!(r.svg.matches("<text").count(), 4);
        assert!(r.svg.contains(">Root<"));
        assert!(r.svg.contains(">History<"));
    }

    #[test]
    fn render_has_n_minus_one_connectors() {
        let r = render_mindmap(SAMPLE, &MermaidOptions::default()).expect("render");
        // 4 nodes → 3 connector paths.
        assert_eq!(r.svg.matches("<path").count(), 3, "connectors; svg={}", r.svg);
    }

    #[test]
    fn xml_escapes_label() {
        let src = "mindmap\nroot[A & B <x>]\n";
        let r = render_mindmap(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("A &amp; B &lt;x&gt;"), "got: {}", r.svg);
        assert!(!r.svg.contains("A & B <x>"));
    }

    #[test]
    fn empty_input_errors() {
        match render_mindmap("", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn missing_header_errors() {
        let r = render_mindmap("graph TD\nA-->B\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    #[test]
    fn header_only_is_empty() {
        let r = render_mindmap("mindmap\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Empty)));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let a = render_mindmap(SAMPLE, &opts).expect("a");
        let b = render_mindmap(SAMPLE, &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
