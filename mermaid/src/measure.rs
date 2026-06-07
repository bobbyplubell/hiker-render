//! Stage 2: measure a node's box size from its label + shape.
//!
//! The label's intrinsic text size comes from [`crate::label::measure`], which
//! handles the rich-label features (markdown emphasis, `<br>`/`\n` lines, and
//! inline `$…$` math) and falls back to real font metrics ([`crate::font`]) for
//! plain labels. This module adds the per-shape padding/allowance so the label
//! visibly fits inside the outline.

use crate::MermaidOptions;
use crate::model::NodeShape;

/// Minimum box dimensions so a 1-char or empty label isn't degenerate.
const MIN_W: f32 = 24.0;
const MIN_H: f32 = 20.0;

/// Return the `(width, height)` in CSS px for a node box that comfortably holds
/// `label` (with `opts` padding) given its `shape`. Diamonds/circles need extra
/// room than a rect for the same label.
///
/// Sizing is a heuristic (see module docs): text size from a per-char advance,
/// then a per-shape allowance so the label visibly fits inside the outline.
pub fn measure_node(label: &str, shape: NodeShape, opts: &MermaidOptions) -> (f32, f32) {
    // Rich measurement: bold/italic widen, `<br>`/`\n` add lines, `$…$` math
    // contributes its rendered box. Plain labels measure exactly as before.
    let (text_w, text_h) = crate::label::measure(label, opts.font_size_px);
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

        // Cylinder (database): a padded box plus vertical room for the top
        // ellipse cap (~10px) so the label clears it.
        NodeShape::Cylinder => (text_w + 2.0 * pad_x, text_h + 2.0 * pad_y + 10.0),

        // Subroutine: a padded box plus horizontal room for the two inner bars
        // (~8px inset each side).
        NodeShape::Subroutine => (text_w + 2.0 * pad_x + 16.0, text_h + 2.0 * pad_y),

        // Document: a padded box plus a little extra at the bottom for the wave.
        NodeShape::Document => (text_w + 2.0 * pad_x, text_h + 2.0 * pad_y + 8.0),

        // Parallelogram/Trapezoid: a padded box plus horizontal room for the
        // slanted left/right edges so the label clears them.
        NodeShape::Parallelogram
        | NodeShape::ParallelogramAlt
        | NodeShape::Trapezoid
        | NodeShape::TrapezoidAlt => (text_w + 2.0 * pad_x + 20.0, text_h + 2.0 * pad_y),

        // Double circle: like Circle, plus a few px for the outer ring.
        NodeShape::DoubleCircle => {
            let diag = text_w.hypot(text_h);
            let d = diag + 2.0 * pad_x + 8.0;
            (d, d)
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
            NodeShape::Cylinder,
            NodeShape::Subroutine,
            NodeShape::Document,
            NodeShape::Parallelogram,
            NodeShape::ParallelogramAlt,
            NodeShape::Trapezoid,
            NodeShape::TrapezoidAlt,
            NodeShape::DoubleCircle,
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
