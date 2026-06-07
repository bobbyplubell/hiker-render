//! Class-diagram drawing: emit the SVG document (class boxes with three
//! compartments, UML relationship markers, edge labels, and notes) from a
//! laid-out [`super::layout::Layout`], and derive per-class hit regions.

use std::fmt::Write as _;

use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb, text_size, LINE_HEIGHT_EM};
use crate::{HitRegion, MermaidError, MermaidOptions, MermaidRender};

use super::layout;
use super::model;
use super::parse::parse;

const STROKE_W: f32 = 1.5;
/// Marker triangle / diamond / arrow length, px.
const MARK_LEN: f32 = 12.0;
const MARK_HALF: f32 = 7.0;

fn fill_attrs(color: [u8; 4]) -> (String, String) {
    (rgb(color), opacity_attr("fill-opacity", color))
}
fn stroke_attrs(color: [u8; 4]) -> (String, String) {
    (rgb(color), opacity_attr("stroke-opacity", color))
}

/// Emit one class box (rect + dividers + three text bands).
fn emit_box(svg: &mut String, b: &layout::Positioned, class: &model::Class, opts: &MermaidOptions) {
    let g = &b.geom;
    let x = b.cx - g.w / 2.0;
    let y = b.cy - g.h / 2.0;
    // Per-class style overrides, falling back to the theme defaults.
    let (fill, fo) = fill_attrs(class.style.fill.unwrap_or(opts.node_fill));
    let (stroke, so) = stroke_attrs(class.style.stroke.unwrap_or(opts.node_stroke));
    let sw = class.style.stroke_width.unwrap_or(STROKE_W);
    // `opacity:` fades the whole box; font-style/decoration ride on the member
    // `<text>` rows. (The class name keeps its UML bold; font-size deferred for
    // class — it would desync the fixed compartment row layout.)
    let op = crate::svgutil::element_opacity_attr(class.style.opacity);
    let text_attrs = crate::svgutil::text_style_attrs(&class.style);

    // Outer rect.
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"{op}/>",
        w = g.w,
        h = g.h,
    );

    // Divider lines between compartments.
    let div1 = y + g.name_h;
    let div2 = y + g.name_h + g.attr_h;
    for dy in [div1, div2] {
        let _ = write!(
            svg,
            "<line x1=\"{x1:.2}\" y1=\"{dy:.2}\" x2=\"{x2:.2}\" y2=\"{dy:.2}\" \
             stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
            x1 = x,
            x2 = x + g.w,
        );
    }

    // Name compartment. When a stereotype is present it sits centered in
    // italics ABOVE the (bold) class name, UML-style; the band is two lines tall.
    let (tfill, tfo) = fill_attrs(class.style.text_color.unwrap_or(opts.text_color));
    let family = escape(&opts.font_family);
    let fs = opts.font_size_px;
    let line_h = fs * LINE_HEIGHT_EM;
    let (ann_cy, name_cy) = if class.annotation.is_some() {
        let band_mid = y + g.name_h / 2.0;
        (band_mid - line_h / 2.0, band_mid + line_h / 2.0)
    } else {
        (0.0, y + g.name_h / 2.0)
    };
    if let Some(ann) = &class.annotation {
        let _ = write!(
            svg,
            "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{fs}\" font-style=\"italic\" fill=\"{tfill}\"{tfo}>{}</text>",
            escape(&format!("«{ann}»")),
            cx = b.cx,
            cy = ann_cy,
        );
    }
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" font-weight=\"bold\" fill=\"{tfill}\"{tfo}>{}</text>",
        escape(&class.display_name),
        cx = b.cx,
        cy = name_cy,
    );

    // Attributes (left-aligned) in the middle band.
    let text_x = x + opts.node_padding_x;
    let attr_top = div1;
    emit_lines(svg, &class.attributes, text_x, attr_top, line_h, opts, &tfill, &tfo, &family, fs, &text_attrs);

    // Methods (left-aligned) in the bottom band.
    let method_top = div2;
    emit_lines(svg, &class.methods, text_x, method_top, line_h, opts, &tfill, &tfo, &family, fs, &text_attrs);
}

#[allow(clippy::too_many_arguments)]
fn emit_lines(
    svg: &mut String,
    members: &[model::Member],
    x: f32,
    band_top: f32,
    line_h: f32,
    opts: &MermaidOptions,
    fill: &str,
    fo: &str,
    family: &str,
    fs: f32,
    extra_attrs: &str,
) {
    let pad_y = opts.node_padding_y;
    for (i, m) in members.iter().enumerate() {
        let cy = band_top + pad_y / 2.0 + line_h * (i as f32 + 0.5);
        let _ = write!(
            svg,
            "<text x=\"{x:.2}\" y=\"{cy:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}{extra_attrs}>{}</text>",
            escape(&m.text),
        );
    }
}

/// Pull the marker end of a polyline back by `amount` so the marker tip lands on
/// the box border. `at_to` trims the last point, else the first.
fn pullback(pts: &mut [(f32, f32)], at_to: bool, amount: f32) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let (tip_i, prev_i) = if at_to { (n - 1, n - 2) } else { (0, 1) };
    let (tx, ty) = pts[tip_i];
    let (px, py) = pts[prev_i];
    let (dx, dy) = (tx - px, ty - py);
    let len = dx.hypot(dy);
    if len <= amount || len == 0.0 {
        return;
    }
    let t = (len - amount) / len;
    pts[tip_i] = (px + dx * t, py + dy * t);
}

/// Emit a relationship polyline + its end marker + optional label.
fn emit_relation(svg: &mut String, r: &layout::RoutedRel, rel: &model::Relation, opts: &MermaidOptions) {
    if r.points.len() < 2 {
        return;
    }
    let (stroke, so) = stroke_attrs(opts.edge_stroke);

    let mut pts = r.points.clone();
    // The marker sits at the from-end when `marker_at_to` is false. dagre routes
    // source→target, i.e. points[0] is `from`, last is `to`.
    let has_marker = rel.marker != model::RelMarker::None;
    if has_marker {
        pullback(&mut pts, rel.marker_at_to, MARK_LEN);
    }

    // Smooth curve through the (already marker-shortened) points; the marker is
    // drawn separately from the original un-shortened points below.
    let d = crate::svgutil::smooth_path_d(&pts);
    let dash = if rel.dashed { " stroke-dasharray=\"5 4\"" } else { "" };
    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"{dash}/>",
        d.trim_end(),
    );

    // Marker: oriented along the terminal segment at the marker end.
    if has_marker {
        // Use the un-pulled-back original points for the tip & direction.
        let (tip, prev) = if rel.marker_at_to {
            (r.points[r.points.len() - 1], r.points[r.points.len() - 2])
        } else {
            (r.points[0], r.points[1])
        };
        emit_marker(svg, rel.marker, tip, prev, opts);
    }

    // Label at dagre's reserved center when available; otherwise the route
    // midpoint, nudged perpendicular for parallel groups.
    if let Some(label) = &rel.label {
        if !label.is_empty() {
            let anchor = match r.dagre_label {
                Some(p) => Some((p.x, p.y)),
                None => {
                    edge_label_anchor(&r.points, r.label_index, r.label_count, opts.font_size_px)
                }
            };
            if let Some((mx, my)) = anchor {
                emit_label(svg, label, mx, my, opts);
            }
        }
    }
}

/// Draw the UML marker polygon at `tip`, pointing from `prev → tip`.
fn emit_marker(
    svg: &mut String,
    marker: model::RelMarker,
    tip: (f32, f32),
    prev: (f32, f32),
    opts: &MermaidOptions,
) {
    let (dx, dy) = (tip.0 - prev.0, tip.1 - prev.1);
    let len = dx.hypot(dy);
    let (ux, uy) = if len > 0.0 { (dx / len, dy / len) } else { (1.0, 0.0) };
    // perpendicular
    let (perpx, perpy) = (-uy, ux);

    let (stroke, so) = stroke_attrs(opts.edge_stroke);
    // Base point: back along the line from the tip by MARK_LEN.
    let base = (tip.0 - ux * MARK_LEN, tip.1 - uy * MARK_LEN);
    let half = MARK_HALF;
    let b1 = (base.0 + perpx * half, base.1 + perpy * half);
    let b2 = (base.0 - perpx * half, base.1 - perpy * half);

    match marker {
        model::RelMarker::Triangle => {
            // Hollow triangle (inheritance / realization): tip + two base corners,
            // filled with the canvas background so it reads as hollow on any theme.
            let _ = write!(
                svg,
                "<polygon points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" \
                 fill=\"{bg}\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
                tip.0, tip.1, b1.0, b1.1, b2.0, b2.1,
                bg = crate::svgutil::surface_fill(opts.edge_label_bg),
            );
        }
        model::RelMarker::Arrow => {
            // Open arrow: two strokes from base corners to tip (no fill).
            let _ = write!(
                svg,
                "<polyline points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" \
                 fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
                b1.0, b1.1, tip.0, tip.1, b2.0, b2.1,
            );
        }
        model::RelMarker::DiamondHollow | model::RelMarker::DiamondFilled => {
            // Diamond: tip, side1, far corner, side2. far = base extended one more
            // MARK_LEN back.
            let far = (tip.0 - ux * 2.0 * MARK_LEN, tip.1 - uy * 2.0 * MARK_LEN);
            let fill = if marker == model::RelMarker::DiamondFilled {
                stroke.clone()
            } else {
                crate::svgutil::surface_fill(opts.edge_label_bg)
            };
            let _ = write!(
                svg,
                "<polygon points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" \
                 fill=\"{fill}\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
                tip.0, tip.1, b1.0, b1.1, far.0, far.1, b2.0, b2.1,
            );
        }
        model::RelMarker::None => {}
    }
}

fn emit_label(svg: &mut String, label: &str, cx: f32, cy: f32, opts: &MermaidOptions) {
    let fs = opts.font_size_px;
    let (w, h) = text_size(label, fs);
    let pad = 2.0;
    let bw = w + 2.0 * pad;
    let bh = h + 2.0 * pad;
    crate::svgutil::label_bg_rect(svg, cx - bw / 2.0, cy - bh / 2.0, bw, bh, 0.0, opts.edge_label_bg);
    let (tfill, tfo) = fill_attrs(opts.text_color);
    let family = escape(&opts.font_family);
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{tfill}\"{tfo}>{}</text>",
        escape(label),
    );
}

/// Note placement: gap between a class box and its attached note, and the
/// note's inner text padding.
const NOTE_GAP: f32 = 24.0;
const NOTE_PAD: f32 = 8.0;
/// Size of the folded "dog-ear" corner.
const NOTE_FOLD: f32 = 10.0;

/// Geometry for one positioned note rectangle.
struct NoteGeom {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    text: String,
}

/// Position every note. Attached notes sit to the right of their class box,
/// aligned to the box top; floating notes stack at the top-left. All notes are
/// laid out so their coordinates stay non-negative and the caller grows the
/// canvas to include them (no shifting of existing content).
fn layout_notes(diagram: &model::ClassDiagram, lay: &layout::Layout, opts: &MermaidOptions) -> Vec<NoteGeom> {
    let fs = opts.font_size_px;
    let mut out = Vec::new();
    let mut float_y = 0.0_f32;
    for note in &diagram.notes {
        let (tw, th) = text_size(&note.text, fs);
        let w = tw + 2.0 * NOTE_PAD + NOTE_FOLD;
        let h = th + 2.0 * NOTE_PAD;
        let (x, y) = match &note.for_class {
            Some(id) => {
                // Find the attached class box; place the note to its right.
                let placed = lay
                    .boxes
                    .iter()
                    .find(|b| diagram.classes[b.class_idx].name == *id)
                    .map(|b| {
                        let g = &b.geom;
                        (b.cx + g.w / 2.0 + NOTE_GAP, b.cy - g.h / 2.0)
                    });
                placed.unwrap_or_else(|| {
                    let y = float_y;
                    float_y += h + NOTE_GAP;
                    (0.0, y)
                })
            }
            None => {
                let y = float_y;
                float_y += h + NOTE_GAP;
                (0.0, y)
            }
        };
        out.push(NoteGeom { x, y, w, h, text: note.text.clone() });
    }
    out
}

/// Emit a single note: a pale rectangle with a folded top-right corner and the
/// note text.
fn emit_note(svg: &mut String, n: &NoteGeom, opts: &MermaidOptions) {
    let (x, y, w, h) = (n.x, n.y, n.w, n.h);
    // Pale fill: blend the node fill toward the background a little. Use a
    // light, semi-distinct note color derived from the theme text color at low
    // opacity over the background, but keep it deterministic & simple: a fixed
    // pale yellow-ish tone that reads as a sticky note on any theme.
    let note_fill = "#fff5ad";
    let (stroke, so) = stroke_attrs(opts.node_stroke);
    let f = NOTE_FOLD;
    // Body outline with the top-right corner folded in.
    let _ = write!(
        svg,
        "<path d=\"M{x:.2},{y:.2} L{xr:.2},{y:.2} L{xrr:.2},{yf:.2} \
         L{xrr:.2},{yb:.2} L{x:.2},{yb:.2} Z\" \
         fill=\"{note_fill}\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        xr = x + w - f,
        xrr = x + w,
        yf = y + f,
        yb = y + h,
    );
    // The fold triangle.
    let _ = write!(
        svg,
        "<path d=\"M{xr:.2},{y:.2} L{xr:.2},{yf:.2} L{xrr:.2},{yf:.2} Z\" \
         fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        xr = x + w - f,
        xrr = x + w,
        yf = y + f,
    );
    // Text, left-aligned with padding, vertically centered.
    let (tfill, tfo) = fill_attrs(opts.text_color);
    let family = escape(&opts.font_family);
    let fs = opts.font_size_px;
    let _ = write!(
        svg,
        "<text x=\"{tx:.2}\" y=\"{ty:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{tfill}\"{tfo}>{}</text>",
        escape(&n.text),
        tx = x + NOTE_PAD,
        ty = y + h / 2.0,
    );
}

/// Final canvas size including any notes (notes only ever extend right/down).
fn canvas_size(diagram: &model::ClassDiagram, lay: &layout::Layout, opts: &MermaidOptions) -> (f32, f32) {
    let notes = layout_notes(diagram, lay, opts);
    let mut w = lay.width;
    let mut h = lay.height;
    for n in &notes {
        w = w.max(n.x + n.w);
        h = h.max(n.y + n.h);
    }
    ((w.ceil() + 1.0).max(1.0), (h.ceil() + 1.0).max(1.0))
}

fn draw(diagram: &model::ClassDiagram, lay: &layout::Layout, opts: &MermaidOptions) -> String {
    let notes = layout_notes(diagram, lay, opts);
    let (w, h) = canvas_size(diagram, lay, opts);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Relationships under boxes.
    for r in &lay.rels {
        emit_relation(&mut svg, r, &diagram.relations[r.rel_idx], opts);
    }
    // Class boxes on top.
    for b in &lay.boxes {
        emit_box(&mut svg, b, &diagram.classes[b.class_idx], opts);
    }
    // Notes on top of everything.
    for n in &notes {
        emit_note(&mut svg, n, opts);
    }

    svg.push_str("</svg>");
    svg
}

/// Shared pipeline for [`render_class`] / [`render_class_with_regions`]: parse →
/// layout → draw, deriving the hit regions from the very same positioned boxes.
pub(super) fn render(
    src: &str,
    opts: &MermaidOptions,
) -> Result<(MermaidRender, Vec<HitRegion>), MermaidError> {
    let diagram = parse(src).map_err(MermaidError::Parse)?;
    if diagram.classes.is_empty() {
        return Err(MermaidError::Empty);
    }
    let lay = layout::layout(&diagram, opts);
    let svg = draw(&diagram, &lay, opts);
    let (width_px, height_px) = canvas_size(&diagram, &lay, opts);
    let regions: Vec<HitRegion> = lay
        .boxes
        .iter()
        .map(|b| {
            let class = &diagram.classes[b.class_idx];
            HitRegion {
                id: class.name.clone(),
                x: b.cx - b.geom.w / 2.0,
                y: b.cy - b.geom.h / 2.0,
                w: b.geom.w,
                h: b.geom.h,
                link: class.link.clone(),
                callback: class.callback.clone(),
                tooltip: class.tooltip.clone(),
            }
        })
        .collect();
    Ok((
        MermaidRender {
            svg,
            width_px,
            height_px,
        },
        regions,
    ))
}
