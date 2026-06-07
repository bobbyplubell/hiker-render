//! SVG emitter: walk a laid-out [`Box`] tree and produce one self-contained SVG
//! document.
//!
//! Each glyph is emitted as an outline `<path>` (no font reference), so the result
//! rasterizes with resvg and no installed fonts. Font glyph coordinates are y-UP
//! with the baseline at y=0; SVG is y-DOWN, so a single
//! `translate(penX, baseline) scale(scale, -scale)` per glyph handles both the
//! units→px scaling and the y-flip.

use std::fmt::Write as _;

use ttf_parser::{Face, OutlineBuilder};

use super::box_layout::{Box, BoxKind};
use super::MathOptions;

/// STIX Two Math is a 1000-unit em (kept here so the scale reads clearly).
const UNITS_PER_EM: f32 = 1000.0;
/// A little vertical padding so glyphs don't touch the SVG edges.
const PAD_PX: f32 = 1.0;

/// Render the laid-out `root` box to an SVG document plus its placement metrics.
///
/// Returns `(svg, width_px, height_px, baseline_px)` where `baseline_px` is the
/// distance from the top of the box down to the math baseline.
pub fn emit(root: &Box, face: &Face<'static>, opts: &MathOptions) -> (String, f32, f32, f32) {
    let _ = UNITS_PER_EM; // each glyph now carries its own units→px scale.
    let width = root.width;
    // Ascent/descent come from the row's max height/depth, plus a little padding.
    let ascent = root.height + PAD_PX;
    let descent = root.depth + PAD_PX;
    let height = ascent + descent;

    let _ = opts; // each glyph/rule now carries its own RGBA color.

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\">"
    );

    // Walk the tree; the baseline sits `ascent` px below the SVG top.
    emit_box(&mut svg, root, face, 0.0, ascent);

    svg.push_str("</svg>");
    (svg, width, height, ascent)
}

/// Recursively emit `b` at SVG coordinates `(x, baseline)` (the box's left edge and
/// the shared baseline). Each glyph and rule fills with the straight RGBA color it
/// recorded at layout time, so a `\color`/`\textcolor` scope paints just its glyphs.
fn emit_box(svg: &mut String, b: &Box, face: &Face<'static>, x: f32, baseline: f32) {
    match &b.kind {
        BoxKind::Glyph { gid, scale, color } => {
            let scale = *scale;
            let (fill, opacity_attr) = fill_attrs(*color);
            let mut path = SvgPath::default();
            // outline_glyph fills the path in font units; the transform scales + flips.
            if face.outline_glyph(*gid, &mut path).is_some() && !path.d.is_empty() {
                let _ = write!(
                    svg,
                    "<path d=\"{}\" transform=\"translate({x},{baseline}) scale({scale},{neg})\" \
                     fill=\"{fill}\"{opacity_attr}/>",
                    path.d,
                    neg = -scale,
                );
            }
        }
        BoxKind::Rule { width, thickness, color } => {
            // A filled bar drawn from the box origin upward: the box's baseline is
            // at `baseline`, and the rule extends `thickness` px above it (SVG y
            // grows down, so the rect's top y is `baseline - thickness`).
            if *width > 0.0 && *thickness > 0.0 {
                let (fill, opacity_attr) = fill_attrs(*color);
                let y = baseline - thickness;
                let _ = write!(
                    svg,
                    "<rect x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{thickness}\" \
                     fill=\"{fill}\"{opacity_attr}/>",
                );
            }
        }
        BoxKind::Line { dx, dy, thickness, color } => {
            // A stroke from the box origin (its baseline left corner) to
            // `origin + (dx, dy)`; `dy` grows downward like a child's shift, which
            // matches SVG's y-down space directly. Honors the straight RGBA color
            // (alpha < 255 → `stroke-opacity`) as the glyph/rule fills do.
            if *thickness > 0.0 {
                let (stroke, opacity_attr) = fill_attrs(*color);
                let opacity_attr = opacity_attr.replace("fill-opacity", "stroke-opacity");
                let (x2, y2) = (x + dx, baseline + dy);
                let _ = write!(
                    svg,
                    "<line x1=\"{x}\" y1=\"{baseline}\" x2=\"{x2}\" y2=\"{y2}\" \
                     stroke=\"{stroke}\" stroke-width=\"{thickness}\"{opacity_attr}/>",
                );
            }
        }
        BoxKind::Fill { width, height, depth, color } => {
            // A solid rectangle spanning the box's full bbox: from `height` px above
            // the baseline down to `depth` px below it (SVG y grows down, so the top
            // y is `baseline - height` and the rect is `height + depth` tall).
            if *width > 0.0 && (*height + *depth) > 0.0 {
                let (fill, opacity_attr) = fill_attrs(*color);
                let y = baseline - height;
                let h = height + depth;
                let _ = write!(
                    svg,
                    "<rect x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{h}\" \
                     fill=\"{fill}\"{opacity_attr}/>",
                );
            }
        }
        BoxKind::Hbox { children } => {
            for child in children {
                // `dy` shifts the child's baseline downward (SVG y grows down).
                emit_box(svg, &child.b, face, x + child.dx, baseline + child.dy);
            }
        }
    }
}

/// Build the `fill="rgb(r,g,b)"` value and an optional ` fill-opacity="…"` attribute
/// from a straight RGBA color (alpha < 255 emits `fill-opacity`; opaque omits it).
fn fill_attrs(color: [u8; 4]) -> (String, String) {
    let [r, g, b, a] = color;
    let fill = format!("rgb({r},{g},{b})");
    let opacity_attr = if a < 255 {
        format!(" fill-opacity=\"{:.4}\"", a as f32 / 255.0)
    } else {
        String::new()
    };
    (fill, opacity_attr)
}

/// An [`OutlineBuilder`] that accumulates an SVG path `d` string in font units
/// (y-up); the caller's `scale(scale, -scale)` transform converts to SVG space.
#[derive(Default)]
struct SvgPath {
    d: String,
}

impl OutlineBuilder for SvgPath {
    fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.d, "M{x} {y}");
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.d, "L{x} {y}");
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(self.d, "Q{x1} {y1} {x} {y}");
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(self.d, "C{x1} {y1} {x2} {y2} {x} {y}");
    }
    fn close(&mut self) {
        self.d.push('Z');
    }
}
