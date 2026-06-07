//! Small SVG helpers shared by the diagram renderers: XML escaping, a font-free
//! text-size heuristic, and color formatting. Keeps each diagram module focused
//! on its own geometry instead of re-rolling these.

/// Average glyph advance as a fraction of the font size (font-free heuristic).
pub const CHAR_ADVANCE_EM: f32 = 0.6;
/// Line height as a fraction of the font size.
pub const LINE_HEIGHT_EM: f32 = 1.2;

/// XML-escape text for inclusion in SVG `<text>` content or an attribute value.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Intrinsic text size for a (possibly multi-line, `\n`) label: width = widest
/// line's advance, height = line count × `LINE_HEIGHT_EM` × font size.
///
/// Backed by real glyph advances from the bundled font (see [`crate::font`]) so
/// boxes hug the text — `iiii` is narrow, `WWWW` is wide. All diagram modules
/// call this, so sizing is consistent across types and matches what the
/// rasterizer draws.
pub fn text_size(label: &str, font_size: f32) -> (f32, f32) {
    crate::font::text_size(label, font_size)
}

/// `rgb(r,g,b)` string for a straight (un-premultiplied) RGBA color. Alpha is
/// emitted separately via [`opacity_attr`].
pub fn rgb(c: [u8; 4]) -> String {
    format!("rgb({},{},{})", c[0], c[1], c[2])
}

/// A fill value for shapes that should read as "hollow" by being filled with
/// the surface color (e.g. class-diagram inheritance triangles / hollow
/// diamonds occluding the line beneath them). Returns `"none"` when the surface
/// color is fully transparent (alpha 0) — on a transparent canvas there is no
/// surface color to fill with, so the shape is left unfilled rather than smeared
/// opaque (the transparent-canvas-paints-black bug). Otherwise `rgb(r,g,b)`.
pub fn surface_fill(c: [u8; 4]) -> String {
    if c[3] == 0 {
        "none".to_string()
    } else {
        rgb(c)
    }
}

/// Emit a label-backing `<rect>` behind text (edge/relationship labels), using
/// straight-RGBA `color` and corner radius `rx`. Skips entirely when `color` is
/// fully transparent (alpha 0): a transparent canvas has no surface color, so
/// painting a box would just be an opaque (often black) smear over the label.
/// Honors partial alpha via `fill-opacity`.
pub fn label_bg_rect(svg: &mut String, x: f32, y: f32, w: f32, h: f32, rx: f32, color: [u8; 4]) {
    use std::fmt::Write as _;
    if color[3] == 0 {
        return;
    }
    let rx_attr = if rx > 0.0 {
        format!(" rx=\"{rx:.2}\"")
    } else {
        String::new()
    };
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\"{rx_attr} \
         fill=\"{fill}\"{fo}/>",
        fill = rgb(color),
        fo = opacity_attr("fill-opacity", color),
    );
}

/// An ` <name>="<a>"` opacity attribute (e.g. `name = "fill-opacity"`) when the
/// color is not fully opaque, else an empty string.
pub fn opacity_attr(name: &str, c: [u8; 4]) -> String {
    if c[3] < 255 {
        format!(" {name}=\"{:.4}\"", c[3] as f32 / 255.0)
    } else {
        String::new()
    }
}

/// Build the run of extra `<text>` attributes for a label from an [`ElemStyle`]'s
/// `font_weight` / `font_style` / `text_decoration` overrides (each `None` →
/// omitted). Each present attribute is space-prefixed and value-escaped so it
/// appends cleanly onto a `<text>` tag. `font_size` and `opacity` are handled
/// separately by callers (they also affect layout / the shape element).
pub fn text_style_attrs(style: &crate::model::ElemStyle) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    if let Some(w) = &style.font_weight {
        let _ = write!(s, " font-weight=\"{}\"", escape(w));
    }
    if let Some(st) = &style.font_style {
        let _ = write!(s, " font-style=\"{}\"", escape(st));
    }
    if let Some(d) = &style.text_decoration {
        let _ = write!(s, " text-decoration=\"{}\"", escape(d));
    }
    s
}

/// ` opacity="…"` for a present 0..1 element opacity (CSS `opacity:`), else an
/// empty string. Applied to a shape element to fade its whole fill+stroke.
pub fn element_opacity_attr(opacity: Option<f32>) -> String {
    match opacity {
        Some(o) => format!(" opacity=\"{:.4}\"", o.clamp(0.0, 1.0)),
        None => String::new(),
    }
}

/// Build an SVG path `d` that draws a SMOOTH curve through `points` (a
/// Catmull-Rom spline converted to cubic Béziers — it passes through every
/// point, so clipped endpoints and arrowheads stay aligned). Falls back to a
/// straight polyline for fewer than 3 points. Returns just the `d` value
/// (e.g. `"M0,0 C..."`).
///
/// Deterministic. Uses the same `{:.2}` number formatting as the rest of this
/// module.
pub fn smooth_path_d(points: &[(f32, f32)]) -> String {
    use std::fmt::Write as _;
    match points.len() {
        0 => return String::new(),
        1 => return format!("M{:.2},{:.2}", points[0].0, points[0].1),
        2 => {
            return format!(
                "M{:.2},{:.2} L{:.2},{:.2}",
                points[0].0, points[0].1, points[1].0, points[1].1
            )
        }
        _ => {}
    }
    let n = points.len();
    let p = |i: isize| -> (f32, f32) {
        let i = i.clamp(0, (n - 1) as isize) as usize;
        points[i]
    };
    let mut d = format!("M{:.2},{:.2}", points[0].0, points[0].1);
    for i in 0..n - 1 {
        let p0 = p(i as isize - 1);
        let p1 = p(i as isize);
        let p2 = p(i as isize + 1);
        let p3 = p(i as isize + 2);
        // Catmull-Rom → cubic Bézier control points.
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        let _ = write!(
            d,
            " C{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
            c1.0, c1.1, c2.0, c2.1, p2.0, p2.1
        );
    }
    d
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    (dx * dx + dy * dy).sqrt()
}

/// Anchor point for an edge's label: the polyline midpoint (by arc length),
/// nudged **perpendicular to the route** so that parallel / bidirectional edges
/// between the same node pair don't stack their labels on top of each other.
///
/// `index`/`count` describe this edge's position within its parallel group — a
/// lone edge is `index = 0, count = 1` and gets the plain midpoint; for `count
/// > 1` the labels are spread symmetrically across the line (one above, one
/// below for a bidirectional pair). Returns `None` for an empty polyline.
pub fn edge_label_anchor(
    points: &[(f32, f32)],
    index: usize,
    count: usize,
    font_size: f32,
) -> Option<(f32, f32)> {
    match points.len() {
        0 => return None,
        1 => return Some(points[0]),
        _ => {}
    }
    // Lone edge → centered on the line (caller draws a background behind it).
    if count <= 1 {
        let (pt, _) = point_at_fraction(points, 0.5);
        return Some(pt);
    }
    // Parallel/bidirectional group: stagger the labels ALONG the edge (so wide
    // labels don't overlap even on a short route) AND nudge them to alternating
    // sides perpendicular to the route — together this separates a bidirectional
    // pair both lengthwise and sideways.
    let frac = (index as f32 + 1.0) / (count as f32 + 1.0);
    let (pt, dir) = point_at_fraction(points, frac);
    let perp = (-dir.1, dir.0);
    let side = if index % 2 == 0 { -1.0 } else { 1.0 };
    let offset = side * font_size * 0.9;
    Some((pt.0 + perp.0 * offset, pt.1 + perp.1 * offset))
}

/// The point at fraction `f` (0..=1) of a polyline's cumulative arc length, plus
/// the unit direction of the segment it lands on.
fn point_at_fraction(points: &[(f32, f32)], f: f32) -> ((f32, f32), (f32, f32)) {
    let total: f32 = points.windows(2).map(|w| dist(w[0], w[1])).sum();
    let target = total * f.clamp(0.0, 1.0);
    let mut acc = 0.0;
    for w in points.windows(2) {
        let seg = dist(w[0], w[1]);
        if acc + seg >= target || seg == 0.0 {
            let t = if seg > 0.0 { (target - acc) / seg } else { 0.0 };
            let pt = (w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t);
            let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
            let len = (dx * dx + dy * dy).sqrt().max(1e-3);
            return (pt, (dx / len, dy / len));
        }
        acc += seg;
    }
    (*points.last().unwrap(), (1.0, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_edge_labels_are_separated() {
        // A horizontal route; two parallel edges should be staggered along the
        // line (different x) AND on opposite sides (different y), so wide labels
        // can't overlap.
        let route = [(0.0, 0.0), (100.0, 0.0)];
        let a = edge_label_anchor(&route, 0, 2, 16.0).unwrap();
        let b = edge_label_anchor(&route, 1, 2, 16.0).unwrap();
        let along = (a.0 - b.0).abs();
        let across = (a.1 - b.1).abs();
        assert!(along > 20.0, "staggered along the line: {a:?} {b:?}");
        assert!(across > 20.0, "opposite sides: {a:?} {b:?}");
        // A lone edge sits on the midpoint.
        let lone = edge_label_anchor(&route, 0, 1, 16.0).unwrap();
        assert!((lone.0 - 50.0).abs() < 0.001 && lone.1.abs() < 0.001, "lone label at midpoint");
    }

    #[test]
    fn smooth_path_curves_and_passes_through_endpoints() {
        // >= 3 points → a smooth cubic-Bézier path through every point.
        let pts = [(0.0, 0.0), (10.0, 20.0), (30.0, 10.0), (50.0, 40.0)];
        let d = smooth_path_d(&pts);
        assert!(d.starts_with('M'), "starts with M: {d}");
        assert!(d.contains('C'), "uses cubic Béziers: {d}");
        // Deterministic.
        assert_eq!(d, smooth_path_d(&pts));
        // The path's first command targets the first point and its last anchor is
        // the last point (the curve passes through both endpoints exactly).
        assert!(d.starts_with("M0.00,0.00"), "starts at first point: {d}");
        assert!(d.ends_with("50.00,40.00"), "ends at last point: {d}");

        // 2 points → a straight line.
        let two = smooth_path_d(&[(1.0, 2.0), (3.0, 4.0)]);
        assert!(two.contains('L'), "two points → polyline: {two}");
        assert_eq!(two, "M1.00,2.00 L3.00,4.00");

        // Degenerate inputs.
        assert_eq!(smooth_path_d(&[(5.0, 6.0)]), "M5.00,6.00");
        assert_eq!(smooth_path_d(&[]), "");
    }

    #[test]
    fn escapes_xml() {
        assert_eq!(escape("a & b < c > d \"e\""), "a &amp; b &lt; c &gt; d &quot;e&quot;");
    }

    #[test]
    fn text_size_scales() {
        let (w1, h1) = text_size("a", 16.0);
        let (w2, h2) = text_size("aaaa", 16.0);
        assert!(w2 > w1 && (h2 - h1).abs() < 0.001);
        let (_, h3) = text_size("a\nb", 16.0);
        assert!(h3 > h1);
    }

    #[test]
    fn color_formatting() {
        assert_eq!(rgb([1, 2, 3, 255]), "rgb(1,2,3)");
        assert_eq!(opacity_attr("fill-opacity", [0, 0, 0, 255]), "");
        assert_eq!(opacity_attr("fill-opacity", [0, 0, 0, 128]), " fill-opacity=\"0.5020\"");
    }
}
