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

/// An ` <name>="<a>"` opacity attribute (e.g. `name = "fill-opacity"`) when the
/// color is not fully opaque, else an empty string.
pub fn opacity_attr(name: &str, c: [u8; 4]) -> String {
    if c[3] < 255 {
        format!(" {name}=\"{:.4}\"", c[3] as f32 / 255.0)
    } else {
        String::new()
    }
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
