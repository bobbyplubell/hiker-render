//! Quadrant-chart diagram (self-contained: parse + draw, no graph layout).
//!
//! Mermaid `quadrantChart` syntax (the subset we support):
//! ```text
//! quadrantChart
//!     title Reach and engagement
//!     x-axis Low Reach --> High Reach
//!     y-axis Low Engagement --> High Engagement
//!     quadrant-1 We should expand
//!     quadrant-2 Need to promote
//!     quadrant-3 Re-evaluate
//!     quadrant-4 May be improved
//!     Campaign A: [0.3, 0.6]
//!     Campaign B: [0.45, 0.23]
//! ```
//! The header line is `quadrantChart`. Directives: `title <text>`;
//! `x-axis <left> --> <right>` (the `--> <right>` part optional); `y-axis
//! <bottom> --> <top>`; `quadrant-1`..`quadrant-4 <label>`. In mermaid's order
//! the quadrants are: q1 = top-right, q2 = top-left, q3 = bottom-left, q4 =
//! bottom-right. Point lines are `Name: [<x>, <y>]` with x,y in 0..1. Blank
//! lines and `%%` comments are ignored.
//!
//! Layout is a square plot (no graph layout): two crossing lines through the
//! center split it into four quadrant cells (each faintly tinted), the quadrant
//! labels are centered in their cells, the x-axis labels run along the bottom
//! (left at left, right at right), the y-axis labels run up the left side
//! (bottom at bottom, top at top), and each point is a small circle at
//! `(x*size, (1-y)*size)` — y is inverted so 0 is at the bottom — with its name
//! label beside it. The title is centered on top.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/quadrant-chart/` for the
//! upstream builder/renderer this mirrors.

use std::fmt::Write as _;

use crate::svgutil::{escape, rgb};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// A plotted point: a name and its (x, y) in the unit square (0..1).
#[derive(Clone, Debug, PartialEq)]
struct Point {
    name: String,
    x: f32,
    y: f32,
}

/// A parsed quadrant chart. Axis labels and quadrant labels are optional.
#[derive(Clone, Debug, PartialEq, Default)]
struct Quadrant {
    title: Option<String>,
    x_left: Option<String>,
    x_right: Option<String>,
    y_bottom: Option<String>,
    y_top: Option<String>,
    /// Quadrant labels in mermaid order: [q1 top-right, q2 top-left,
    /// q3 bottom-left, q4 bottom-right].
    quadrants: [Option<String>; 4],
    points: Vec<Point>,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse mermaid quadrant source into a [`Quadrant`]. Returns `Err(message)`
/// when the `quadrantChart` header is missing/malformed.
fn parse_quadrant(src: &str) -> Result<Quadrant, String> {
    let mut q = Quadrant::default();
    let mut saw_header = false;

    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if !saw_header {
            line.strip_prefix("quadrantChart")
                .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
                .ok_or_else(|| format!("expected 'quadrantChart' header, got: {line:?}"))?;
            saw_header = true;
            continue;
        }

        if let Some(t) = strip_keyword(line, "title") {
            if !t.is_empty() {
                q.title = Some(t.to_string());
            }
            continue;
        }
        if let Some(rest) = strip_keyword(line, "x-axis") {
            let (a, b) = split_arrow(rest);
            q.x_left = Some(a);
            q.x_right = b;
            continue;
        }
        if let Some(rest) = strip_keyword(line, "y-axis") {
            let (a, b) = split_arrow(rest);
            q.y_bottom = Some(a);
            q.y_top = b;
            continue;
        }
        let mut matched_quad = false;
        for (idx, kw) in ["quadrant-1", "quadrant-2", "quadrant-3", "quadrant-4"]
            .iter()
            .enumerate()
        {
            if let Some(label) = strip_keyword(line, kw) {
                q.quadrants[idx] = Some(label.to_string());
                matched_quad = true;
                break;
            }
        }
        if matched_quad {
            continue;
        }

        // Otherwise try a point line: `Name: [x, y]`.
        if let Some(p) = parse_point_line(line) {
            q.points.push(p);
        }
        // Unrecognized lines are skipped (forgiving).
    }

    if !saw_header {
        return Err("empty input / no 'quadrantChart' header".to_string());
    }
    Ok(q)
}

/// If `line` begins with `kw` followed by whitespace (or is exactly `kw`),
/// return the trimmed remainder; otherwise `None`.
fn strip_keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

/// Split an axis spec on `-->`. Returns (left/first, optional right/second).
fn split_arrow(s: &str) -> (String, Option<String>) {
    if let Some(pos) = s.find("-->") {
        let a = s[..pos].trim().to_string();
        let b = s[pos + 3..].trim();
        (a, if b.is_empty() { None } else { Some(b.to_string()) })
    } else {
        (s.trim().to_string(), None)
    }
}

/// Parse a point line `Name: [x, y]` with x,y parseable floats. Returns `None`
/// if the line is not a well-formed `name: [num, num]`.
fn parse_point_line(line: &str) -> Option<Point> {
    let colon = line.find(':')?;
    let name = line[..colon].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let rest = line[colon + 1..].trim();
    let inner = rest.strip_prefix('[')?.strip_suffix(']')?;
    let mut nums = inner.split(',');
    let x: f32 = nums.next()?.trim().parse().ok()?;
    let y: f32 = nums.next()?.trim().parse().ok()?;
    if nums.next().is_some() || !x.is_finite() || !y.is_finite() {
        return None;
    }
    Some(Point { name, x, y })
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

const MARGIN: f32 = 20.0;
/// Plot side length, px.
const PLOT_SIZE: f32 = 400.0;
/// Width of the left gutter reserved for the y-axis labels, px.
const AXIS_GUTTER: f32 = 28.0;
/// Point circle radius, px.
const POINT_R: f32 = 5.0;
/// Crossing-line / border stroke width, px.
const STROKE_W: f32 = 1.5;

/// Faint tints for the four quadrant cells, in mermaid order
/// [q1 TR, q2 TL, q3 BL, q4 BR].
const QUAD_TINTS: [[u8; 3]; 4] = [
    [0xE6, 0xF2, 0xE6], // top-right, faint green
    [0xF2, 0xF2, 0xE6], // top-left, faint yellow
    [0xF2, 0xE6, 0xE6], // bottom-left, faint red
    [0xE6, 0xE6, 0xF2], // bottom-right, faint blue
];

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render mermaid quadrant-chart source to an SVG document.
pub fn render_quadrant(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let q = parse_quadrant(src).map_err(MermaidError::Parse)?;

    // Need something to draw: at least a point, an axis, or a quadrant label.
    let has_content = !q.points.is_empty()
        || q.x_left.is_some()
        || q.y_bottom.is_some()
        || q.quadrants.iter().any(Option::is_some);
    if !has_content {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let title_fs = fs * 1.5;
    let size = PLOT_SIZE;

    let title_band = if q.title.is_some() { title_fs + MARGIN * 0.5 } else { 0.0 };
    // Plot origin (top-left corner of the square).
    let ox = MARGIN + AXIS_GUTTER;
    let oy = title_band + MARGIN;

    let bottom_axis_band = fs * 1.4; // room under the plot for x-axis labels
    let width = ox + size + MARGIN;
    let height = oy + size + bottom_axis_band + MARGIN;

    let mut svg = String::new();
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Plot→pixel mapping. x: 0→ox, 1→ox+size. y inverted: 0→bottom, 1→top.
    let px = |x: f32| ox + x.clamp(0.0, 1.0) * size;
    let py = |y: f32| oy + (1.0 - y.clamp(0.0, 1.0)) * size;
    let cx_center = ox + size / 2.0;
    let cy_center = oy + size / 2.0;

    // Quadrant cell tints. Cell pixel rects (each is half the side):
    //   q1 top-right, q2 top-left, q3 bottom-left, q4 bottom-right.
    let half = size / 2.0;
    let cells: [(f32, f32); 4] = [
        (cx_center, oy),        // q1 TR top-left corner
        (ox, oy),               // q2 TL
        (ox, cy_center),        // q3 BL
        (cx_center, cy_center), // q4 BR
    ];
    for (i, (rx, ry)) in cells.iter().enumerate() {
        let [r, g, b] = QUAD_TINTS[i];
        let _ = write!(
            svg,
            "<rect x=\"{rx:.2}\" y=\"{ry:.2}\" width=\"{half:.2}\" height=\"{half:.2}\" \
             fill=\"rgb({r},{g},{b})\" stroke=\"none\"/>",
        );
    }

    // Outer border.
    let _ = write!(
        svg,
        "<rect x=\"{ox:.2}\" y=\"{oy:.2}\" width=\"{size:.2}\" height=\"{size:.2}\" \
         fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{STROKE_W}\"/>",
        stroke = rgb(opts.edge_stroke),
    );

    // Two crossing lines through the center.
    let _ = write!(
        svg,
        "<line x1=\"{x1:.2}\" y1=\"{cy_center:.2}\" x2=\"{x2:.2}\" y2=\"{cy_center:.2}\" \
         stroke=\"{stroke}\" stroke-width=\"{STROKE_W}\"/>",
        x1 = ox,
        x2 = ox + size,
        stroke = rgb(opts.edge_stroke),
    );
    let _ = write!(
        svg,
        "<line x1=\"{cx_center:.2}\" y1=\"{y1:.2}\" x2=\"{cx_center:.2}\" y2=\"{y2:.2}\" \
         stroke=\"{stroke}\" stroke-width=\"{STROKE_W}\"/>",
        y1 = oy,
        y2 = oy + size,
        stroke = rgb(opts.edge_stroke),
    );

    // Quadrant labels centered in each cell.
    let cell_centers: [(f32, f32); 4] = [
        (ox + 3.0 * half / 2.0, oy + half / 2.0), // q1 TR
        (ox + half / 2.0, oy + half / 2.0),       // q2 TL
        (ox + half / 2.0, oy + 3.0 * half / 2.0), // q3 BL
        (ox + 3.0 * half / 2.0, oy + 3.0 * half / 2.0), // q4 BR
    ];
    for (i, label) in q.quadrants.iter().enumerate() {
        if let Some(label) = label {
            let (lcx, lcy) = cell_centers[i];
            emit_text_centered(&mut svg, label, lcx, lcy, fs, opts, true);
        }
    }

    // Axis labels.
    // x-axis: left label at bottom-left, right label at bottom-right.
    let x_label_y = oy + size + bottom_axis_band / 2.0;
    if let Some(l) = &q.x_left {
        emit_text(&mut svg, l, ox + 2.0, x_label_y, fs, opts, "start");
    }
    if let Some(r) = &q.x_right {
        emit_text(&mut svg, r, ox + size - 2.0, x_label_y, fs, opts, "end");
    }
    // y-axis: rotated -90°, bottom label at bottom, top label at top of the
    // left gutter. Anchored on the gutter centerline.
    let y_axis_x = MARGIN + AXIS_GUTTER / 2.0;
    if let Some(b) = &q.y_bottom {
        emit_text_rotated(&mut svg, b, y_axis_x, oy + size - 2.0, fs, opts, "start");
    }
    if let Some(t) = &q.y_top {
        emit_text_rotated(&mut svg, t, y_axis_x, oy + 2.0, fs, opts, "end");
    }

    // Points: a small circle at (x*size, (1-y)*size), name label to the right.
    for p in &q.points {
        let cx = px(p.x);
        let cy = py(p.y);
        let _ = write!(
            svg,
            "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{POINT_R}\" fill=\"{fill}\" \
             stroke=\"{stroke}\" stroke-width=\"1\"/>",
            fill = rgb(opts.node_fill),
            stroke = rgb(opts.node_stroke),
        );
        let lx = cx + POINT_R + 3.0;
        emit_text(&mut svg, &p.name, lx, cy, fs * 0.85, opts, "start");
    }

    // Title centered on top.
    if let Some(t) = &q.title {
        let tcx = w / 2.0;
        let ty = title_band / 2.0;
        let _ = write!(
            svg,
            "<text x=\"{tcx:.2}\" y=\"{ty:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{title_fs}\" font-weight=\"bold\" fill=\"{fill}\">{txt}</text>",
            family = escape(&opts.font_family),
            fill = rgb(opts.text_color),
            txt = escape(t),
        );
    }

    svg.push_str("</svg>");

    Ok(MermaidRender { svg, width_px: w, height_px: h })
}

/// A `<text>` anchored at (x, y) with the given anchor; vertically centered.
fn emit_text(svg: &mut String, text: &str, x: f32, y: f32, fs: f32, opts: &MermaidOptions, anchor: &str) {
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"{anchor}\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs:.2}\" fill=\"{fill}\">{txt}</text>",
        family = escape(&opts.font_family),
        fill = rgb(opts.text_color),
        txt = escape(text),
    );
}

/// A centered `<text>`; `bold` toggles bold weight (for quadrant labels).
fn emit_text_centered(svg: &mut String, text: &str, x: f32, y: f32, fs: f32, opts: &MermaidOptions, bold: bool) {
    let weight = if bold { " font-weight=\"bold\"" } else { "" };
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs:.2}\"{weight} fill=\"{fill}\">{txt}</text>",
        family = escape(&opts.font_family),
        fill = rgb(opts.text_color),
        txt = escape(text),
    );
}

/// A `<text>` rotated -90° about (x, y), used for the vertical y-axis labels.
fn emit_text_rotated(svg: &mut String, text: &str, x: f32, y: f32, fs: f32, opts: &MermaidOptions, anchor: &str) {
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" transform=\"rotate(-90 {x:.2} {y:.2})\" \
         text-anchor=\"{anchor}\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs:.2}\" fill=\"{fill}\">{txt}</text>",
        family = escape(&opts.font_family),
        fill = rgb(opts.text_color),
        txt = escape(text),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"quadrantChart
    title Reach and engagement of campaigns
    x-axis Low Reach --> High Reach
    y-axis Low Engagement --> High Engagement
    quadrant-1 We should expand
    quadrant-2 Need to promote
    quadrant-3 Re-evaluate
    quadrant-4 May be improved
    Campaign A: [0.3, 0.6]
    Campaign B: [0.45, 0.23]
    Campaign C: [0.57, 0.69]
"#;

    #[test]
    fn parses_title_axes_quadrants_points() {
        let q = parse_quadrant(SAMPLE).expect("parse");
        assert_eq!(q.title.as_deref(), Some("Reach and engagement of campaigns"));
        assert_eq!(q.x_left.as_deref(), Some("Low Reach"));
        assert_eq!(q.x_right.as_deref(), Some("High Reach"));
        assert_eq!(q.y_bottom.as_deref(), Some("Low Engagement"));
        assert_eq!(q.y_top.as_deref(), Some("High Engagement"));
        assert_eq!(q.quadrants[0].as_deref(), Some("We should expand"));
        assert_eq!(q.quadrants[3].as_deref(), Some("May be improved"));
        assert_eq!(q.points.len(), 3);
        assert_eq!(q.points[0].name, "Campaign A");
        assert_eq!(q.points[0].x, 0.3);
        assert_eq!(q.points[0].y, 0.6);
    }

    #[test]
    fn x_axis_without_arrow_ok() {
        let q = parse_quadrant("quadrantChart\nx-axis Just Left\n").expect("parse");
        assert_eq!(q.x_left.as_deref(), Some("Just Left"));
        assert_eq!(q.x_right, None);
    }

    #[test]
    fn point_values_in_unit_range() {
        let q = parse_quadrant(SAMPLE).expect("parse");
        for p in &q.points {
            assert!((0.0..=1.0).contains(&p.x), "x out of range: {}", p.x);
            assert!((0.0..=1.0).contains(&p.y), "y out of range: {}", p.y);
        }
    }

    #[test]
    fn ignores_comments_and_blanks() {
        let src = "quadrantChart\n%% c\n\n  P: [0.5, 0.5]  %% inline\n";
        let q = parse_quadrant(src).expect("parse");
        assert_eq!(q.points.len(), 1);
    }

    #[test]
    fn malformed_point_skipped() {
        let q = parse_quadrant("quadrantChart\nGarbage line\nGood: [0.1, 0.2]\n").expect("parse");
        assert_eq!(q.points.len(), 1);
        assert_eq!(q.points[0].name, "Good");
    }

    #[test]
    fn render_well_formed_svg() {
        let r = render_quadrant(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_has_two_crossing_lines() {
        let r = render_quadrant(SAMPLE, &MermaidOptions::default()).expect("render");
        assert_eq!(r.svg.matches("<line").count(), 2, "expected exactly 2 crossing lines");
    }

    #[test]
    fn render_has_four_quadrant_labels() {
        let r = render_quadrant(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("We should expand"));
        assert!(r.svg.contains("Need to promote"));
        assert!(r.svg.contains("Re-evaluate"));
        assert!(r.svg.contains("May be improved"));
    }

    #[test]
    fn render_has_axis_labels() {
        let r = render_quadrant(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("Low Reach"));
        assert!(r.svg.contains("High Reach"));
        assert!(r.svg.contains("Low Engagement"));
        assert!(r.svg.contains("High Engagement"));
    }

    #[test]
    fn render_one_circle_per_point() {
        let r = render_quadrant(SAMPLE, &MermaidOptions::default()).expect("render");
        assert_eq!(r.svg.matches("<circle").count(), 3, "one circle per point");
        assert!(r.svg.contains("Campaign A"));
    }

    #[test]
    fn point_y_is_inverted() {
        // A point at y=1 (top) must render with a smaller cy than y=0 (bottom).
        let src = "quadrantChart\nTop: [0.5, 1.0]\nBot: [0.5, 0.0]\n";
        let r = render_quadrant(src, &MermaidOptions::default()).expect("render");
        let cys: Vec<f32> = r
            .svg
            .match_indices("<circle")
            .filter_map(|(idx, _)| {
                let seg = &r.svg[idx..];
                let cyi = seg.find("cy=\"")? + 4;
                let end = seg[cyi..].find('"')? + cyi;
                seg[cyi..end].parse::<f32>().ok()
            })
            .collect();
        assert_eq!(cys.len(), 2);
        // Top point (first) should have smaller cy (higher on screen).
        assert!(cys[0] < cys[1], "y=1 should be above y=0: {cys:?}");
    }

    #[test]
    fn point_at_correct_pixel() {
        // A point at (1, 0) maps to (ox+size, oy+size) — bottom-right corner.
        let src = "quadrantChart\nBR: [1.0, 0.0]\n";
        let r = render_quadrant(src, &MermaidOptions::default()).expect("render");
        let ox = MARGIN + AXIS_GUTTER;
        let oy = MARGIN; // no title band here
        let want_cx = ox + PLOT_SIZE;
        let want_cy = oy + PLOT_SIZE;
        let idx = r.svg.find("<circle").unwrap();
        let seg = &r.svg[idx..];
        let cxi = seg.find("cx=\"").unwrap() + 4;
        let cxe = seg[cxi..].find('"').unwrap() + cxi;
        let cx: f32 = seg[cxi..cxe].parse().unwrap();
        let cyi = seg.find("cy=\"").unwrap() + 4;
        let cye = seg[cyi..].find('"').unwrap() + cyi;
        let cy: f32 = seg[cyi..cye].parse().unwrap();
        assert!((cx - want_cx).abs() < 0.5, "cx {cx} != {want_cx}");
        assert!((cy - want_cy).abs() < 0.5, "cy {cy} != {want_cy}");
    }

    #[test]
    fn xml_escapes_text() {
        let src = "quadrantChart\ntitle A & B\nquadrant-1 <q1>\nP & Q: [0.5, 0.5]\n";
        let r = render_quadrant(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("A &amp; B"));
        assert!(r.svg.contains("&lt;q1&gt;"));
        assert!(r.svg.contains("P &amp; Q"));
        assert!(!r.svg.contains("A & B"));
    }

    #[test]
    fn empty_input_errors() {
        match render_quadrant("", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn header_only_is_empty() {
        let r = render_quadrant("quadrantChart\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Empty)));
    }

    #[test]
    fn missing_header_errors() {
        let r = render_quadrant("graph TD\nA-->B\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let a = render_quadrant(SAMPLE, &opts).expect("a");
        let b = render_quadrant(SAMPLE, &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
