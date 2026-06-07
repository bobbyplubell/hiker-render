//! `assign` logic-circuit schematics — WaveJSON with a top-level `assign` key
//! renders a combinational logic-gate diagram instead of a waveform.
//!
//! ## WaveJSON shape
//! ```json
//! { "assign": [
//!   [ "out", [ "|", [ "&", "a", "b" ], [ "&", [ "~", "a" ], "c" ] ] ]
//! ] }
//! ```
//! `assign` is an array of assignments; each assignment is a 2-element array
//! `[ <output-name:string>, <expr> ]`. An `<expr>` is either a **leaf** (a JSON
//! string naming a wire) or a **gate node** — an array `[ <op>, <operand>… ]`
//! whose first element is the operator string and whose remaining elements are
//! operand exprs (recursively).
//!
//! ## Rendering model
//! The expression tree flows **left → right**: leaf inputs sit on the far left,
//! gates in columns by depth, and the assignment's output label on the right of
//! the root gate. Layout assigns each node an `(x, y)`: `x` from its column
//! (distance from the root) and `y` from a tidy subtree centroid so a gate
//! vertically centers on its operands. Gates are drawn as monochrome SVG path
//! glyphs (standard logic symbols) and connected to their operands by elbow
//! wires. Multiple assignments stack vertically.
//!
//! This is recognizable rather than pixel-identical to wavedrom.js: glyphs are
//! hand-built from a few path segments, wires are simple two-elbow routes, and
//! we do not de-duplicate shared sub-expressions (each operand draws its own
//! subtree). See the per-glyph comments for the path math.

use std::fmt::Write as _;

use crate::svgutil::{opacity_attr, rgb, text};
use crate::{WaveDromError, WaveDromOptions, WaveDromRender};

// ---- expression model ------------------------------------------------------

/// A logic expression: either a named wire (leaf) or a gate over sub-exprs.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// A named input wire.
    Leaf(String),
    /// A gate applied to one or more operand expressions.
    Gate {
        /// The raw operator string from the WaveJSON (e.g. `"&"`, `"~|"`).
        op: String,
        /// Operand sub-expressions, drawn top-to-bottom.
        inputs: Vec<Expr>,
    },
}

/// One `[ output, expr ]` assignment.
#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    pub output: String,
    pub expr: Expr,
}

/// Parse the top-level `assign` array into a list of [`Assignment`]s.
fn parse_assign(val: &serde_json::Value) -> Result<Vec<Assignment>, WaveDromError> {
    let arr = val
        .as_array()
        .ok_or_else(|| WaveDromError::Unsupported("`assign` must be an array".to_string()))?;
    let mut out = Vec::new();
    for a in arr {
        let pair = a.as_array().ok_or_else(|| {
            WaveDromError::Unsupported("each assignment must be a [name, expr] array".to_string())
        })?;
        if pair.len() != 2 {
            return Err(WaveDromError::Unsupported(
                "each assignment must be [name, expr] (2 elements)".to_string(),
            ));
        }
        let output = pair[0]
            .as_str()
            .ok_or_else(|| WaveDromError::Unsupported("assignment output must be a string".into()))?
            .to_string();
        let expr = parse_expr(&pair[1])?;
        out.push(Assignment { output, expr });
    }
    if out.is_empty() {
        return Err(WaveDromError::Empty);
    }
    Ok(out)
}

/// Parse an expr node: a string is a [`Expr::Leaf`]; an array is a gate whose
/// head is the operator and whose tail are operand exprs.
fn parse_expr(val: &serde_json::Value) -> Result<Expr, WaveDromError> {
    if let Some(s) = val.as_str() {
        return Ok(Expr::Leaf(s.to_string()));
    }
    let arr = val
        .as_array()
        .ok_or_else(|| WaveDromError::Unsupported("expr must be a string or [op, …] array".into()))?;
    let op = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| WaveDromError::Unsupported("gate's first element must be the op string".into()))?
        .to_string();
    let mut inputs = Vec::new();
    for v in &arr[1..] {
        inputs.push(parse_expr(v)?);
    }
    if inputs.is_empty() {
        // An op with no operands is degenerate; treat the op text as a leaf so
        // we never panic on malformed input.
        return Ok(Expr::Leaf(op));
    }
    Ok(Expr::Gate { op, inputs })
}

// ---- gate classification ---------------------------------------------------

/// The drawable shape family for a gate, derived from its op string.
#[derive(Clone, Copy, PartialEq, Debug)]
enum GateKind {
    And,
    Or,
    Xor,
    /// NOT / inverter / buffer (single-operand triangle). `bubble` = inverting.
    Buffer { bubble: bool },
    /// An op we do not recognize: drawn as a labeled box.
    Box,
}

/// Whether the gate's *output* carries an inverting bubble (NAND/NOR/XNOR/NOT).
struct GateStyle {
    kind: GateKind,
    bubble: bool,
    /// Text drawn inside a [`GateKind::Box`] fallback (the raw op).
    box_label: String,
}

fn classify(op: &str) -> GateStyle {
    let (kind, bubble) = match op {
        "&" => (GateKind::And, false),
        "~&" | "nand" => (GateKind::And, true),
        "|" => (GateKind::Or, false),
        "~|" | "nor" => (GateKind::Or, true),
        "^" => (GateKind::Xor, false),
        "~^" | "^~" | "xnor" => (GateKind::Xor, true),
        "~" | "inv" | "not" => (GateKind::Buffer { bubble: true }, false),
        "=" | "buf" => (GateKind::Buffer { bubble: false }, false),
        _ => (GateKind::Box, false),
    };
    GateStyle { kind, bubble, box_label: op.to_string() }
}

// ---- layout ----------------------------------------------------------------

/// Horizontal pitch between gate columns (px).
const COL_W: f32 = 90.0;
/// Vertical pitch between sibling leaves/gates (px).
const ROW_H: f32 = 44.0;
/// Gate body half-height (px); body spans `2*GATE_HH`.
const GATE_HH: f32 = 16.0;
/// Gate body width (px), left edge to the start of the output nub.
const GATE_W: f32 = 34.0;
/// Output bubble radius (px).
const BUBBLE_R: f32 = 4.0;
/// Padding around the whole drawing (px).
const PAD: f32 = 14.0;

/// A laid-out node: position is the gate body's **output point** (right-center)
/// for gates, or the wire endpoint (right-center) for leaves.
struct Node {
    expr_kind: NodeKind,
    /// Column depth from root (0 = root). Higher = further left.
    depth: usize,
    /// Output point x (right edge where wire leaves toward the parent).
    x: f32,
    /// Output point y (vertical center).
    y: f32,
    /// Child node indices (operands), top-to-bottom. Empty for leaves.
    children: Vec<usize>,
}

enum NodeKind {
    Leaf(String),
    Gate(GateStyle),
}

/// Flattened layout for one assignment's tree.
struct Tree {
    nodes: Vec<Node>,
    root: usize,
    /// Inclusive y-extent of all nodes after centroid placement.
    min_y: f32,
    max_y: f32,
}

/// Build the flattened node list and assign columns + a running leaf cursor for
/// y, then center each gate on its children. Returns the root node index.
fn build_tree(expr: &Expr) -> Tree {
    let mut nodes: Vec<Node> = Vec::new();
    // `leaf_cursor` hands out stacked y positions to leaves in visit order so
    // sibling leaves never overlap; gates then center on their children.
    let mut leaf_cursor = 0.0_f32;
    let root = place(expr, 0, &mut nodes, &mut leaf_cursor);
    // Resolve max depth so we can flip x to flow left→right (root on the right).
    let max_depth = nodes.iter().map(|n| n.depth).max().unwrap_or(0);
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for n in &mut nodes {
        // Column 0 (root) is at the right; deeper nodes go further left.
        n.x = (max_depth - n.depth) as f32 * COL_W;
        min_y = min_y.min(n.y);
        max_y = max_y.max(n.y);
    }
    Tree { nodes, root, min_y, max_y }
}

/// Recursively place `expr`, returning its node index. Leaves consume the next
/// `leaf_cursor` slot; gates are centered on the mean y of their children.
fn place(expr: &Expr, depth: usize, nodes: &mut Vec<Node>, leaf_cursor: &mut f32) -> usize {
    match expr {
        Expr::Leaf(name) => {
            let y = *leaf_cursor;
            *leaf_cursor += ROW_H;
            let idx = nodes.len();
            nodes.push(Node {
                expr_kind: NodeKind::Leaf(name.clone()),
                depth,
                x: 0.0,
                y,
                children: Vec::new(),
            });
            idx
        }
        Expr::Gate { op, inputs } => {
            let mut children = Vec::with_capacity(inputs.len());
            for inp in inputs {
                children.push(place(inp, depth + 1, nodes, leaf_cursor));
            }
            // Center on the subtree: mean of children's y (tidy centroid).
            let sum: f32 = children.iter().map(|&c| nodes[c].y).sum();
            let y = sum / children.len() as f32;
            let idx = nodes.len();
            nodes.push(Node {
                expr_kind: NodeKind::Gate(classify(op)),
                depth,
                x: 0.0,
                y,
                children,
            });
            idx
        }
    }
}

// ---- glyph drawing ----------------------------------------------------------

/// Draw a gate body whose **right-center output point** is `(ox, oy)`. The body
/// occupies `[ox - GATE_W, ox]` in x (plus a bubble past `ox` for inverting
/// gates) and `[oy - GATE_HH, oy + GATE_HH]` in y. Returns the x where input
/// wires should terminate (the gate's left edge).
fn draw_gate(svg: &mut String, style: &GateStyle, ox: f32, oy: f32, opts: &WaveDromOptions) -> f32 {
    let fg = rgb(opts.foreground);
    let fo = opacity_attr("stroke-opacity", opts.foreground);
    let stroke = format!("fill=\"none\" stroke=\"{fg}\" stroke-width=\"1.6\"{fo}");

    let left = ox - GATE_W;
    let top = oy - GATE_HH;
    let bot = oy + GATE_HH;
    // For gates with an output bubble, the body's nominal right edge sits a
    // bubble-diameter inside `ox` so the bubble's far side lands on `ox`.
    let body_r = if style.bubble { ox - 2.0 * BUBBLE_R } else { ox };

    match style.kind {
        GateKind::And => {
            // AND: flat left edge, right side is a semicircle of radius GATE_HH.
            // Body = rect [left, body_r-GATE_HH] + half-circle bulging to body_r.
            let flat_r = body_r - GATE_HH; // x where the arc begins
            let _ = write!(
                svg,
                "<path d=\"M {left:.2} {top:.2} L {flat_r:.2} {top:.2} \
                 A {r:.2} {r:.2} 0 0 1 {flat_r:.2} {bot:.2} \
                 L {left:.2} {bot:.2} Z\" {stroke}/>",
                r = GATE_HH,
            );
        }
        GateKind::Or | GateKind::Xor => {
            // OR: concave back (left edge curves in), two convex sides sweeping
            // to a point at `body_r`. We approximate the classic shield with a
            // quadratic from each corner to the tip, plus a shallow back arc.
            let tip = body_r;
            let back_cx = left + 10.0; // control depth of the concave back
            let _ = write!(
                svg,
                "<path d=\"M {left:.2} {top:.2} \
                 Q {back_cx:.2} {oy:.2} {left:.2} {bot:.2} \
                 Q {mid:.2} {bot:.2} {tip:.2} {oy:.2} \
                 Q {mid:.2} {top:.2} {left:.2} {top:.2} Z\" {stroke}/>",
                mid = left + GATE_W * 0.55,
            );
            if style.kind == GateKind::Xor {
                // XOR: an extra concave arc just behind the OR back.
                let bx = left - 6.0;
                let bcx = bx + 10.0;
                let _ = write!(
                    svg,
                    "<path d=\"M {bx:.2} {top:.2} Q {bcx:.2} {oy:.2} {bx:.2} {bot:.2}\" {stroke}/>",
                );
            }
        }
        GateKind::Buffer { bubble } => {
            // Buffer/NOT: triangle pointing right, apex at `body_r`.
            let tip = body_r;
            let _ = write!(
                svg,
                "<path d=\"M {left:.2} {top:.2} L {left:.2} {bot:.2} L {tip:.2} {oy:.2} Z\" {stroke}/>",
            );
            if bubble {
                draw_bubble(svg, body_r, oy, opts);
            }
        }
        GateKind::Box => {
            // Fallback: a labeled rectangle so unknown ops never panic.
            let _ = write!(
                svg,
                "<rect x=\"{left:.2}\" y=\"{top:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
                 rx=\"3\" {stroke}/>",
                w = body_r - left,
                h = bot - top,
            );
            text(
                svg,
                &style.box_label,
                (left + body_r) / 2.0,
                oy,
                "middle",
                opts.font_size_px * 0.8,
                &opts.font_family,
                opts.foreground,
                None,
            );
        }
    }

    // Output bubble for NAND/NOR/XNOR. (The NOT inverter's bubble is drawn by
    // the Buffer arm above, so only the body-shaped gates need one here.)
    if style.bubble && matches!(style.kind, GateKind::And | GateKind::Or | GateKind::Xor) {
        draw_bubble(svg, body_r, oy, opts);
    }

    left
}

/// Draw a small inverting-output bubble whose left edge touches `(bx, by)`.
fn draw_bubble(svg: &mut String, bx: f32, by: f32, opts: &WaveDromOptions) {
    let fg = rgb(opts.foreground);
    let fo = opacity_attr("stroke-opacity", opts.foreground);
    let bg = rgb(opts.background);
    let _ = write!(
        svg,
        "<circle cx=\"{cx:.2}\" cy=\"{by:.2}\" r=\"{r:.2}\" fill=\"{bg}\" \
         stroke=\"{fg}\" stroke-width=\"1.6\"{fo}/>",
        cx = bx + BUBBLE_R,
        r = BUBBLE_R,
    );
}

/// Draw an elbow wire from `(x0,y0)` (a child's output) to `(x1,y1)` (a parent
/// input). Two segments via a vertical riser at the horizontal midpoint.
fn draw_wire(svg: &mut String, x0: f32, y0: f32, x1: f32, y1: f32, opts: &WaveDromOptions) {
    let fg = rgb(opts.foreground);
    let fo = opacity_attr("stroke-opacity", opts.foreground);
    let midx = (x0 + x1) / 2.0;
    let _ = write!(
        svg,
        "<path d=\"M {x0:.2} {y0:.2} L {midx:.2} {y0:.2} L {midx:.2} {y1:.2} L {x1:.2} {y1:.2}\" \
         fill=\"none\" stroke=\"{fg}\" stroke-width=\"1.4\"{fo}/>",
    );
}

// ---- top-level render -------------------------------------------------------

/// Render an `assign` schematic to SVG.
pub fn render(val: &serde_json::Value, opts: &WaveDromOptions) -> Result<WaveDromRender, WaveDromError> {
    let assigns = parse_assign(val)?;

    // Lay out each assignment's tree, stacking them vertically. We collect the
    // SVG body first to discover the full extent, then emit the header.
    let mut body = String::new();
    let mut max_right = 0.0_f32; // rightmost x consumed (incl. output label)
    let mut max_left = 0.0_f32; // leftmost x consumed (incl. leaf labels)
    let mut y_off = 0.0_f32; // running vertical offset between assignments

    for asg in &assigns {
        let tree = build_tree(&asg.expr);
        // Shift this tree so its top sits at the running offset.
        let shift_y = y_off - tree.min_y;
        let tree_h = tree.max_y - tree.min_y + 2.0 * GATE_HH;

        // Draw gates (parents before/after children doesn't matter for paint
        // order here; bubbles use bg fill so order is safe). Draw wires first
        // so glyphs sit on top.
        for (idx, node) in tree.nodes.iter().enumerate() {
            let nx = node.x;
            let ny = node.y + shift_y;
            if let NodeKind::Gate(_) = node.expr_kind {
                // Wire each child's output to this gate's input edge.
                let in_x = nx - GATE_W; // gate left edge (input side)
                let n_children = node.children.len();
                for (k, &c) in node.children.iter().enumerate() {
                    let cx = tree.nodes[c].x;
                    let cy = tree.nodes[c].y + shift_y;
                    // Spread input contact points across the gate's left edge.
                    let frac = if n_children == 1 {
                        0.5
                    } else {
                        k as f32 / (n_children - 1) as f32
                    };
                    let in_y = ny - GATE_HH + frac * 2.0 * GATE_HH;
                    draw_wire(&mut body, cx, cy, in_x, in_y, opts);
                    let _ = idx;
                }
            }
        }

        // Draw glyphs and leaf labels on top of the wires.
        for node in &tree.nodes {
            let nx = node.x;
            let ny = node.y + shift_y;
            match &node.expr_kind {
                NodeKind::Gate(style) => {
                    let _left = draw_gate(&mut body, style, nx, ny, opts);
                    let right = if style.bubble || matches!(style.kind, GateKind::Buffer { bubble: true }) {
                        nx + 2.0 * BUBBLE_R
                    } else {
                        nx
                    };
                    max_right = max_right.max(right);
                }
                NodeKind::Leaf(name) => {
                    // A short input stub plus the name label to its left.
                    let stub_x0 = nx - 14.0;
                    draw_wire(&mut body, stub_x0, ny, nx, ny, opts);
                    let label_x = stub_x0 - 4.0;
                    text(
                        &mut body,
                        name,
                        label_x,
                        ny,
                        "end",
                        opts.font_size_px,
                        &opts.font_family,
                        opts.foreground,
                        None,
                    );
                    // `label_x` is the (end-anchored) right edge of the label,
                    // so it extends from `label_x - lw` leftward. The leftmost
                    // leaves sit at nx == 0, giving negative x; track how far.
                    let lw = crate::font::line_width(name, opts.font_size_px);
                    max_left = max_left.max(lw - label_x);
                }
            }
        }

        // Output label + stub at the root (right side).
        let root = &tree.nodes[tree.root];
        let rx = match &root.expr_kind {
            NodeKind::Gate(style)
                if style.bubble
                    || matches!(style.kind, GateKind::Buffer { bubble: true }) =>
            {
                root.x + 2.0 * BUBBLE_R
            }
            _ => root.x,
        };
        let ry = root.y + shift_y;
        let out_stub = rx + 16.0;
        draw_wire(&mut body, rx, ry, out_stub, ry, opts);
        text(
            &mut body,
            &asg.output,
            out_stub + 4.0,
            ry,
            "start",
            opts.font_size_px,
            &opts.font_family,
            opts.foreground,
            None,
        );
        let ow = crate::font::line_width(&asg.output, opts.font_size_px);
        max_right = max_right.max(out_stub + 4.0 + ow);

        y_off += tree_h + ROW_H * 0.5;
    }

    // The leftmost content is a leaf label; its width plus the leaf stub sets
    // the left margin. We computed `max_left` as the leftward extent past x=0.
    let left_margin = max_left.max(0.0) + PAD;
    let width = left_margin + max_right + PAD;
    let height = y_off - ROW_H * 0.5 + 2.0 * PAD;

    // Compose the document. All body coordinates are translated right by the
    // left margin (leaf labels live at negative x) and down by PAD.
    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" \
         viewBox=\"0 0 {w:.0} {h:.0}\">",
        w = width.max(1.0),
        h = height.max(1.0),
    );
    if opts.background[3] > 0 {
        let _ = write!(
            svg,
            "<rect x=\"0\" y=\"0\" width=\"{w:.0}\" height=\"{h:.0}\" fill=\"{bg}\"{bo}/>",
            w = width.max(1.0),
            h = height.max(1.0),
            bg = rgb(opts.background),
            bo = opacity_attr("fill-opacity", opts.background),
        );
    }
    let _ = write!(svg, "<g transform=\"translate({left_margin:.2},{PAD:.2})\">");
    svg.push_str(&body);
    svg.push_str("</g></svg>");

    Ok(WaveDromRender { svg, width_px: width.max(1.0), height_px: height.max(1.0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(s: &str) -> serde_json::Value {
        json5::from_str(s).unwrap()
    }

    #[test]
    fn parse_nested_expr_tree() {
        let v = json(r#"{"assign":[["out",["|",["&","a","b"],["&",["~","a"],"c"]]]]}"#);
        let assigns = parse_assign(v.get("assign").unwrap()).unwrap();
        assert_eq!(assigns.len(), 1);
        assert_eq!(assigns[0].output, "out");
        // root is OR with two AND children.
        match &assigns[0].expr {
            Expr::Gate { op, inputs } => {
                assert_eq!(op, "|");
                assert_eq!(inputs.len(), 2);
                // first child: & a b
                match &inputs[0] {
                    Expr::Gate { op, inputs } => {
                        assert_eq!(op, "&");
                        assert_eq!(inputs[0], Expr::Leaf("a".into()));
                        assert_eq!(inputs[1], Expr::Leaf("b".into()));
                    }
                    _ => panic!("expected AND gate"),
                }
                // second child: & (~ a) c  — nested NOT
                match &inputs[1] {
                    Expr::Gate { op, inputs } => {
                        assert_eq!(op, "&");
                        match &inputs[0] {
                            Expr::Gate { op, inputs } => {
                                assert_eq!(op, "~");
                                assert_eq!(inputs[0], Expr::Leaf("a".into()));
                            }
                            _ => panic!("expected NOT gate"),
                        }
                        assert_eq!(inputs[1], Expr::Leaf("c".into()));
                    }
                    _ => panic!("expected AND gate"),
                }
            }
            _ => panic!("expected OR root"),
        }
    }

    #[test]
    fn render_emits_glyphs_and_labels() {
        let v = json(r#"{"assign":[["out",["|",["&","a","b"],["&",["~","a"],"c"]]]]}"#);
        let r = render(v.get("assign").unwrap(), &WaveDromOptions::default()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.ends_with("</svg>"));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
        // gate glyphs emit <path>; the NOT bubble emits a <circle>.
        assert!(r.svg.contains("<path"), "expected gate/wire paths");
        assert!(r.svg.contains("<circle"), "expected a NOT-gate bubble");
        // input + output labels present.
        assert!(r.svg.contains(">a<"));
        assert!(r.svg.contains(">b<"));
        assert!(r.svg.contains(">c<"));
        assert!(r.svg.contains(">out<"));
    }

    #[test]
    fn xor_and_nand_distinguishable() {
        let v = json(r#"{"assign":[["y",["^",["~&","a","b"],"c"]]]}"#);
        let r = render(v.get("assign").unwrap(), &WaveDromOptions::default()).unwrap();
        // NAND contributes an output bubble (circle); both XOR and NAND are paths.
        assert!(r.svg.contains("<circle"), "NAND should draw a bubble");
        assert!(r.svg.contains(">y<"));
        assert!(r.svg.contains(">a<") && r.svg.contains(">b<") && r.svg.contains(">c<"));
    }

    #[test]
    fn unknown_op_falls_back_to_box() {
        // `mux` is not a known op → box fallback, must not panic.
        let v = json(r#"{"assign":[["q",["mux","a","b"]]]}"#);
        let r = render(v.get("assign").unwrap(), &WaveDromOptions::default()).unwrap();
        assert!(r.svg.contains("<rect"), "unknown op should draw a box");
        assert!(r.svg.contains(">mux<"), "box should be labeled with the op");
        assert!(r.svg.contains(">q<"));
    }

    #[test]
    fn multiple_assignments_stack() {
        let one = json(r#"{"assign":[["o1",["&","a","b"]]]}"#);
        let two = json(r#"{"assign":[["o1",["&","a","b"]],["o2",["|","c","d"]]]}"#);
        let r1 = render(one.get("assign").unwrap(), &WaveDromOptions::default()).unwrap();
        let r2 = render(two.get("assign").unwrap(), &WaveDromOptions::default()).unwrap();
        assert!(r2.height_px > r1.height_px, "two assignments taller than one");
        assert!(r2.svg.contains(">o2<"));
    }

    #[test]
    fn empty_assign_is_empty_error() {
        let v = json(r#"{"assign":[]}"#);
        assert_eq!(render(v.get("assign").unwrap(), &WaveDromOptions::default()), Err(WaveDromError::Empty));
    }
}
