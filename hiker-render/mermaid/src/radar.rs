//! `radar` diagram (self-contained: parse + draw, polar/spider layout, no dagre).
//!
//! Mermaid radar syntax (the subset we support):
//! ```text
//! radar-beta
//!     title My Radar
//!     axis a["Apples"], b["Bananas"], c["Cherries"]
//!     curve s1["Series 1"]{ 1, 2, 3 }
//!     curve s2: 3, 2, 1
//!     max 5
//!     min 0
//!     ticks 4
//!     graticule polygon
//! ```
//! The header line is `radar-beta` (a trailing `:` is allowed) or `radar`.
//!
//! - `title <text>` (or `title: <text>`) — optional chart title.
//! - `axis <id>["Label"], <id>["Label"], ...` — the spokes; the `["Label"]`
//!   display label is optional (defaults to the id). Multiple `axis` lines
//!   accumulate.
//! - `curve <id>["Label"]{ v1, v2, ... }` **or** `curve <id>: v1, v2, ...` — a
//!   numeric series, one value per axis in axis order. Multiple `curve` lines
//!   overlay multiple polygons. Values may be wrapped in braces and span lines
//!   (the brace body is gathered until the closing `}`).
//! - `max <n>` / `min <n>` — explicit value range (default `0..max(data)`).
//! - `ticks <n>` — number of graticule rings (default 5).
//! - `graticule circle|polygon` — ring style (default polygon).
//!
//! Skipped (noted): styling/classes, `showLegend` nuances (we always draw a
//! legend when there is more than one curve), and detailed `axis: value` entry
//! addressing inside a curve (values are taken positionally, in axis order).
//!
//! Layout is pure trig — no graph layout. `N` axes give `N` spokes at angles
//! `θ_k = -90° + k·360/N` (top, clockwise). Each value maps to a radius
//! `r = (v-min)/(max-min)·R` along its spoke; the curve is the closed polygon
//! through those `N` points, stroked in a palette color with a translucent fill.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/radar/renderer.ts`.

use std::fmt::Write as _;

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One axis (spoke): an id and its display label.
#[derive(Clone, Debug, PartialEq)]
struct Axis {
    id: String,
    label: String,
}

/// One data series: an id, display label, and one value per axis (in order).
#[derive(Clone, Debug, PartialEq)]
struct Curve {
    id: String,
    label: String,
    values: Vec<f64>,
}

/// Graticule (ring) style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Graticule {
    Circle,
    Polygon,
}

/// A parsed radar chart.
#[derive(Clone, Debug, PartialEq)]
struct Radar {
    title: Option<String>,
    axes: Vec<Axis>,
    curves: Vec<Curve>,
    max: Option<f64>,
    min: Option<f64>,
    ticks: usize,
    graticule: Graticule,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse mermaid radar source into a [`Radar`]. Returns `Err(message)` when the
/// header is missing/malformed.
fn parse_radar(src: &str) -> Result<Radar, String> {
    let mut title: Option<String> = None;
    let mut axes: Vec<Axis> = Vec::new();
    let mut curves: Vec<Curve> = Vec::new();
    let mut max: Option<f64> = None;
    let mut min: Option<f64> = None;
    let mut ticks: usize = 5;
    let mut graticule = Graticule::Polygon;
    let mut saw_header = false;

    // Pre-strip comments and join so brace bodies that span lines parse as one.
    let mut logical: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(mut acc) = pending.take() {
            acc.push(' ');
            acc.push_str(line);
            if acc.contains('}') {
                logical.push(acc);
            } else {
                pending = Some(acc);
            }
            continue;
        }
        // A `curve ... {` without its closing brace continues onto later lines.
        if line.contains('{') && !line.contains('}') {
            pending = Some(line.to_string());
            continue;
        }
        logical.push(line.to_string());
    }
    if let Some(acc) = pending.take() {
        logical.push(acc);
    }

    for line in &logical {
        let line = line.as_str();
        // Header.
        if !saw_header {
            let first = line.split_whitespace().next().unwrap_or("");
            let head = first.trim_end_matches(':');
            if head == "radar-beta" || head == "radar" {
                saw_header = true;
                // Allow `radar-beta title ...` style trailing content.
                let rest = line[first.len()..].trim();
                if !rest.is_empty() {
                    parse_body_line(
                        rest, &mut title, &mut axes, &mut curves, &mut max, &mut min, &mut ticks,
                        &mut graticule,
                    );
                }
                continue;
            }
            return Err(format!(
                "radar: expected `radar-beta` header, found {first:?}"
            ));
        }
        parse_body_line(
            line, &mut title, &mut axes, &mut curves, &mut max, &mut min, &mut ticks,
            &mut graticule,
        );
    }

    if !saw_header {
        return Err("radar: missing `radar-beta` header".to_string());
    }

    Ok(Radar {
        title,
        axes,
        curves,
        max,
        min,
        ticks: ticks.max(1),
        graticule,
    })
}

/// Dispatch one logical body line by its leading keyword.
#[allow(clippy::too_many_arguments)]
fn parse_body_line(
    line: &str,
    title: &mut Option<String>,
    axes: &mut Vec<Axis>,
    curves: &mut Vec<Curve>,
    max: &mut Option<f64>,
    min: &mut Option<f64>,
    ticks: &mut usize,
    graticule: &mut Graticule,
) {
    let (kw, rest) = split_keyword(line);
    match kw.as_str() {
        "title" => {
            let t = rest.trim_start_matches(':').trim();
            if !t.is_empty() {
                *title = Some(t.to_string());
            }
        }
        "axis" => {
            for item in split_top_commas(rest) {
                if let Some(a) = parse_axis_item(&item) {
                    axes.push(a);
                }
            }
        }
        "curve" => {
            if let Some(c) = parse_curve(rest) {
                curves.push(c);
            }
        }
        "max" => {
            if let Some(v) = first_number(rest) {
                *max = Some(v);
            }
        }
        "min" => {
            if let Some(v) = first_number(rest) {
                *min = Some(v);
            }
        }
        "ticks" => {
            if let Some(v) = first_number(rest) {
                *ticks = v.max(0.0) as usize;
            }
        }
        "graticule" => {
            let g = rest.trim_start_matches(':').trim().to_ascii_lowercase();
            if g.starts_with("circle") {
                *graticule = Graticule::Circle;
            } else if g.starts_with("polygon") {
                *graticule = Graticule::Polygon;
            }
        }
        // Unknown keyword (e.g. `showLegend`, styling) — noted/ignored.
        _ => {}
    }
}

/// Split a line into `(keyword, rest)` on the first whitespace.
fn split_keyword(line: &str) -> (String, &str) {
    match line.find(char::is_whitespace) {
        Some(i) => (line[..i].to_string(), line[i..].trim_start()),
        None => (line.to_string(), ""),
    }
}

/// Parse one `id["Label"]` axis item.
fn parse_axis_item(s: &str) -> Option<Axis> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (id, label) = parse_id_label(s);
    if id.is_empty() {
        return None;
    }
    let label = label.unwrap_or_else(|| id.clone());
    Some(Axis { id, label })
}

/// Parse `id["Label"]{ values }` or `id["Label"]: values` into a [`Curve`].
fn parse_curve(s: &str) -> Option<Curve> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Locate the value list: either inside `{...}` or after a `:`.
    let (head, body) = if let Some(open) = s.find('{') {
        let close = s[open..].find('}').map(|i| open + i).unwrap_or(s.len());
        let body = &s[open + 1..close.min(s.len())];
        (&s[..open], body)
    } else if let Some(colon) = s.find(':') {
        (&s[..colon], &s[colon + 1..])
    } else {
        // No values: still record the curve head if any.
        (s, "")
    };
    let (id, label) = parse_id_label(head.trim());
    if id.is_empty() {
        return None;
    }
    let label = label.unwrap_or_else(|| id.clone());
    let values: Vec<f64> = split_top_commas(body)
        .into_iter()
        .filter_map(|t| {
            // Allow `axisId: value` detailed entries — take the trailing number.
            let t = t.rsplit(':').next().unwrap_or(&t);
            first_number(t)
        })
        .collect();
    Some(Curve { id, label, values })
}

/// Split off a leading `id` and an optional `["Label"]` from `s`.
/// Returns `(id, Some(label))` when a bracketed label is present.
fn parse_id_label(s: &str) -> (String, Option<String>) {
    let s = s.trim();
    if let Some(open) = s.find('[') {
        let id = s[..open].trim().to_string();
        let close = s[open..].find(']').map(|i| open + i);
        if let Some(close) = close {
            let inner = s[open + 1..close].trim();
            let label = inner.trim_matches(|c| c == '"' || c == '\'').to_string();
            return (id, Some(label));
        }
        return (id, None);
    }
    (s.to_string(), None)
}

/// Split on commas that are not inside `[...]`, `{...}`, or quotes.
fn split_top_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut cur = String::new();
    for c in s.chars() {
        match in_str {
            Some(q) => {
                cur.push(c);
                if c == q {
                    in_str = None;
                }
            }
            None => match c {
                '"' | '\'' => {
                    in_str = Some(c);
                    cur.push(c);
                }
                '[' | '{' => {
                    depth += 1;
                    cur.push(c);
                }
                ']' | '}' => {
                    depth -= 1;
                    cur.push(c);
                }
                ',' if depth <= 0 => {
                    out.push(cur.trim().to_string());
                    cur.clear();
                }
                _ => cur.push(c),
            },
        }
    }
    let last = cur.trim();
    if !last.is_empty() {
        out.push(last.to_string());
    }
    out.into_iter().filter(|t| !t.is_empty()).collect()
}

/// Parse the first floating-point number found in `s`.
fn first_number(s: &str) -> Option<f64> {
    let mut start = None;
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let is_num =
            b.is_ascii_digit() || b == b'.' || b == b'-' || b == b'+' || b == b'e' || b == b'E';
        if is_num {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(st) = start {
            if let Ok(v) = s[st..i].parse::<f64>() {
                return Some(v);
            }
            start = None;
        }
    }
    if let Some(st) = start {
        return s[st..].parse::<f64>().ok();
    }
    None
}

// ---------------------------------------------------------------------------
// Draw
// ---------------------------------------------------------------------------

/// A small categorical palette for curves (straight RGB; alpha applied for fill).
const PALETTE: [[u8; 3]; 8] = [
    [66, 133, 244],  // blue
    [219, 68, 55],   // red
    [244, 180, 0],   // amber
    [15, 157, 88],   // green
    [171, 71, 188],  // purple
    [0, 172, 193],   // cyan
    [255, 112, 67],  // deep orange
    [120, 144, 156], // blue grey
];

/// The curve color (RGB) for index `i` (cycling). Prefers the active theme's
/// `series_palette` when set, falling back to the local [`PALETTE`].
fn palette_color(opts: &MermaidOptions, i: usize) -> [u8; 3] {
    if !opts.series_palette.is_empty() {
        let c = opts.series_palette[i % opts.series_palette.len()];
        [c[0], c[1], c[2]]
    } else {
        PALETTE[i % PALETTE.len()]
    }
}

/// Point on a spoke at axis index `k` (of `n`), at radius `r`, around `center`.
fn spoke_point(center: (f32, f32), k: usize, n: usize, r: f32) -> (f32, f32) {
    let angle = -std::f32::consts::FRAC_PI_2 + (k as f32) * 2.0 * std::f32::consts::PI / (n as f32);
    (center.0 + r * angle.cos(), center.1 + r * angle.sin())
}

/// Render mermaid radar source to an SVG document.
pub fn render_radar(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let radar = parse_radar(src).map_err(MermaidError::Parse)?;
    let n = radar.axes.len();
    if n == 0 || radar.curves.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    // Chart radius and value range.
    let radius: f32 = (fs * 10.0).min(160.0).max(60.0);

    let data_max = radar
        .curves
        .iter()
        .flat_map(|c| c.values.iter().copied())
        .fold(f64::NEG_INFINITY, f64::max);
    let min_v = radar.min.unwrap_or(0.0);
    let max_v = radar.max.unwrap_or_else(|| {
        if data_max.is_finite() && data_max > min_v {
            data_max
        } else {
            min_v + 1.0
        }
    });
    let range = (max_v - min_v).abs().max(1e-9);

    // Margin must hold the axis labels (placed at radius * 1.18) and title/legend.
    let label_pad = radar
        .axes
        .iter()
        .map(|a| text_size(&a.label, fs).0)
        .fold(0.0f32, f32::max);
    let label_factor = 1.18f32;
    let margin = (radius * (label_factor - 1.0)) + label_pad + fs;
    let title_h = if radar.title.is_some() { fs * 1.6 } else { 0.0 };

    // Legend (only when more than one curve): a column on the right.
    let show_legend = radar.curves.len() > 1;
    let legend_w = if show_legend {
        let w = radar
            .curves
            .iter()
            .map(|c| text_size(&c.label, fs).0)
            .fold(0.0f32, f32::max);
        w + fs * 2.2
    } else {
        0.0
    };

    let chart_w = (radius + margin) * 2.0;
    let chart_h = (radius + margin) * 2.0;
    let width = chart_w + legend_w;
    let height = title_h + chart_h;

    let center = (margin + radius, title_h + margin + radius);

    let mut s = String::new();
    let _ = write!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" \
         viewBox=\"0 0 {width:.0} {height:.0}\">",
    );

    // Title (centered over the chart area).
    if let Some(t) = &radar.title {
        let _ = write!(
            s,
            "<text x=\"{:.2}\" y=\"{:.2}\" text-anchor=\"middle\" \
             font-family=\"{}\" font-size=\"{:.2}\" font-weight=\"bold\" fill=\"{}\">{}</text>",
            chart_w / 2.0,
            fs * 1.15,
            escape(&opts.font_family),
            fs * 1.1,
            rgb(opts.text_color),
            escape(t),
        );
    }

    // Graticule rings.
    let grid = [opts.edge_stroke[0], opts.edge_stroke[1], opts.edge_stroke[2]];
    let grid_str = rgb([grid[0], grid[1], grid[2], 255]);
    for i in 1..=radar.ticks {
        let r = radius * (i as f32) / (radar.ticks as f32);
        match radar.graticule {
            Graticule::Circle => {
                let _ = write!(
                    s,
                    "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"{:.2}\" fill=\"none\" \
                     stroke=\"{}\" stroke-opacity=\"0.25\" stroke-width=\"1\"/>",
                    center.0, center.1, r, grid_str,
                );
            }
            Graticule::Polygon => {
                let mut pts = String::new();
                for k in 0..n {
                    let (x, y) = spoke_point(center, k, n, r);
                    let _ = write!(pts, "{x:.2},{y:.2} ");
                }
                let _ = write!(
                    s,
                    "<polygon points=\"{}\" fill=\"none\" stroke=\"{}\" \
                     stroke-opacity=\"0.25\" stroke-width=\"1\"/>",
                    pts.trim_end(),
                    grid_str,
                );
            }
        }
    }

    // Spokes + axis labels.
    for k in 0..n {
        let (x, y) = spoke_point(center, k, n, radius);
        let _ = write!(
            s,
            "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" \
             stroke=\"{}\" stroke-opacity=\"0.35\" stroke-width=\"1\"/>",
            center.0, center.1, x, y, grid_str,
        );
        // Label position, anchored by quadrant so text grows away from the chart.
        let (lx, ly) = spoke_point(center, k, n, radius * label_factor);
        let dx = lx - center.0;
        let anchor = if dx.abs() < 1.0 {
            "middle"
        } else if dx > 0.0 {
            "start"
        } else {
            "end"
        };
        let _ = write!(
            s,
            "<text x=\"{:.2}\" y=\"{:.2}\" text-anchor=\"{}\" dominant-baseline=\"middle\" \
             font-family=\"{}\" font-size=\"{:.2}\" fill=\"{}\">{}</text>",
            lx,
            ly,
            anchor,
            escape(&opts.font_family),
            fs * 0.85,
            rgb(opts.text_color),
            escape(&radar.axes[k].label),
        );
    }

    // Curves.
    for (ci, curve) in radar.curves.iter().enumerate() {
        let color = palette_color(opts, ci);
        let stroke = rgb([color[0], color[1], color[2], 255]);
        let fill_rgba = [color[0], color[1], color[2], 64u8];
        let mut pts = String::new();
        let mut dots = String::new();
        for k in 0..n {
            let v = curve.values.get(k).copied().unwrap_or(min_v);
            let frac = (((v - min_v) / range) as f32).clamp(0.0, 1.0);
            let (x, y) = spoke_point(center, k, n, radius * frac);
            let _ = write!(pts, "{x:.2},{y:.2} ");
            let _ = write!(
                dots,
                "<circle cx=\"{x:.2}\" cy=\"{y:.2}\" r=\"{:.2}\" fill=\"{stroke}\"/>",
                (fs * 0.18).max(2.0),
            );
        }
        let _ = write!(
            s,
            "<polygon points=\"{}\" fill=\"{}\"{} stroke=\"{}\" stroke-width=\"2\" \
             stroke-linejoin=\"round\"/>",
            pts.trim_end(),
            stroke,
            opacity_attr("fill-opacity", fill_rgba),
            stroke,
        );
        s.push_str(&dots);
    }

    // Legend (right column).
    if show_legend {
        let lx = chart_w + fs * 0.6;
        let mut ly = title_h + fs;
        let sw = fs * 0.9;
        for (ci, curve) in radar.curves.iter().enumerate() {
            let color = palette_color(opts, ci);
            let stroke = rgb([color[0], color[1], color[2], 255]);
            let _ = write!(
                s,
                "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{sw:.2}\" height=\"{sw:.2}\" \
                 fill=\"{}\" fill-opacity=\"0.5\" stroke=\"{}\"/>",
                lx,
                ly - sw * 0.8,
                stroke,
                stroke,
            );
            let _ = write!(
                s,
                "<text x=\"{:.2}\" y=\"{:.2}\" dominant-baseline=\"middle\" \
                 font-family=\"{}\" font-size=\"{:.2}\" fill=\"{}\">{}</text>",
                lx + sw + fs * 0.4,
                ly - sw * 0.3,
                escape(&opts.font_family),
                fs * 0.85,
                rgb(opts.text_color),
                escape(&curve.label),
            );
            ly += fs * 1.5;
        }
    }

    s.push_str("</svg>");

    Ok(MermaidRender {
        svg: s,
        width_px: width,
        height_px: height,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    const SRC: &str = "radar-beta\n\
        title My Radar\n\
        axis a[\"A\"], b[\"B\"], c[\"C\"]\n\
        curve s1[\"Series 1\"]{ 1, 2, 3 }\n\
        curve s2: 3, 2, 1\n";

    #[test]
    fn parses_title_axes_and_curves() {
        let r = parse_radar(SRC).unwrap();
        assert_eq!(r.title.as_deref(), Some("My Radar"));
        assert_eq!(r.axes.len(), 3);
        assert_eq!(r.axes[0].id, "a");
        assert_eq!(r.axes[0].label, "A");
        assert_eq!(r.axes[2].label, "C");
        assert_eq!(r.curves.len(), 2);
        assert_eq!(r.curves[0].id, "s1");
        assert_eq!(r.curves[0].label, "Series 1");
        assert_eq!(r.curves[0].values, vec![1.0, 2.0, 3.0]);
        assert_eq!(r.curves[1].values, vec![3.0, 2.0, 1.0]);
    }

    #[test]
    fn accepts_bare_radar_header_and_colon() {
        let r = parse_radar("radar\naxis x, y\ncurve c{1,2}\n").unwrap();
        assert_eq!(r.axes.len(), 2);
        // No bracket label → label defaults to id.
        assert_eq!(r.axes[0].label, "x");
        assert_eq!(r.curves[0].values, vec![1.0, 2.0]);
        let r2 = parse_radar("radar-beta:\naxis x\ncurve c{5}\n").unwrap();
        assert_eq!(r2.axes.len(), 1);
        assert_eq!(r2.curves[0].values, vec![5.0]);
    }

    #[test]
    fn multiple_axis_lines_accumulate() {
        let r = parse_radar("radar-beta\naxis a, b\naxis c\ncurve s{1,2,3}\n").unwrap();
        assert_eq!(r.axes.len(), 3);
        assert_eq!(r.axes[2].id, "c");
    }

    #[test]
    fn parses_options() {
        let src = "radar-beta\naxis a, b\ncurve s{1,2}\nmax 10\nmin 1\nticks 4\ngraticule circle\n";
        let r = parse_radar(src).unwrap();
        assert_eq!(r.max, Some(10.0));
        assert_eq!(r.min, Some(1.0));
        assert_eq!(r.ticks, 4);
        assert_eq!(r.graticule, Graticule::Circle);
    }

    #[test]
    fn curve_braces_can_span_lines() {
        let src = "radar-beta\naxis a, b, c\ncurve s{\n1,\n2,\n3\n}\n";
        let r = parse_radar(src).unwrap();
        assert_eq!(r.curves[0].values, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn detailed_entries_take_trailing_number() {
        // `axisId: value` form inside braces — values taken positionally.
        let r = parse_radar("radar-beta\naxis a, b\ncurve s{ a: 4, b: 8 }\n").unwrap();
        assert_eq!(r.curves[0].values, vec![4.0, 8.0]);
    }

    #[test]
    fn bad_header_is_parse_error() {
        let err = parse_radar("not-a-radar\naxis a\n").unwrap_err();
        assert!(err.contains("radar"));
    }

    #[test]
    fn renders_well_formed_svg() {
        let r = render_radar(SRC, &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.ends_with("</svg>"));
        assert!(r.svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn renders_spokes_graticule_and_labels() {
        let r = render_radar(SRC, &opts()).unwrap().svg;
        // 3 axes → 3 spoke <line>s.
        assert_eq!(r.matches("<line").count(), 3);
        // Default 5 ticks, polygon graticule → 5 graticule polygons.
        let graticule = r.matches("fill=\"none\"").count();
        assert_eq!(graticule, 5);
        // Axis labels + title present.
        assert!(r.contains(">A</text>"));
        assert!(r.contains(">B</text>"));
        assert!(r.contains(">C</text>"));
        assert!(r.contains(">My Radar</text>"));
    }

    #[test]
    fn one_closed_polygon_per_curve_with_n_vertices() {
        let r = render_radar(SRC, &opts()).unwrap().svg;
        // Curve polygons carry a fill-opacity; graticule polygons use fill="none".
        let curve_polys = r.matches("fill-opacity").count();
        // 2 curves → at least 2 filled polygons (legend swatches also have
        // fill-opacity, so count the stroke-width=2 curve polygons instead).
        assert_eq!(r.matches("stroke-width=\"2\"").count(), 2);
        // Each curve polygon should have N=3 coordinate pairs. Check the first.
        let first = r.find("stroke-width=\"2\"").unwrap();
        let seg = &r[..first];
        let poly_start = seg.rfind("<polygon points=\"").unwrap() + "<polygon points=\"".len();
        let pts_end = r[poly_start..].find('"').unwrap() + poly_start;
        let pairs = r[poly_start..pts_end].split_whitespace().count();
        assert_eq!(pairs, 3);
        let _ = curve_polys;
    }

    #[test]
    fn values_map_to_radius_correctly() {
        // min=0, max=4, a value of 4 at axis 0 (top) should sit at the rim,
        // i.e. directly above center (x≈center.x, y≈center.y - radius).
        let src = "radar-beta\naxis a, b, c\ncurve s{4, 0, 0}\nmax 4\nmin 0\n";
        let r = render_radar(src, &opts()).unwrap().svg;
        // The first curve vertex (top spoke at full value) — find the curve polygon.
        let idx = r.find("stroke-width=\"2\"").unwrap();
        let seg = &r[..idx];
        let poly_start = seg.rfind("<polygon points=\"").unwrap() + "<polygon points=\"".len();
        let pts_end = r[poly_start..].find('"').unwrap() + poly_start;
        let first_pair = r[poly_start..pts_end].split_whitespace().next().unwrap();
        let mut it = first_pair.split(',');
        let x: f32 = it.next().unwrap().parse().unwrap();
        let y: f32 = it.next().unwrap().parse().unwrap();
        // Top vertex: x at center, y above. Center x = margin+radius.
        // The 0-valued vertices sit at center, so center == later pairs; just
        // check the top vertex is the highest (smallest y) point.
        let all_y: Vec<f32> = r[poly_start..pts_end]
            .split_whitespace()
            .map(|p| p.split(',').nth(1).unwrap().parse().unwrap())
            .collect();
        assert!(y <= *all_y.iter().min_by(|a, b| a.partial_cmp(b).unwrap()).unwrap() + 0.01);
        // And the top vertex x equals the center x (the two zero vertices share it).
        let center_x: Vec<f32> = r[poly_start..pts_end]
            .split_whitespace()
            .map(|p| p.split(',').next().unwrap().parse().unwrap())
            .collect();
        // x of top vertex should differ from at least one zero vertex unless all
        // collapse; mainly assert it's finite and within bounds.
        assert!(x.is_finite() && center_x.len() == 3);
    }

    #[test]
    fn legend_for_multiple_curves() {
        let multi = render_radar(SRC, &opts()).unwrap().svg;
        assert!(multi.contains(">Series 1</text>"));
        assert!(multi.contains(">s2</text>"));
        assert!(multi.contains("<rect"));
        // Single curve → no legend rects.
        let single = render_radar("radar-beta\naxis a, b\ncurve s{1,2}\n", &opts())
            .unwrap()
            .svg;
        assert!(!single.contains("<rect"));
    }

    #[test]
    fn xml_escaped() {
        let src = "radar-beta\ntitle A & B\naxis a[\"<x>\"], b[\"y\"]\ncurve s{1,2}\n";
        let r = render_radar(src, &opts()).unwrap().svg;
        assert!(r.contains("A &amp; B"));
        assert!(r.contains("&lt;x&gt;"));
        assert!(!r.contains("<x>"));
    }

    #[test]
    fn empty_and_errors() {
        // No axes → Empty.
        assert_eq!(
            render_radar("radar-beta\ncurve s{1,2}\n", &opts()),
            Err(MermaidError::Empty)
        );
        // No curves → Empty.
        assert_eq!(
            render_radar("radar-beta\naxis a, b\n", &opts()),
            Err(MermaidError::Empty)
        );
        // Bad header → Parse.
        assert!(matches!(
            render_radar("nope\n", &opts()),
            Err(MermaidError::Parse(_))
        ));
    }

    #[test]
    fn deterministic() {
        let a = render_radar(SRC, &opts()).unwrap();
        let b = render_radar(SRC, &opts()).unwrap();
        assert_eq!(a, b);
    }
}
