//! `xychart` (a.k.a. `xychart-beta`) diagram — self-contained: parse + draw, no
//! dagre layout.
//!
//! Mermaid xychart syntax (the subset we support):
//! ```text
//! xychart-beta
//!     title "Sales Revenue"
//!     x-axis [jan, feb, mar, apr]
//!     y-axis "Revenue (k$)" 0 --> 100
//!     bar [50, 60, 75, 40]
//!     line [40, 55, 70, 35]
//! ```
//! Header `xychart-beta` (or `xychart`), with an optional `horizontal`
//! orientation token (default `vertical`). We parse the orientation flag but
//! render only the vertical layout (the common case); the flag is recorded for
//! completeness.
//!
//! Axes:
//! - `x-axis` — **categorical** `x-axis [a, b, c]` (optionally with a leading
//!   quoted title: `x-axis "Label" [a, b, c]`), or **numeric**
//!   `x-axis <min> --> <max>` (optionally titled). The categorical form is the
//!   one that pairs with bars and is fully supported; the numeric form is parsed
//!   into evenly-spaced synthetic categories so a chart still renders.
//! - `y-axis "<title>" <min> --> <max>` — title and/or range optional. With no
//!   range the y scale auto-fits from the data (`[min(0, data_min), data_max]`
//!   plus a little headroom).
//!
//! Series:
//! - `bar [v1, v2, ...]` and `line [v1, v2, ...]`, optionally preceded by a
//!   (quoted) series title: `bar "Revenue" [..]`. Multiple `bar`/`line` lines
//!   overlay; values align to x categories by index.
//!
//! Skipped (vs. upstream): multiple named axes, per-series colors via config,
//! true numeric-x scatter plots, and horizontal-orientation rendering.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/xychart/`.

use std::fmt::Write as _;

use crate::svgutil::{escape, rgb};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Chart orientation. We parse both but only render `Vertical`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Orientation {
    Vertical,
    Horizontal,
}

/// A bar or line data series.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SeriesKind {
    Bar,
    Line,
}

/// One plotted series: its kind, optional title, and per-category values.
#[derive(Clone, Debug, PartialEq)]
struct Series {
    kind: SeriesKind,
    #[allow(dead_code)]
    title: String,
    values: Vec<f64>,
}

/// A parsed xychart.
#[derive(Clone, Debug, PartialEq)]
struct XyChart {
    orientation: Orientation,
    title: Option<String>,
    /// X category labels (synthesized for the numeric-x form).
    categories: Vec<String>,
    /// Explicit y-axis title, if given.
    y_title: Option<String>,
    /// Explicit y-axis `[min, max]`, if given (else auto-fit).
    y_range: Option<(f64, f64)>,
    series: Vec<Series>,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse mermaid xychart source into an [`XyChart`]. Returns `Err(message)` on a
/// missing/bad header or a malformed axis/series line.
fn parse_xychart(src: &str) -> Result<XyChart, String> {
    let mut saw_header = false;
    let mut orientation = Orientation::Vertical;
    let mut title: Option<String> = None;
    let mut categories: Vec<String> = Vec::new();
    let mut y_title: Option<String> = None;
    let mut y_range: Option<(f64, f64)> = None;
    let mut series: Vec<Series> = Vec::new();

    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if !saw_header {
            // Header: `xychart-beta` / `xychart`, optionally `... horizontal`.
            let rest = strip_keyword(line, "xychart-beta")
                .or_else(|| strip_keyword(line, "xychart"))
                .ok_or_else(|| format!("expected 'xychart-beta' header, got: {line:?}"))?;
            saw_header = true;
            let rest = rest.trim();
            if eq_ic(rest, "horizontal") {
                orientation = Orientation::Horizontal;
            } else if eq_ic(rest, "vertical") {
                orientation = Orientation::Vertical;
            }
            continue;
        }

        if let Some(rest) = strip_keyword(line, "title") {
            let t = unquote(rest.trim());
            if !t.is_empty() {
                title = Some(t);
            }
            continue;
        }
        if let Some(rest) = strip_keyword(line, "x-axis") {
            categories = parse_x_axis(rest.trim())?;
            continue;
        }
        if let Some(rest) = strip_keyword(line, "y-axis") {
            let (yt, yr) = parse_y_axis(rest.trim())?;
            if yt.is_some() {
                y_title = yt;
            }
            if yr.is_some() {
                y_range = yr;
            }
            continue;
        }
        if let Some(rest) = strip_keyword(line, "bar") {
            series.push(parse_series(SeriesKind::Bar, rest.trim())?);
            continue;
        }
        if let Some(rest) = strip_keyword(line, "line") {
            series.push(parse_series(SeriesKind::Line, rest.trim())?);
            continue;
        }
        // Tolerate accessibility directives and unknown lines silently.
        if strip_keyword(line, "accTitle").is_some()
            || strip_keyword(line, "accDescr").is_some()
        {
            continue;
        }
        return Err(format!("unrecognized xychart line: {line:?}"));
    }

    if !saw_header {
        return Err("empty input / no 'xychart' header".to_string());
    }

    // If no x-axis was declared but we have series, synthesize categories from
    // the longest series length so bars/lines still place.
    if categories.is_empty() {
        let n = series.iter().map(|s| s.values.len()).max().unwrap_or(0);
        categories = (1..=n).map(|i| i.to_string()).collect();
    }

    Ok(XyChart {
        orientation,
        title,
        categories,
        y_title,
        y_range,
        series,
    })
}

/// Strip a leading keyword if `line` starts with it followed by end-of-string or
/// whitespace (case-insensitive). Returns the remainder (with leading ws kept).
fn strip_keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    if line.len() < kw.len() {
        return None;
    }
    let (head, tail) = line.split_at(kw.len());
    if head.eq_ignore_ascii_case(kw) && (tail.is_empty() || tail.starts_with(char::is_whitespace)) {
        Some(tail)
    } else {
        None
    }
}

fn eq_ic(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Strip surrounding double quotes from `s` if present, else return `s` trimmed.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parse the tail of an `x-axis` line into category labels.
///
/// Forms: `[a, b, c]`, `"Title" [a, b, c]`, `<min> --> <max>`,
/// `"Title" <min> --> <max>`. A numeric range is turned into a handful of
/// evenly-spaced synthetic tick labels so the chart still draws.
fn parse_x_axis(tail: &str) -> Result<Vec<String>, String> {
    let mut rest = tail.trim();
    // Optional leading quoted axis title — we keep category parsing simple and
    // just drop the x title (not drawn distinctly).
    if rest.starts_with('"') {
        if let Some(end) = rest[1..].find('"') {
            rest = rest[end + 2..].trim();
        }
    }
    if let Some(inner) = bracketed(rest) {
        let cats = split_items(inner)
            .into_iter()
            .map(|s| unquote(s.trim()))
            .collect::<Vec<_>>();
        return Ok(cats);
    }
    // Numeric range `min --> max`.
    if let Some((lo, hi)) = parse_range(rest) {
        // Synthesize 5 evenly-spaced labels across the range.
        let n = 5usize;
        let mut cats = Vec::with_capacity(n);
        for i in 0..n {
            let v = lo + (hi - lo) * (i as f64) / ((n - 1) as f64);
            cats.push(fmt_num(v));
        }
        return Ok(cats);
    }
    Err(format!("malformed x-axis: {tail:?}"))
}

/// Parse the tail of a `y-axis` line: optional quoted title, optional
/// `min --> max` range. Returns `(title, range)` with either/both possibly
/// `None`.
fn parse_y_axis(tail: &str) -> Result<(Option<String>, Option<(f64, f64)>), String> {
    let mut rest = tail.trim();
    let mut title = None;
    if rest.starts_with('"') {
        if let Some(end) = rest[1..].find('"') {
            let t = rest[1..end + 1].to_string();
            if !t.is_empty() {
                title = Some(t);
            }
            rest = rest[end + 2..].trim();
        }
    }
    if rest.is_empty() {
        return Ok((title, None));
    }
    if let Some(range) = parse_range(rest) {
        return Ok((title, Some(range)));
    }
    // A bare unquoted title with no range, e.g. `y-axis Revenue`.
    if title.is_none() {
        return Ok((Some(rest.to_string()), None));
    }
    Err(format!("malformed y-axis: {tail:?}"))
}

/// Parse `min --> max` into `(min, max)`, normalizing so `min <= max`.
fn parse_range(s: &str) -> Option<(f64, f64)> {
    let (a, b) = s.split_once("-->")?;
    let lo: f64 = a.trim().parse().ok()?;
    let hi: f64 = b.trim().parse().ok()?;
    if !lo.is_finite() || !hi.is_finite() {
        return None;
    }
    Some((lo.min(hi), lo.max(hi)))
}

/// Parse a `bar`/`line` series tail: optional (quoted) title then `[v1, v2, …]`.
fn parse_series(kind: SeriesKind, tail: &str) -> Result<Series, String> {
    let mut rest = tail.trim();
    let mut title = String::new();
    if rest.starts_with('"') {
        if let Some(end) = rest[1..].find('"') {
            title = rest[1..end + 1].to_string();
            rest = rest[end + 2..].trim();
        }
    }
    let inner = bracketed(rest)
        .ok_or_else(|| format!("expected '[...]' values in series, got: {tail:?}"))?;
    let mut values = Vec::new();
    for item in split_items(inner) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let v: f64 = item
            .parse()
            .map_err(|_| format!("non-numeric series value: {item:?}"))?;
        if !v.is_finite() {
            return Err(format!("non-finite series value: {item:?}"));
        }
        values.push(v);
    }
    Ok(Series { kind, title, values })
}

/// If `s` is `[ ... ]`, return the inner slice; else `None`.
fn bracketed(s: &str) -> Option<&str> {
    let s = s.trim();
    let inner = s.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner)
}

/// Split a comma-separated list, respecting double-quoted items (so a category
/// label may contain a comma).
fn split_items(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_quote = false;
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quote = !in_quote,
            b',' if !in_quote => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = &s[start..];
    if !tail.trim().is_empty() || !out.is_empty() {
        out.push(tail);
    }
    out
}

/// Format a number without a trailing `.0` for clean labels.
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        // Trim to a few significant decimals to keep tick labels tidy.
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// Series color palette (cycled). Approximates mermaid's xychart default theme.
const PALETTE: [[u8; 4]; 8] = [
    [0x3A, 0x86, 0xFF, 255], // blue
    [0xFB, 0x5C, 0x65, 255], // red
    [0x2E, 0xC4, 0x7E, 255], // green
    [0xFF, 0xA6, 0x00, 255], // amber
    [0x8E, 0x5C, 0xF7, 255], // purple
    [0x00, 0xBC, 0xD4, 255], // cyan
    [0xE0, 0x6C, 0xD6, 255], // magenta
    [0x9E, 0x9E, 0x9E, 255], // gray
];

fn palette(i: usize) -> [u8; 4] {
    PALETTE[i % PALETTE.len()]
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

const MARGIN: f32 = 16.0;
const PLOT_W: f32 = 520.0;
const PLOT_H: f32 = 320.0;
const Y_TICKS: usize = 5;
/// Fraction of a category slot occupied by (all) bars together.
const BAR_GROUP_FRAC: f32 = 0.7;
const AXIS_STROKE_W: f32 = 1.5;

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render mermaid xychart source to an SVG document.
pub fn render_xychart(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let chart = parse_xychart(src).map_err(MermaidError::Parse)?;

    // Need at least one series carrying at least one value.
    if chart.series.is_empty() || chart.series.iter().all(|s| s.values.is_empty()) {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let title_fs = fs * 1.4;

    let n_cats = chart.categories.len().max(1);

    // Y scale: explicit range, else auto-fit from data.
    let (y_min, y_max) = y_scale(&chart);

    // ---- Gutters / plot rectangle -------------------------------------
    // Left gutter holds y tick labels (+ rotated y title). Bottom holds x
    // category labels (+ x/y axis title band). Top holds the chart title.
    let max_tick_chars = (0..=Y_TICKS)
        .map(|i| {
            let v = y_min + (y_max - y_min) * (i as f64) / (Y_TICKS as f64);
            fmt_num(v).chars().count()
        })
        .max()
        .unwrap_or(1) as f32;
    let tick_label_w = max_tick_chars * fs * 0.6;
    let y_title_band = if chart.y_title.is_some() { fs * 1.4 } else { 0.0 };
    let left_gutter = MARGIN + y_title_band + tick_label_w + 8.0;

    let title_band = if chart.title.is_some() { title_fs + MARGIN } else { MARGIN };
    let x_label_band = fs * 1.4;
    let bottom_gutter = x_label_band + MARGIN;

    let plot_x = left_gutter;
    let plot_y = title_band;
    let plot_w = PLOT_W;
    let plot_h = PLOT_H;

    let width = plot_x + plot_w + MARGIN;
    let height = plot_y + plot_h + bottom_gutter;

    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);

    // Map a y data value to a pixel y (top of plot = y_max, bottom = y_min).
    let span = (y_max - y_min).max(1e-9);
    let y_px = |v: f64| -> f32 {
        let t = ((v - y_min) / span) as f32;
        plot_y + plot_h - t * plot_h
    };
    // Category slot centers across the plot width.
    let slot_w = plot_w / n_cats as f32;
    let cat_center = |i: usize| -> f32 { plot_x + slot_w * (i as f32 + 0.5) };

    let baseline = y_px(y_min.max(0.0).min(y_max).max(y_min));

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">"
    );

    // ---- Title --------------------------------------------------------
    if let Some(t) = &chart.title {
        emit_text(
            &mut svg,
            t,
            plot_x + plot_w / 2.0,
            title_band / 2.0,
            title_fs,
            "middle",
            opts.text_color,
            true,
            &opts.font_family,
        );
    }

    // ---- Gridlines + y ticks/labels -----------------------------------
    for i in 0..=Y_TICKS {
        let v = y_min + (y_max - y_min) * (i as f64) / (Y_TICKS as f64);
        let gy = y_px(v);
        // Light horizontal gridline.
        let _ = write!(
            svg,
            "<line x1=\"{x1:.2}\" y1=\"{gy:.2}\" x2=\"{x2:.2}\" y2=\"{gy:.2}\" stroke=\"rgb(221,221,221)\" stroke-width=\"1\"/>",
            x1 = plot_x,
            x2 = plot_x + plot_w,
        );
        // Tick mark.
        let _ = write!(
            svg,
            "<line x1=\"{x1:.2}\" y1=\"{gy:.2}\" x2=\"{x2:.2}\" y2=\"{gy:.2}\" stroke=\"{s}\" stroke-width=\"1\"/>",
            x1 = plot_x - 4.0,
            x2 = plot_x,
            s = rgb(opts.edge_stroke),
        );
        // Numeric label.
        emit_text(
            &mut svg,
            &fmt_num(v),
            plot_x - 7.0,
            gy,
            fs,
            "end",
            opts.text_color,
            false,
            &opts.font_family,
        );
    }

    // ---- Axis lines ---------------------------------------------------
    // Y axis.
    let _ = write!(
        svg,
        "<line x1=\"{x:.2}\" y1=\"{y1:.2}\" x2=\"{x:.2}\" y2=\"{y2:.2}\" stroke=\"{s}\" stroke-width=\"{AXIS_STROKE_W}\"/>",
        x = plot_x,
        y1 = plot_y,
        y2 = plot_y + plot_h,
        s = rgb(opts.edge_stroke),
    );
    // X axis (drawn along the plot bottom; bars rise/fall from the data baseline).
    let _ = write!(
        svg,
        "<line x1=\"{x1:.2}\" y1=\"{y:.2}\" x2=\"{x2:.2}\" y2=\"{y:.2}\" stroke=\"{s}\" stroke-width=\"{AXIS_STROKE_W}\"/>",
        x1 = plot_x,
        x2 = plot_x + plot_w,
        y = plot_y + plot_h,
        s = rgb(opts.edge_stroke),
    );

    // ---- Bars ---------------------------------------------------------
    let bar_series: Vec<(usize, &Series)> = chart
        .series
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == SeriesKind::Bar)
        .collect();
    let n_bar = bar_series.len().max(1);
    let group_w = slot_w * BAR_GROUP_FRAC;
    let bar_w = group_w / n_bar as f32;

    for (bi, (si, s)) in bar_series.iter().enumerate() {
        let color = palette(*si);
        for (ci, &v) in s.values.iter().enumerate() {
            if ci >= n_cats {
                break;
            }
            let center = cat_center(ci);
            let group_left = center - group_w / 2.0;
            let x = group_left + bar_w * bi as f32;
            let top = y_px(v.max(y_min).min(y_max));
            let base = baseline;
            let (ry, rh) = if top <= base { (top, base - top) } else { (base, top - base) };
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{ry:.2}\" width=\"{bw:.2}\" height=\"{rh:.2}\" fill=\"{f}\" stroke=\"none\"/>",
                bw = bar_w.max(0.5),
                f = rgb(color),
            );
        }
    }

    // ---- Lines --------------------------------------------------------
    for (si, s) in chart.series.iter().enumerate() {
        if s.kind != SeriesKind::Line {
            continue;
        }
        let color = palette(si);
        let mut pts = String::new();
        let mut dots: Vec<(f32, f32)> = Vec::new();
        for (ci, &v) in s.values.iter().enumerate() {
            if ci >= n_cats {
                break;
            }
            let x = cat_center(ci);
            let y = y_px(v.max(y_min).min(y_max));
            if !pts.is_empty() {
                pts.push(' ');
            }
            let _ = write!(pts, "{x:.2},{y:.2}");
            dots.push((x, y));
        }
        if dots.len() >= 2 {
            let _ = write!(
                svg,
                "<polyline points=\"{pts}\" fill=\"none\" stroke=\"{s}\" stroke-width=\"2\"/>",
                s = rgb(color),
            );
        }
        for (x, y) in dots {
            let _ = write!(
                svg,
                "<circle cx=\"{x:.2}\" cy=\"{y:.2}\" r=\"3\" fill=\"{f}\"/>",
                f = rgb(color),
            );
        }
    }

    // ---- X category labels --------------------------------------------
    for (i, label) in chart.categories.iter().enumerate() {
        let cx = cat_center(i);
        emit_text(
            &mut svg,
            label,
            cx,
            plot_y + plot_h + fs,
            fs,
            "middle",
            opts.text_color,
            false,
            &opts.font_family,
        );
    }

    // ---- Y axis title (rotated) ---------------------------------------
    if let Some(t) = &chart.y_title {
        let tx = MARGIN + y_title_band / 2.0;
        let ty = plot_y + plot_h / 2.0;
        let [r, g, b, _] = opts.text_color;
        let _ = write!(
            svg,
            "<text x=\"{tx:.2}\" y=\"{ty:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
             transform=\"rotate(-90 {tx:.2} {ty:.2})\" font-family=\"{family}\" font-size=\"{fs}\" \
             fill=\"rgb({r},{g},{b})\">{txt}</text>",
            family = escape(&opts.font_family),
            txt = escape(t),
        );
    }

    svg.push_str("</svg>");

    Ok(MermaidRender { svg, width_px: w, height_px: h })
}

/// Compute the `(min, max)` y scale: explicit range if given, else auto-fit from
/// the data as `[min(0, data_min), data_max]` with ~5% headroom.
fn y_scale(chart: &XyChart) -> (f64, f64) {
    if let Some((lo, hi)) = chart.y_range {
        if (hi - lo).abs() > f64::EPSILON {
            return (lo, hi);
        }
    }
    let mut data_min = f64::INFINITY;
    let mut data_max = f64::NEG_INFINITY;
    for s in &chart.series {
        for &v in &s.values {
            data_min = data_min.min(v);
            data_max = data_max.max(v);
        }
    }
    if !data_min.is_finite() || !data_max.is_finite() {
        return (0.0, 1.0);
    }
    let lo = data_min.min(0.0);
    let mut hi = data_max;
    if (hi - lo).abs() < f64::EPSILON {
        // Flat data — give the axis a unit of span so it renders.
        hi = lo + 1.0;
    } else {
        hi += (hi - lo) * 0.05; // headroom
    }
    (lo, hi)
}

/// Emit a `<text>` element. `anchor` is the SVG `text-anchor`; `bold` toggles
/// the title weight.
#[allow(clippy::too_many_arguments)]
fn emit_text(
    svg: &mut String,
    text: &str,
    x: f32,
    y: f32,
    fs: f32,
    anchor: &str,
    color: [u8; 4],
    bold: bool,
    family: &str,
) {
    let [r, g, b, _] = color;
    let weight = if bold { " font-weight=\"bold\"" } else { "" };
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"{anchor}\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\"{weight} fill=\"rgb({r},{g},{b})\">{txt}</text>",
        family = escape(family),
        txt = escape(text),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"xychart-beta
    title "Sales Revenue"
    x-axis [jan, feb, mar, apr]
    y-axis "Revenue (k$)" 0 --> 100
    bar [50, 60, 75, 40]
    line [40, 55, 70, 35]
"#;

    #[test]
    fn parses_title_axes_and_series() {
        let c = parse_xychart(SAMPLE).expect("parse");
        assert_eq!(c.orientation, Orientation::Vertical);
        assert_eq!(c.title.as_deref(), Some("Sales Revenue"));
        assert_eq!(c.categories, vec!["jan", "feb", "mar", "apr"]);
        assert_eq!(c.y_title.as_deref(), Some("Revenue (k$)"));
        assert_eq!(c.y_range, Some((0.0, 100.0)));
        assert_eq!(c.series.len(), 2);
        assert_eq!(c.series[0].kind, SeriesKind::Bar);
        assert_eq!(c.series[0].values, vec![50.0, 60.0, 75.0, 40.0]);
        assert_eq!(c.series[1].kind, SeriesKind::Line);
        assert_eq!(c.series[1].values, vec![40.0, 55.0, 70.0, 35.0]);
    }

    #[test]
    fn parses_horizontal_orientation() {
        let c = parse_xychart("xychart-beta horizontal\nbar [1, 2]\n").expect("parse");
        assert_eq!(c.orientation, Orientation::Horizontal);
    }

    #[test]
    fn accepts_plain_xychart_header() {
        let c = parse_xychart("xychart\nbar [1, 2, 3]\n").expect("parse");
        assert_eq!(c.series.len(), 1);
        // No x-axis → synthesized 1..=3 categories.
        assert_eq!(c.categories, vec!["1", "2", "3"]);
    }

    #[test]
    fn x_axis_with_quoted_title() {
        let c = parse_xychart("xychart-beta\nx-axis \"Month\" [a, b, c]\nbar [1,2,3]\n")
            .expect("parse");
        assert_eq!(c.categories, vec!["a", "b", "c"]);
    }

    #[test]
    fn numeric_x_axis_synthesizes_categories() {
        let c = parse_xychart("xychart-beta\nx-axis 0 --> 100\nline [1,2,3,4,5]\n").expect("parse");
        assert_eq!(c.categories.len(), 5);
        assert_eq!(c.categories.first().map(|s| s.as_str()), Some("0"));
        assert_eq!(c.categories.last().map(|s| s.as_str()), Some("100"));
    }

    #[test]
    fn y_axis_title_only_no_range() {
        let c = parse_xychart("xychart-beta\nx-axis [a,b]\ny-axis \"Score\"\nbar [3, 9]\n")
            .expect("parse");
        assert_eq!(c.y_title.as_deref(), Some("Score"));
        assert_eq!(c.y_range, None);
    }

    #[test]
    fn auto_y_range_when_omitted() {
        let c = parse_xychart("xychart-beta\nx-axis [a,b,c]\nbar [10, 20, 40]\n").expect("parse");
        assert_eq!(c.y_range, None);
        let (lo, hi) = y_scale(&c);
        assert_eq!(lo, 0.0, "min(0, data_min) when all positive");
        assert!(hi >= 40.0, "max includes data peak + headroom, got {hi}");
    }

    #[test]
    fn auto_y_range_includes_negatives() {
        let c = parse_xychart("xychart-beta\nx-axis [a,b]\nbar [-5, 8]\n").expect("parse");
        let (lo, hi) = y_scale(&c);
        assert!(lo <= -5.0, "min covers negative data, got {lo}");
        assert!(hi >= 8.0, "max covers peak, got {hi}");
    }

    #[test]
    fn parses_titled_series() {
        let c = parse_xychart("xychart-beta\nx-axis [a,b]\nbar \"Rev\" [1, 2]\n").expect("parse");
        assert_eq!(c.series[0].title, "Rev");
        assert_eq!(c.series[0].values, vec![1.0, 2.0]);
    }

    #[test]
    fn parses_decimal_and_negative_values() {
        let c = parse_xychart("xychart-beta\nx-axis [a,b,c]\nline [1.5, -2.25, 3]\n")
            .expect("parse");
        assert_eq!(c.series[0].values, vec![1.5, -2.25, 3.0]);
    }

    #[test]
    fn ignores_comments_and_blanks() {
        let src = "xychart-beta\n%% a comment\n\nx-axis [a,b]  %% inline\nbar [1, 2]\n";
        let c = parse_xychart(src).expect("parse");
        assert_eq!(c.categories, vec!["a", "b"]);
        assert_eq!(c.series.len(), 1);
    }

    #[test]
    fn multiple_bar_series_overlay() {
        let c = parse_xychart("xychart-beta\nx-axis [a,b]\nbar [1,2]\nbar [3,4]\n").expect("parse");
        assert_eq!(c.series.len(), 2);
        assert!(c.series.iter().all(|s| s.kind == SeriesKind::Bar));
    }

    // ---- Render -------------------------------------------------------

    #[test]
    fn render_well_formed_svg() {
        let r = render_xychart(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"), "got: {}", &r.svg[..40.min(r.svg.len())]);
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_has_axis_lines() {
        let r = render_xychart(SAMPLE, &MermaidOptions::default()).expect("render");
        // At least the y-axis and x-axis lines (plus gridlines/ticks).
        assert!(r.svg.matches("<line").count() >= 2, "expected axis lines");
    }

    #[test]
    fn render_one_bar_rect_per_category() {
        let r = render_xychart(SAMPLE, &MermaidOptions::default()).expect("render");
        // One bar series × 4 categories → 4 rects.
        assert_eq!(r.svg.matches("<rect").count(), 4, "one rect per category");
    }

    #[test]
    fn render_bars_per_series_per_category() {
        let src = "xychart-beta\nx-axis [a,b,c]\nbar [1,2,3]\nbar [4,5,6]\n";
        let r = render_xychart(src, &MermaidOptions::default()).expect("render");
        // Two bar series × 3 categories → 6 rects.
        assert_eq!(r.svg.matches("<rect").count(), 6);
    }

    #[test]
    fn render_polyline_per_line_series() {
        let r = render_xychart(SAMPLE, &MermaidOptions::default()).expect("render");
        assert_eq!(r.svg.matches("<polyline").count(), 1, "one polyline per line series");
        // Line dots present.
        assert!(r.svg.contains("<circle"), "expected line dots");
    }

    #[test]
    fn render_has_category_labels_and_title() {
        let r = render_xychart(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("Sales Revenue"), "title present");
        for cat in ["jan", "feb", "mar", "apr"] {
            assert!(r.svg.contains(cat), "category {cat} present");
        }
        assert!(r.svg.contains("Revenue (k$)"), "y-axis title present");
    }

    #[test]
    fn render_xml_escapes_text() {
        let src = "xychart-beta\ntitle \"A & B <x>\"\nx-axis [\"p<q\", r]\nbar [1, 2]\n";
        let r = render_xychart(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("A &amp; B &lt;x&gt;"), "title escaped");
        assert!(r.svg.contains("p&lt;q"), "category escaped");
        assert!(!r.svg.contains("A & B"));
    }

    #[test]
    fn render_auto_y_range_no_explicit_axis() {
        let src = "xychart-beta\nx-axis [a,b,c]\nbar [10, 20, 40]\n";
        let r = render_xychart(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"));
        assert_eq!(r.svg.matches("<rect").count(), 3);
    }

    #[test]
    fn empty_no_series_errors() {
        let r = render_xychart("xychart-beta\nx-axis [a,b]\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Empty)), "got {r:?}");
    }

    #[test]
    fn empty_input_parse_errors() {
        match render_xychart("", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn bad_header_errors() {
        let r = render_xychart("graph TD\nA-->B\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    #[test]
    fn non_numeric_series_value_errors() {
        let r = render_xychart(
            "xychart-beta\nx-axis [a,b]\nbar [1, oops]\n",
            &MermaidOptions::default(),
        );
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let a = render_xychart(SAMPLE, &opts).expect("a");
        let b = render_xychart(SAMPLE, &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
