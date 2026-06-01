//! `treeView` diagram (self-contained: parse + draw).
//!
//! A treeView is a hierarchical, file-explorer-style indented tree. The header
//! is `treeView-beta` (upstream detector is `/^\s*treeView-beta/`); we also
//! accept the bare `treeView` / `treeview` aliases the dispatcher routes here.
//!
//! Syntax (the subset we support):
//! ```text
//! treeView-beta
//!     title My Project
//!     my-project/
//!         src/
//!             index.js
//!         package.json
//! ```
//! - An optional `title` line (right after the header) is rendered centered on
//!   top.
//! - **Indentation defines hierarchy**: each non-blank line is one node, and its
//!   leading-whitespace depth (tabs normalised) picks its parent via an
//!   indentation stack (exactly like the mindmap parser).
//! - A label may be quoted (`"my file"`) for names with spaces, or use the
//!   bracket form `id[Label]` (the inner text becomes the label). Bare text uses
//!   the whole trimmed line.
//! - Annotations (`:::class`, `icon(...)`, `## description`) and a trailing
//!   directory `/` marker are stripped from the label.
//!
//! Layout/draw: a top-down indented tree. Nodes are placed one per row in
//! pre-order (y increases per node); x is indented by `depth × INDENT_STEP`.
//! Each non-root node gets an orthogonal "elbow" connector: a vertical drop down
//! the parent's left rail, then a short horizontal stub into the node. Labels
//! are drawn as small rounded boxes in themed colors. Box-drawing-character input
//! is **not** supported (noted; we rely on indentation only).

use std::fmt::Write as _;

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One parsed tree node: its display label and (parse-time) indentation depth.
#[derive(Clone, Debug, PartialEq)]
struct Node {
    label: String,
    /// Leading-whitespace columns; used during parse only.
    depth: usize,
}

/// A parsed treeView: nodes in pre-order, a parent array (`None` = root), and an
/// optional title.
#[derive(Clone, Debug, PartialEq)]
struct Tree {
    nodes: Vec<Node>,
    parents: Vec<Option<usize>>,
    title: Option<String>,
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

/// Strip `:::class`, `icon(...)`, and `## description` annotations from a label.
fn strip_annotations(s: &str) -> String {
    let mut out = s.to_string();
    // `## description` runs to end of line.
    if let Some(i) = out.find("##") {
        out.truncate(i);
    }
    // `:::class` styling.
    if let Some(i) = out.find(":::") {
        out.truncate(i);
    }
    // `icon(...)` anywhere.
    while let Some(i) = out.find("icon(") {
        if let Some(rel_close) = out[i..].find(')') {
            out.replace_range(i..i + rel_close + 1, "");
        } else {
            out.truncate(i);
            break;
        }
    }
    out
}

/// Turn a node's raw (trimmed, comment-stripped) content into a display label.
/// Handles quoted labels, the `id[Label]` bracket form, annotation stripping,
/// and the trailing directory `/` marker.
fn parse_label(content: &str) -> String {
    let content = strip_annotations(content);
    let content = content.trim();

    // Quoted label: take the inner text verbatim.
    if (content.starts_with('"') && content.ends_with('"') && content.len() >= 2)
        || (content.starts_with('\'') && content.ends_with('\'') && content.len() >= 2)
    {
        return content[1..content.len() - 1].to_string();
    }

    // Bracket form `id[Label]` → inner text.
    if let Some(start) = content.find('[') {
        if let Some(end) = content.rfind(']') {
            if end > start {
                let inner = content[start + 1..end].trim();
                if !inner.is_empty() {
                    return inner.to_string();
                }
            }
        }
    }

    // Bare text: drop a trailing directory `/` marker for a cleaner label.
    content.trim_end_matches('/').trim().to_string()
}

/// Parse treeView source into a [`Tree`]. Returns `Err(message)` when the header
/// is missing/wrong.
fn parse_tree(src: &str) -> Result<Tree, String> {
    let mut saw_header = false;
    let mut title: Option<String> = None;
    let mut nodes: Vec<Node> = Vec::new();
    let mut parents: Vec<Option<usize>> = Vec::new();
    // Indentation stack of (depth, node_index) for parent resolution.
    let mut stack: Vec<(usize, usize)> = Vec::new();

    for raw in src.lines() {
        let no_comment = raw.split("%%").next().unwrap_or("");
        let trimmed = no_comment.trim();
        if trimmed.is_empty() {
            continue;
        }

        if !saw_header {
            let first = trimmed.split_whitespace().next().unwrap_or("");
            if !matches!(first, "treeView-beta" | "treeView" | "treeview") {
                return Err(format!("expected 'treeView-beta' header, got: {trimmed:?}"));
            }
            saw_header = true;
            continue;
        }

        // A `title ...` line (before any node) sets the diagram title.
        if nodes.is_empty() {
            if let Some(rest) = trimmed.strip_prefix("title") {
                if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                    let t = rest.trim();
                    if !t.is_empty() {
                        title = Some(t.to_string());
                    }
                    continue;
                }
            }
        }

        let depth = indent_of(no_comment);
        let label = parse_label(trimmed);
        let idx = nodes.len();

        // Resolve the parent: pop until the stack top has a strictly smaller
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
        nodes.push(Node { label, depth });
        stack.push((depth, idx));
    }

    if !saw_header {
        return Err("empty input / no 'treeView-beta' header".to_string());
    }
    Ok(Tree { nodes, parents, title })
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

/// Margin around the whole drawing, px.
const MARGIN: f32 = 16.0;
/// Horizontal indent per depth level, px.
const INDENT_STEP: f32 = 28.0;
/// Vertical distance between successive node rows, px.
const ROW_STEP: f32 = 34.0;
/// Inner padding around a node label, px.
const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 4.0;
/// Corner radius for the node boxes, px.
const CORNER_R: f32 = 5.0;
/// Stroke width for boxes and connectors, px.
const STROKE_W: f32 = 1.25;
/// Gap below the title before the first row, px.
const TITLE_GAP: f32 = 12.0;

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render mermaid treeView source to an SVG document.
pub fn render_treeview(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let tree = parse_tree(src).map_err(MermaidError::Parse)?;
    if tree.nodes.is_empty() {
        return Err(MermaidError::Empty);
    }

    let n = tree.nodes.len();
    let title_fs = opts.font_size_px * 1.1;
    let (title_w, title_h) = match &tree.title {
        Some(t) => text_size(t, title_fs),
        None => (0.0, 0.0),
    };
    let title_block = if tree.title.is_some() { title_h + TITLE_GAP } else { 0.0 };

    // Measure each node's box.
    let sizes: Vec<(f32, f32)> = tree
        .nodes
        .iter()
        .map(|node| {
            let (tw, th) = text_size(&node.label, opts.font_size_px);
            (tw + 2.0 * PAD_X, th + 2.0 * PAD_Y)
        })
        .collect();

    // Structural depth (root = 0) per node from the parent chain — uniform layout
    // indentation regardless of how many source spaces were used.
    let depths: Vec<usize> = (0..n)
        .map(|i| {
            let mut d = 0;
            let mut cur = tree.parents[i];
            while let Some(p) = cur {
                d += 1;
                cur = tree.parents[p];
            }
            d
        })
        .collect();

    // Pre-order placement: row i lives at a fixed y; x indented by depth.
    let row_top = MARGIN + title_block;
    // Box left edge per node.
    let xs: Vec<f32> = (0..n)
        .map(|i| MARGIN + depths[i] as f32 * INDENT_STEP)
        .collect();
    // Row center y per node.
    let ys: Vec<f32> = (0..n)
        .map(|i| row_top + ROW_STEP * i as f32 + ROW_STEP / 2.0)
        .collect();

    // Canvas size: widest box's right edge, last row's bottom.
    let mut content_right = 0.0f32;
    for i in 0..n {
        content_right = content_right.max(xs[i] + sizes[i].0);
    }
    content_right = content_right.max(MARGIN + title_w);
    let width = content_right + MARGIN;
    let height = row_top + ROW_STEP * n as f32 + MARGIN;
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Title (centered over the canvas).
    if let Some(t) = &tree.title {
        let _ = write!(
            svg,
            "<text x=\"{cx:.2}\" y=\"{ty:.2}\" text-anchor=\"middle\" \
             dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{title_fs}\" \
             font-weight=\"bold\" fill=\"{tc}\"{to}>{txt}</text>",
            cx = w / 2.0,
            ty = MARGIN + title_h / 2.0,
            family = escape(&opts.font_family),
            tc = rgb(opts.text_color),
            to = opacity_attr("fill-opacity", opts.text_color),
            txt = escape(t),
        );
    }

    // Connectors (drawn under the boxes): one elbow per non-root node. A vertical
    // line drops down the parent's left rail to the child's row, then a short
    // horizontal stub runs into the child box's left edge.
    let stroke = rgb(opts.edge_stroke);
    let so = opacity_attr("stroke-opacity", opts.edge_stroke);
    for (i, parent) in tree.parents.iter().enumerate() {
        let Some(p) = *parent else { continue };
        // Rail x: a little right of the parent box's left edge.
        let rail_x = xs[p] + INDENT_STEP / 2.0;
        let drop_top = ys[p] + sizes[p].1 / 2.0; // bottom of parent box
        let drop_bottom = ys[i]; // child row center
        let stub_end = xs[i]; // child box left edge
        let _ = write!(
            svg,
            "<path d=\"M {rx:.2},{t:.2} L {rx:.2},{b:.2} L {sx:.2},{b:.2}\" \
             fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
            rx = rail_x,
            t = drop_top,
            b = drop_bottom,
            sx = stub_end,
        );
    }

    // Node boxes + labels.
    let fill = rgb(opts.node_fill);
    let fo = opacity_attr("fill-opacity", opts.node_fill);
    let n_stroke = rgb(opts.node_stroke);
    let nso = opacity_attr("stroke-opacity", opts.node_stroke);
    let tc = rgb(opts.text_color);
    let to = opacity_attr("fill-opacity", opts.text_color);
    for i in 0..n {
        let (bw, bh) = sizes[i];
        let bx = xs[i];
        let by = ys[i] - bh / 2.0;
        let _ = write!(
            svg,
            "<rect x=\"{bx:.2}\" y=\"{by:.2}\" width=\"{bw:.2}\" height=\"{bh:.2}\" \
             rx=\"{CORNER_R}\" ry=\"{CORNER_R}\" fill=\"{fill}\"{fo} stroke=\"{n_stroke}\"{nso} \
             stroke-width=\"{STROKE_W}\"/>",
        );
        let _ = write!(
            svg,
            "<text x=\"{tx:.2}\" y=\"{ty:.2}\" text-anchor=\"start\" \
             dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{fs}\" \
             fill=\"{tc}\"{to}>{txt}</text>",
            tx = bx + PAD_X,
            ty = ys[i],
            family = escape(&opts.font_family),
            fs = opts.font_size_px,
            txt = escape(&tree.nodes[i].label),
        );
    }

    svg.push_str("</svg>");

    Ok(MermaidRender { svg, width_px: w, height_px: h })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "treeView-beta
    title My Project
    my-project/
        src/
            index.js
        package.json
";

    #[test]
    fn parses_indentation_into_parents() {
        let t = parse_tree(SAMPLE).expect("parse");
        // root + 2 children + a grandchild = 4 nodes.
        assert_eq!(t.nodes.len(), 4);
        assert_eq!(t.nodes[0].label, "my-project");
        assert_eq!(t.nodes[1].label, "src");
        assert_eq!(t.nodes[2].label, "index.js");
        assert_eq!(t.nodes[3].label, "package.json");
        // my-project is the sole root.
        assert_eq!(t.parents[0], None);
        // src and package.json are children of my-project.
        assert_eq!(t.parents[1], Some(0));
        assert_eq!(t.parents[3], Some(0));
        // index.js is a grandchild (child of src).
        assert_eq!(t.parents[2], Some(1));
    }

    #[test]
    fn captures_title() {
        let t = parse_tree(SAMPLE).expect("parse");
        assert_eq!(t.title.as_deref(), Some("My Project"));
    }

    #[test]
    fn single_root_only() {
        let t = parse_tree(SAMPLE).expect("parse");
        let roots = t.parents.iter().filter(|p| p.is_none()).count();
        assert_eq!(roots, 1);
    }

    #[test]
    fn quoted_and_bracket_labels() {
        assert_eq!(parse_label("\"my file\""), "my file");
        assert_eq!(parse_label("id[Label]"), "Label");
        assert_eq!(parse_label("src/"), "src");
        assert_eq!(parse_label("README.md"), "README.md");
    }

    #[test]
    fn strips_annotations() {
        assert_eq!(parse_label("App.tsx :::highlight icon(react) ## main"), "App.tsx");
        assert_eq!(parse_label(".env ## environment variables"), ".env");
    }

    #[test]
    fn tabs_count_as_indentation() {
        let src = "treeView-beta\nroot\n\tchild\n";
        let t = parse_tree(src).expect("parse");
        assert_eq!(t.nodes.len(), 2);
        assert_eq!(t.parents[1], Some(0));
    }

    #[test]
    fn render_well_formed_svg() {
        let r = render_treeview(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"), "got: {}", &r.svg[..40.min(r.svg.len())]);
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_has_label_per_node_and_connectors() {
        let r = render_treeview(SAMPLE, &MermaidOptions::default()).expect("render");
        // 4 node boxes.
        assert_eq!(r.svg.matches("<rect").count(), 4, "boxes; svg={}", r.svg);
        // 4 node labels + 1 title text = 5 texts.
        assert_eq!(r.svg.matches("<text").count(), 5, "texts; svg={}", r.svg);
        // n-1 = 3 connectors.
        assert_eq!(r.svg.matches("<path").count(), 3, "connectors; svg={}", r.svg);
        assert!(r.svg.contains(">my-project<"));
        assert!(r.svg.contains(">index.js<"));
    }

    #[test]
    fn renders_title() {
        let r = render_treeview(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains(">My Project<"), "title; svg={}", r.svg);
    }

    #[test]
    fn no_title_no_extra_text() {
        let src = "treeView-beta\n root\n  child\n";
        let r = render_treeview(src, &MermaidOptions::default()).expect("render");
        // 2 node labels, no title.
        assert_eq!(r.svg.matches("<text").count(), 2);
        assert_eq!(r.svg.matches("<path").count(), 1);
    }

    #[test]
    fn xml_escapes_label() {
        let src = "treeView-beta\n root[A & B <x>]\n";
        let r = render_treeview(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("A &amp; B &lt;x&gt;"), "got: {}", r.svg);
        assert!(!r.svg.contains("A & B <x>"));
    }

    #[test]
    fn empty_or_header_only_errors() {
        // No nodes after the header → Empty.
        match render_treeview("treeView-beta\n", &MermaidOptions::default()) {
            Err(MermaidError::Empty) => {}
            other => panic!("expected Empty, got {other:?}"),
        }
        // Title but no nodes → still Empty.
        match render_treeview("treeView-beta\n title T\n", &MermaidOptions::default()) {
            Err(MermaidError::Empty) => {}
            other => panic!("expected Empty, got {other:?}"),
        }
    }

    #[test]
    fn bad_header_errors() {
        match render_treeview("graph TD\nA-->B\n", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
        assert!(matches!(render_treeview("", &MermaidOptions::default()), Err(MermaidError::Parse(_))));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let a = render_treeview(SAMPLE, &opts).expect("a");
        let b = render_treeview(SAMPLE, &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
