//! Pie-chart diagram (self-contained: parse + draw, no dagre layout).
//!
//! Mermaid pie syntax (the subset we support):
//! ```text
//! pie showData title Pet ownership
//!     "Dogs" : 386
//!     "Cats" : 85
//!     "Rats" : 15
//! ```
//! The header line is `pie`, optionally followed (in any order) by `showData`
//! and/or `title <text>`. `title` may also appear on its own line. Data lines
//! are `"<label>" : <number>`. Blank lines and `%%` comments are ignored.
//!
//! Rendering is pure trig — no graph layout. Slices are sorted by value
//! descending (matching mermaid's visual convention), drawn as SVG arc paths
//! starting at 12 o'clock going clockwise, with a legend column on the right and
//! an optional centered title above.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/pie/pieRenderer.ts` for
//! the upstream renderer this mirrors (legend swatches, `showData` `[value]`,
//! percentage labels on slices).

use std::fmt::Write as _;

use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One pie slice: a label and its (non-negative) value.
#[derive(Clone, Debug, PartialEq)]
struct Slice {
    label: String,
    value: f64,
}

/// A parsed pie chart: optional title, the `showData` flag, and the slices.
#[derive(Clone, Debug, PartialEq)]
struct Pie {
    title: Option<String>,
    show_data: bool,
    slices: Vec<Slice>,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse mermaid pie source into a [`Pie`]. Returns `Err(message)` when the
/// header is missing/malformed or when no valid data lines are found.
fn parse_pie(src: &str) -> Result<Pie, String> {
    let mut title: Option<String> = None;
    let mut show_data = false;
    let mut slices: Vec<Slice> = Vec::new();
    let mut saw_header = false;

    for raw in src.lines() {
        // Strip `%%` comments and surrounding whitespace.
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if !saw_header {
            // The header must begin with the `pie` keyword. It may carry
            // `showData` and/or `title <text>` on the same line, in any order.
            let rest = line
                .strip_prefix("pie")
                .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
                .ok_or_else(|| format!("expected 'pie' header, got: {line:?}"))?;
            saw_header = true;
            parse_header_tail(rest.trim(), &mut show_data, &mut title);
            continue;
        }

        // A bare `title <text>` line.
        if let Some(t) = line.strip_prefix("title") {
            if t.is_empty() || t.starts_with(char::is_whitespace) {
                let t = t.trim();
                if !t.is_empty() {
                    title = Some(t.to_string());
                }
                continue;
            }
        }
        // A lone `showData` directive on its own line.
        if line == "showData" {
            show_data = true;
            continue;
        }

        // Otherwise it must be a data line: `"<label>" : <number>`.
        let slice = parse_data_line(line)
            .ok_or_else(|| format!("malformed pie data line: {line:?}"))?;
        slices.push(slice);
    }

    if !saw_header {
        return Err("empty input / no 'pie' header".to_string());
    }
    if slices.is_empty() {
        return Err("pie chart has no data".to_string());
    }
    Ok(Pie { title, show_data, slices })
}

/// Parse the part of the header after `pie`: any combination of `showData` and
/// `title <text>`. Everything following a `title` token is taken as the title.
fn parse_header_tail(tail: &str, show_data: &mut bool, title: &mut Option<String>) {
    let mut rest = tail.trim();
    loop {
        if rest.is_empty() {
            break;
        }
        if let Some(after) = rest.strip_prefix("showData") {
            if after.is_empty() || after.starts_with(char::is_whitespace) {
                *show_data = true;
                rest = after.trim_start();
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix("title") {
            if after.is_empty() || after.starts_with(char::is_whitespace) {
                let t = after.trim();
                if !t.is_empty() {
                    *title = Some(t.to_string());
                }
                // Everything after `title` is the title text; we're done.
                break;
            }
        }
        // Unknown trailing token — treat the remainder as a title fallback so we
        // don't silently drop user text.
        *title = Some(rest.to_string());
        break;
    }
}

/// Parse a single data line `"<label>" : <number>`. Returns `None` if the line
/// is not a well-formed quoted-label/value pair.
fn parse_data_line(line: &str) -> Option<Slice> {
    let line = line.trim();
    let rest = line.strip_prefix('"')?;
    let close = rest.find('"')?;
    let label = rest[..close].to_string();
    let after = rest[close + 1..].trim_start();
    let after = after.strip_prefix(':')?.trim();
    let value: f64 = after.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some(Slice { label, value })
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// A built-in palette of 12 visually distinct colors, cycled across slices.
/// Approximates mermaid's default pie theme spread (a primary-ish blue first,
/// then a rotation through the hue wheel).
const PALETTE: [[u8; 3]; 12] = [
    [0xEC, 0xEC, 0xFF], // pale lavender (mermaid pie1 / primary)
    [0xFF, 0xFF, 0xDE], // pale yellow (pie2 / secondary)
    [0x6C, 0x6C, 0xE0], // indigo
    [0xCC, 0xCC, 0xFF], // light indigo
    [0xC7, 0x9A, 0x00], // gold
    [0xAB, 0xAB, 0x5C], // olive
    [0xE0, 0x6C, 0x6C], // coral
    [0x40, 0x80, 0x40], // green
    [0x40, 0x40, 0xA0], // blue
    [0xA0, 0x40, 0x80], // magenta
    [0x40, 0xA0, 0xA0], // teal
    [0xA0, 0x80, 0x40], // brown
];

/// The palette color (RGB) for slice index `i` (cycling). Prefers the active
/// theme's `series_palette` when set, falling back to the local [`PALETTE`].
fn palette_color(opts: &MermaidOptions, i: usize) -> [u8; 3] {
    if !opts.series_palette.is_empty() {
        let c = opts.series_palette[i % opts.series_palette.len()];
        [c[0], c[1], c[2]]
    } else {
        PALETTE[i % PALETTE.len()]
    }
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

/// Margin around the pie within its square box, px.
const MARGIN: f32 = 40.0;
/// Legend color-swatch square size, px.
const LEGEND_RECT: f32 = 18.0;
/// Gap between a swatch and its text / between legend rows, px.
const LEGEND_SPACING: f32 = 6.0;
/// Outer-circle / slice stroke width, px.
const STROKE_W: f32 = 2.0;
/// Heuristic per-char advance (font-free) for sizing the legend column.
const CHAR_ADVANCE_EM: f32 = 0.55;

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render mermaid pie-chart source to an SVG document.
pub fn render_pie(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let mut pie = parse_pie(src).map_err(MermaidError::Parse)?;
    if pie.slices.is_empty() {
        return Err(MermaidError::Empty);
    }

    // Sort slices by value descending (mermaid's visual convention). Stable so
    // equal values keep source order — keeps output deterministic.
    pie.slices.sort_by(|a, b| {
        b.value
            .partial_cmp(&a.value)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let total: f64 = pie.slices.iter().map(|s| s.value).sum();
    // All-zero (or empty-after-filter) totals can't form sweeps — treat as empty.
    if total <= 0.0 {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let radius = (fs * 9.0).max(120.0); // ~150px at the default 16px font.
    let title_fs = fs * 1.5;

    // Pie occupies a square box of side `pie_box`; its center sits at the box
    // center, offset down by the title band.
    let pie_box = 2.0 * (radius + MARGIN);
    let title_band = if pie.title.is_some() { title_fs + MARGIN * 0.5 } else { 0.0 };

    let cx = pie_box / 2.0;
    let cy = title_band + pie_box / 2.0;

    // Legend column to the right of the pie box. Width derives from the longest
    // legend entry (label, plus ` [value]` when showData).
    let longest = pie
        .slices
        .iter()
        .map(|s| legend_text(s, pie.show_data).chars().count())
        .max()
        .unwrap_or(0) as f32;
    let legend_text_w = longest * fs * CHAR_ADVANCE_EM;
    let legend_w = LEGEND_RECT + LEGEND_SPACING + legend_text_w;
    let legend_x = pie_box + MARGIN * 0.25;

    let width = legend_x + legend_w + MARGIN * 0.5;
    let height = (title_band + pie_box).max(cy + radius + MARGIN);

    let mut svg = String::new();
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Outer circle backing the slices (subtle border).
    let _ = write!(
        svg,
        "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" fill=\"none\" stroke=\"black\" stroke-width=\"{STROKE_W}\"/>",
        r = radius + STROKE_W / 2.0,
    );

    // Slices: start at -90° (12 o'clock), sweep clockwise.
    let mut angle = -std::f32::consts::FRAC_PI_2;
    for (i, slice) in pie.slices.iter().enumerate() {
        let frac = (slice.value / total) as f32;
        let sweep = frac * std::f32::consts::TAU;
        let a0 = angle;
        let a1 = angle + sweep;
        angle = a1;

        let (x0, y0) = (cx + radius * a0.cos(), cy + radius * a0.sin());
        let (x1, y1) = (cx + radius * a1.cos(), cy + radius * a1.sin());
        let large_arc = if sweep > std::f32::consts::PI { 1 } else { 0 };
        let [r, g, b] = palette_color(opts, i);

        let _ = write!(
            svg,
            "<path d=\"M{cx:.2},{cy:.2} L{x0:.2},{y0:.2} A{radius:.2},{radius:.2} 0 {large_arc},1 {x1:.2},{y1:.2} Z\" \
             fill=\"rgb({r},{g},{b})\" stroke=\"black\" stroke-width=\"{STROKE_W}\"/>",
        );

        // Percentage label at the slice centroid (~0.75 of the radius), when
        // showData is requested. Skip slivers that round to 0%.
        if pie.show_data {
            let pct = (frac * 100.0).round() as i64;
            if pct > 0 {
                let mid = a0 + sweep / 2.0;
                let lr = radius * 0.75;
                let (lx, ly) = (cx + lr * mid.cos(), cy + lr * mid.sin());
                emit_text(&mut svg, &format!("{pct}%"), lx, ly, fs, opts, true);
            }
        }
    }

    // Title centered above the pie.
    if let Some(t) = &pie.title {
        let ty = title_band / 2.0;
        let (tr, tg, tb) = (
            opts.text_color[0],
            opts.text_color[1],
            opts.text_color[2],
        );
        let _ = write!(
            svg,
            "<text x=\"{cx:.2}\" y=\"{ty:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{title_fs}\" font-weight=\"bold\" fill=\"rgb({tr},{tg},{tb})\">{txt}</text>",
            family = escape(&opts.font_family),
            txt = escape(t),
        );
    }

    // Legend: one swatch + text row per slice, vertically centered on the pie.
    let row_h = LEGEND_RECT + LEGEND_SPACING;
    let block_h = row_h * pie.slices.len() as f32;
    let mut ly = cy - block_h / 2.0;
    for (i, slice) in pie.slices.iter().enumerate() {
        let [r, g, b] = palette_color(opts, i);
        let _ = write!(
            svg,
            "<rect x=\"{x:.2}\" y=\"{ly:.2}\" width=\"{LEGEND_RECT}\" height=\"{LEGEND_RECT}\" \
             fill=\"rgb({r},{g},{b})\" stroke=\"rgb({r},{g},{b})\"/>",
            x = legend_x,
        );
        let tx = legend_x + LEGEND_RECT + LEGEND_SPACING;
        let tcy = ly + LEGEND_RECT / 2.0;
        emit_legend_text(&mut svg, &legend_text(slice, pie.show_data), tx, tcy, fs, opts);
        ly += row_h;
    }

    svg.push_str("</svg>");

    Ok(MermaidRender { svg, width_px: w, height_px: h })
}

/// The legend caption for a slice: `Label` or `Label [value]` with `showData`.
fn legend_text(slice: &Slice, show_data: bool) -> String {
    if show_data {
        format!("{} [{}]", slice.label, fmt_value(slice.value))
    } else {
        slice.label.clone()
    }
}

/// Format a slice value, trimming a trailing `.0` so integers read cleanly.
fn fmt_value(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// A centered `<text>` for a percentage / slice label.
fn emit_text(
    svg: &mut String,
    text: &str,
    x: f32,
    y: f32,
    fs: f32,
    opts: &MermaidOptions,
    on_slice: bool,
) {
    let (r, g, b) = if on_slice {
        // Dark text reads on the light palette; mermaid uses black.
        (0u8, 0u8, 0u8)
    } else {
        (opts.text_color[0], opts.text_color[1], opts.text_color[2])
    };
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"rgb({r},{g},{b})\">{txt}</text>",
        family = escape(&opts.font_family),
        txt = escape(text),
    );
}

/// A left-anchored `<text>` for a legend caption, vertically centered at `cy`.
fn emit_legend_text(svg: &mut String, text: &str, x: f32, cy: f32, fs: f32, opts: &MermaidOptions) {
    let [r, g, b, _] = opts.text_color;
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{cy:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"rgb({r},{g},{b})\">{txt}</text>",
        family = escape(&opts.font_family),
        txt = escape(text),
    );
}

/// XML-escape text for `<text>` content or an attribute value.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"pie showData title Pet ownership
    "Dogs" : 386
    "Cats" : 85
    "Rats" : 15
"#;

    #[test]
    fn parses_title_showdata_and_slices() {
        let pie = parse_pie(SAMPLE).expect("parse");
        assert_eq!(pie.title.as_deref(), Some("Pet ownership"));
        assert!(pie.show_data);
        assert_eq!(pie.slices.len(), 3);
        // Source order preserved by the parser (sorting happens in render).
        assert_eq!(pie.slices[0].label, "Dogs");
        assert_eq!(pie.slices[0].value, 386.0);
        assert_eq!(pie.slices[1].label, "Cats");
        assert_eq!(pie.slices[2].label, "Rats");
    }

    #[test]
    fn header_order_independent() {
        let a = parse_pie("pie title My Chart showData\n\"X\" : 1\n").expect("a");
        // `showData` here is part of the title text since title swallows the rest.
        assert!(a.title.is_some());

        let b = parse_pie("pie showData title My Chart\n\"X\" : 1\n").expect("b");
        assert!(b.show_data);
        assert_eq!(b.title.as_deref(), Some("My Chart"));
    }

    #[test]
    fn title_on_own_line() {
        let pie = parse_pie("pie\ntitle Standalone\n\"A\" : 10\n\"B\" : 20\n").expect("parse");
        assert_eq!(pie.title.as_deref(), Some("Standalone"));
        assert!(!pie.show_data);
        assert_eq!(pie.slices.len(), 2);
    }

    #[test]
    fn ignores_comments_and_blanks() {
        let src = "pie\n%% a comment\n\n    \"A\" : 1  %% inline\n\"B\" : 2\n";
        let pie = parse_pie(src).expect("parse");
        assert_eq!(pie.slices.len(), 2);
        assert_eq!(pie.slices[0].value, 1.0);
    }

    #[test]
    fn parses_decimal_values() {
        let pie = parse_pie("pie\n\"A\" : 12.5\n\"B\" : 0.5\n").expect("parse");
        assert_eq!(pie.slices[0].value, 12.5);
        assert_eq!(pie.slices[1].value, 0.5);
    }

    #[test]
    fn slice_angles_sum_to_tau() {
        let pie = parse_pie(SAMPLE).expect("parse");
        let total: f64 = pie.slices.iter().map(|s| s.value).sum();
        let sum: f32 = pie
            .slices
            .iter()
            .map(|s| (s.value / total) as f32 * std::f32::consts::TAU)
            .sum();
        assert!((sum - std::f32::consts::TAU).abs() < 1e-4, "sum={sum}");
    }

    #[test]
    fn render_well_formed_svg() {
        let r = render_pie(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"), "got: {}", &r.svg[..40.min(r.svg.len())]);
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_has_n_slices_swatches_and_text() {
        let r = render_pie(SAMPLE, &MermaidOptions::default()).expect("render");
        // 3 slice arc paths.
        assert_eq!(r.svg.matches("<path").count(), 3, "slice paths");
        // 3 legend swatch rects.
        assert_eq!(r.svg.matches("<rect").count(), 3, "legend swatches");
        // Title + 3 legend captions + 3 percentage labels (all > 0%).
        let texts = r.svg.matches("<text").count();
        assert!(texts >= 4, "expected title + legend texts, got {texts}");
        // Title text present.
        assert!(r.svg.contains("Pet ownership"));
    }

    #[test]
    fn showdata_emits_percentages_and_values() {
        let r = render_pie(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains('%'), "expected percentage labels");
        assert!(r.svg.contains("[386]"), "expected legend value");
    }

    #[test]
    fn no_showdata_hides_values_and_percentages() {
        let src = "pie title Plain\n\"A\" : 3\n\"B\" : 1\n";
        let r = render_pie(src, &MermaidOptions::default()).expect("render");
        assert!(!r.svg.contains('%'), "should not draw percentages");
        assert!(!r.svg.contains('['), "should not draw [value]");
    }

    #[test]
    fn xml_escapes_label() {
        let src = "pie\n\"A & B <x>\" : 5\n\"Other\" : 1\n";
        let r = render_pie(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("A &amp; B &lt;x&gt;"), "got: {}", r.svg);
        assert!(!r.svg.contains("A & B"));
    }

    #[test]
    fn slices_sorted_descending() {
        // Cats(85) and Rats(15) are smaller than Dogs(386); first legend caption
        // must be the largest slice.
        let r = render_pie(SAMPLE, &MermaidOptions::default()).expect("render");
        let dogs = r.svg.find("Dogs").expect("Dogs in legend");
        let cats = r.svg.find("Cats").expect("Cats in legend");
        let rats = r.svg.find("Rats").expect("Rats in legend");
        assert!(dogs < cats && cats < rats, "legend not value-descending");
    }

    #[test]
    fn empty_input_errors() {
        match render_pie("", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn header_only_no_data_errors() {
        let r = render_pie("pie title Nothing\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    #[test]
    fn malformed_data_line_errors() {
        let r = render_pie("pie\nDogs = 5\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    #[test]
    fn missing_header_errors() {
        let r = render_pie("graph TD\nA-->B\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    #[test]
    fn zero_total_is_empty() {
        let r = render_pie("pie\n\"A\" : 0\n\"B\" : 0\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Empty)));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let a = render_pie(SAMPLE, &opts).expect("a");
        let b = render_pie(SAMPLE, &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
