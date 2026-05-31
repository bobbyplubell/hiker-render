//! Stage 2: measure a node's box size from its label + shape. Upstream: text
//! metrics (heuristic for v1).
//!
//! v1 uses a **font-free heuristic**: a fixed per-character advance times the
//! font size. This deliberately avoids a font dependency so `measure` (and thus
//! `layout`) is pure and cheap; the trade-off is that the boxes won't match the
//! real shaped glyph widths exactly (narrow text gets a bit of slack, very wide
//! glyphs may overflow slightly). It should later be replaced with real font
//! metrics (e.g. shaping via the same fontdb that `draw`'s `<text>` resolves
//! against) so the box hugs the rendered glyphs.

use crate::MermaidOptions;
use crate::model::NodeShape;

/// Average glyph advance as a fraction of the font size. ~0.6 em is a decent
/// mean for proportional sans-serif text (digits/lowercase narrower, caps/wide
/// glyphs wider) — a heuristic, see module docs.
const CHAR_ADVANCE_EM: f32 = 0.6;
/// Line height as a fraction of the font size.
const LINE_HEIGHT_EM: f32 = 1.2;

/// Minimum box dimensions so a 1-char or empty label isn't degenerate.
const MIN_W: f32 = 24.0;
const MIN_H: f32 = 20.0;

/// Heuristic intrinsic text-block size for `label` (no padding/shape allowance).
///
/// Multi-line (`\n`-separated) labels take the widest line for width and
/// `line_count` lines for height. An empty label still reports one line tall so
/// nodes keep a sensible minimum height.
fn text_size(label: &str, font_size: f32) -> (f32, f32) {
    let mut max_chars = 0usize;
    let mut lines = 0usize;
    for line in label.split('\n') {
        max_chars = max_chars.max(line.chars().count());
        lines += 1;
    }
    let lines = lines.max(1);
    let text_w = max_chars as f32 * font_size * CHAR_ADVANCE_EM;
    let text_h = lines as f32 * font_size * LINE_HEIGHT_EM;
    (text_w, text_h)
}

/// Return the `(width, height)` in CSS px for a node box that comfortably holds
/// `label` (with `opts` padding) given its `shape`. Diamonds/circles need extra
/// room than a rect for the same label.
///
/// Sizing is a heuristic (see module docs): text size from a per-char advance,
/// then a per-shape allowance so the label visibly fits inside the outline.
pub fn measure_node(label: &str, shape: NodeShape, opts: &MermaidOptions) -> (f32, f32) {
    let (text_w, text_h) = text_size(label, opts.font_size_px);
    let pad_x = opts.node_padding_x;
    let pad_y = opts.node_padding_y;

    let (mut w, mut h) = match shape {
        // Plain box: text + padding on each side.
        NodeShape::Rect | NodeShape::RoundRect => (text_w + 2.0 * pad_x, text_h + 2.0 * pad_y),

        // Stadium: padded box plus the two semicircular pill ends, each radius
        // h/2, so add a full `h` of horizontal room.
        NodeShape::Stadium => {
            let h = text_h + 2.0 * pad_y;
            (text_w + 2.0 * pad_x + h, h)
        }

        // Circle: the text box must be inscribed in the circle, so the circle's
        // diameter has to cover the box diagonal. d = diag(textbox) + padding,
        // and the node is square.
        NodeShape::Circle => {
            let diag = text_w.hypot(text_h);
            let d = diag + 2.0 * pad_x;
            (d, d)
        }

        // Diamond (rhombus): a centered axis-aligned box of size t_w × t_h fits
        // inside a rhombus of width W and height H when t_w/W + t_h/H <= 1. The
        // simple symmetric choice W = 2*t_w, H = 2*t_h satisfies that with room
        // to spare; we add padding to each text dimension first. This keeps the
        // label clear of the slanted edges.
        NodeShape::Diamond => {
            let w = 2.0 * (text_w + pad_x) + 2.0 * text_h; // widen for the slant
            let h = 2.0 * (text_h + pad_y);
            (w, h)
        }

        // Hexagon: vertical sides plus two slanted ends ~h/2 wide total, so add a
        // full `h` of horizontal room (like the stadium, but flat-topped).
        NodeShape::Hexagon => {
            let h = text_h + 2.0 * pad_y;
            (text_w + 2.0 * pad_x + h, h)
        }
    };

    w = w.max(MIN_W);
    h = h.max(MIN_H);
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    #[test]
    fn sizes_are_positive() {
        for shape in [
            NodeShape::Rect,
            NodeShape::RoundRect,
            NodeShape::Stadium,
            NodeShape::Circle,
            NodeShape::Diamond,
            NodeShape::Hexagon,
        ] {
            let (w, h) = measure_node("Hello", shape, &opts());
            assert!(w > 0.0 && h > 0.0, "{shape:?} -> ({w},{h})");
            assert!(w.is_finite() && h.is_finite());
        }
    }

    #[test]
    fn empty_label_hits_minimums() {
        let (w, h) = measure_node("", NodeShape::Rect, &opts());
        assert!(w >= MIN_W && h >= MIN_H);
    }

    #[test]
    fn width_grows_with_label_length() {
        let short = measure_node("A", NodeShape::Rect, &opts()).0;
        let long = measure_node("A much longer label here", NodeShape::Rect, &opts()).0;
        assert!(long > short, "long {long} should exceed short {short}");
    }

    #[test]
    fn diamond_and_circle_bigger_than_rect() {
        let label = "Decision point";
        let rect = measure_node(label, NodeShape::Rect, &opts());
        let diamond = measure_node(label, NodeShape::Diamond, &opts());
        let circle = measure_node(label, NodeShape::Circle, &opts());
        // Both width and height should exceed the plain rect for the same label.
        assert!(diamond.0 > rect.0 && diamond.1 > rect.1, "diamond {diamond:?} vs rect {rect:?}");
        assert!(circle.0 > rect.0 && circle.1 > rect.1, "circle {circle:?} vs rect {rect:?}");
    }

    #[test]
    fn multiline_is_taller() {
        let one = measure_node("one line", NodeShape::Rect, &opts());
        let two = measure_node("two\nlines", NodeShape::Rect, &opts());
        assert!(two.1 > one.1, "two-line {:?} should be taller than one-line {:?}", two, one);
    }

    #[test]
    fn multiline_width_is_widest_line() {
        // Width tracks the widest line, not the total char count.
        let (w, _) = measure_node("short\na very long line", NodeShape::Rect, &opts());
        let (wide, _) = measure_node("a very long line", NodeShape::Rect, &opts());
        assert!((w - wide).abs() < 0.001, "{w} vs {wide}");
    }

    #[test]
    fn stadium_wider_than_rect() {
        let label = "Pill";
        let rect = measure_node(label, NodeShape::Rect, &opts());
        let stadium = measure_node(label, NodeShape::Stadium, &opts());
        assert!(stadium.0 > rect.0, "stadium {stadium:?} vs rect {rect:?}");
    }
}
