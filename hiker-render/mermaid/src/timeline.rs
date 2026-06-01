//! `timeline` diagram (self-contained: parse + layout + draw). No dagre.
//!
//! Mermaid timeline syntax (the subset we support):
//! ```text
//! timeline
//!     title History of Social Media
//!     section 2002-2004
//!         2002 : LinkedIn
//!         2004 : Facebook : Google
//!     section 2005-2006
//!         2005 : Youtube
//!         2006 : Twitter
//! ```
//! The header line is `timeline` (an optional direction token like `LR`/`TD`
//! after it is accepted and ignored). Lines:
//! - `title <text>` — diagram title (drawn centered at the top).
//! - `section <name>` — starts a colored band grouping the periods that follow.
//! - **period line** `<period> : <event1> : <event2> : ...` — the text before the
//!   first `:` is the time period/label; each subsequent `:`-separated chunk is an
//!   event under that period. A period with no `:` is just a labelled point with no
//!   events.
//!
//! Layout: periods are laid left→right, one column each. A central horizontal axis
//! line runs across all columns; each period gets a colored period box sitting on
//! the axis, with its events stacked as small boxes below it. Sections are drawn as
//! colored background bands spanning the columns they group, with the section name
//! above. Colors come from a fixed palette indexed per section (periods with no
//! section share a default slot).
//!
//! Skipped (noted, not rendered): multiline continuation (a bare `:` continuation
//! line adding events to the previous period is **not** supported — every period is
//! a single line); per-task `@{}`/class styling.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/timeline/` for the
//! upstream parser/renderer this mirrors.

use std::fmt::Write as _;

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One time period column: its label, its events, and the section it belongs to.
#[derive(Clone, Debug, PartialEq)]
struct Period {
    label: String,
    events: Vec<String>,
    /// Index into [`Timeline::sections`], or `None` if before any `section`.
    section: Option<usize>,
}

/// A parsed timeline: optional title, the section names, and the period columns.
#[derive(Clone, Debug, PartialEq)]
struct Timeline {
    title: Option<String>,
    sections: Vec<String>,
    periods: Vec<Period>,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse timeline source into a [`Timeline`]. Returns `Err(message)` when the
/// `timeline` header is missing.
fn parse_timeline(src: &str) -> Result<Timeline, String> {
    let mut saw_header = false;
    let mut title: Option<String> = None;
    let mut sections: Vec<String> = Vec::new();
    let mut periods: Vec<Period> = Vec::new();
    let mut cur_section: Option<usize> = None;

    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if !saw_header {
            let first = line.split_whitespace().next().unwrap_or("");
            if first != "timeline" {
                return Err(format!("expected 'timeline' header, got: {line:?}"));
            }
            saw_header = true;
            continue;
        }

        // `title ...`
        if let Some(rest) = strip_keyword(line, "title") {
            title = Some(rest.trim().to_string());
            continue;
        }
        // `section ...`
        if let Some(rest) = strip_keyword(line, "section") {
            sections.push(rest.trim().to_string());
            cur_section = Some(sections.len() - 1);
            continue;
        }

        // Period line: split on `:` — first chunk is the period label, the rest
        // are events.
        let mut parts = line.split(':');
        let label = parts.next().unwrap_or("").trim().to_string();
        let events: Vec<String> = parts
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty())
            .collect();
        if label.is_empty() {
            // A continuation line (leading `:`) — unsupported; skip it.
            continue;
        }
        periods.push(Period { label, events, section: cur_section });
    }

    if !saw_header {
        return Err("empty input / no 'timeline' header".to_string());
    }
    Ok(Timeline { title, sections, periods })
}

/// If `line` begins with `kw` followed by whitespace (or is exactly `kw`),
/// return the remainder after the keyword; else `None`.
fn strip_keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    if rest.is_empty() {
        return Some("");
    }
    let first = rest.chars().next().unwrap();
    if first.is_whitespace() {
        Some(rest)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

/// Margin around the whole drawing, px.
const MARGIN: f32 = 24.0;
/// Fixed column width per period, px.
const COL_W: f32 = 160.0;
/// Horizontal gap between period columns, px.
const COL_GAP: f32 = 16.0;
/// Border / axis stroke width, px.
const STROKE_W: f32 = 1.5;
/// Corner radius for rounded boxes, px.
const CORNER_R: f32 = 6.0;
/// Vertical gap between stacked event boxes, px.
const EVENT_GAP: f32 = 8.0;
/// Vertical gap from the axis down to the first event box, px.
const AXIS_TO_EVENT: f32 = 18.0;

/// A small fixed palette (straight RGBA) cycled per section/period.
const PALETTE: [[u8; 4]; 8] = [
    [236, 236, 255, 255],
    [255, 236, 236, 255],
    [236, 255, 236, 255],
    [255, 248, 220, 255],
    [220, 248, 255, 255],
    [248, 236, 255, 255],
    [255, 236, 248, 255],
    [236, 255, 248, 255],
];

/// Pick a palette color for a slot index.
fn palette(i: usize) -> [u8; 4] {
    PALETTE[i % PALETTE.len()]
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render mermaid timeline source to an SVG document.
pub fn render_timeline(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let tl = parse_timeline(src).map_err(MermaidError::Parse)?;
    if tl.periods.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let px = opts.node_padding_x;
    let py = opts.node_padding_y;
    let line_h = fs * 1.2;

    let n = tl.periods.len();

    // Column x positions (left edge of each column).
    let col_x = |i: usize| MARGIN + i as f32 * (COL_W + COL_GAP);

    // Period box height (single label line).
    let period_h = line_h + 2.0 * py;
    // Event box heights (label wrapped to one or more lines, fit to COL_W).
    let inner_w = COL_W - 2.0 * px;
    let event_box_h = |label: &str| -> f32 {
        let lines = wrap_lines(label, inner_w, fs);
        lines.len() as f32 * line_h + 2.0 * py
    };

    // Vertical layout: [title] [section labels band] [period boxes on axis]
    // [events stacked below].
    let title_h = if tl.title.is_some() { line_h + 8.0 } else { 0.0 };
    let section_band_h = if tl.sections.is_empty() { 0.0 } else { line_h + 8.0 };

    let top = MARGIN + title_h + section_band_h;
    // Axis sits at the vertical center of the period box row.
    let period_top = top;
    let axis_y = period_top + period_h / 2.0;
    let events_top = period_top + period_h + AXIS_TO_EVENT;

    // Tallest column's events stack → board height.
    let mut max_events_h = 0.0f32;
    for p in &tl.periods {
        let mut h = 0.0f32;
        for ev in &p.events {
            h += event_box_h(ev) + EVENT_GAP;
        }
        max_events_h = max_events_h.max(h);
    }

    let width = MARGIN + n as f32 * (COL_W + COL_GAP) - COL_GAP + MARGIN;
    let height = events_top + max_events_h + MARGIN;
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Section bands first (drawn under everything): one rect spanning the
    // contiguous run of periods that share a section, plus the section name.
    if !tl.sections.is_empty() {
        let band_top = MARGIN + title_h;
        let band_bottom = h - MARGIN;
        let mut i = 0usize;
        while i < n {
            let sec = tl.periods[i].section;
            let mut j = i;
            while j < n && tl.periods[j].section == sec {
                j += 1;
            }
            // Run is periods [i, j). Draw a band only for real sections.
            if let Some(s) = sec {
                let x0 = col_x(i);
                let x1 = col_x(j - 1) + COL_W;
                let color = palette(s);
                let _ = write!(
                    svg,
                    "<rect x=\"{x0:.2}\" y=\"{band_top:.2}\" width=\"{bw:.2}\" \
                     height=\"{bh:.2}\" rx=\"{CORNER_R}\" ry=\"{CORNER_R}\" \
                     fill=\"{fill}\" fill-opacity=\"0.35\"/>",
                    bw = x1 - x0,
                    bh = band_bottom - band_top,
                    fill = rgb(color),
                );
                // Section label centered over the run, at the band top.
                draw_text(
                    &mut svg,
                    (x0 + x1) / 2.0,
                    MARGIN + title_h + line_h / 2.0,
                    &tl.sections[s],
                    fs,
                    opts,
                    true,
                );
            }
            i = j;
        }
    }

    // Title centered at the very top.
    if let Some(t) = &tl.title {
        draw_text(&mut svg, w / 2.0, MARGIN + line_h / 2.0, t, fs * 1.2, opts, true);
    }

    // Central horizontal axis line.
    let axis_x0 = col_x(0);
    let axis_x1 = col_x(n - 1) + COL_W;
    let _ = write!(
        svg,
        "<line x1=\"{axis_x0:.2}\" y1=\"{axis_y:.2}\" x2=\"{axis_x1:.2}\" \
         y2=\"{axis_y:.2}\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        stroke = rgb(opts.edge_stroke),
        so = opacity_attr("stroke-opacity", opts.edge_stroke),
    );

    // Period boxes + their events.
    for (i, p) in tl.periods.iter().enumerate() {
        let x = col_x(i);
        // Color: by section if any, else cycle by period index.
        let color = match p.section {
            Some(s) => palette(s),
            None => palette(i),
        };

        // Period box (on the axis).
        let _ = write!(
            svg,
            "<rect x=\"{x:.2}\" y=\"{period_top:.2}\" width=\"{COL_W:.2}\" \
             height=\"{period_h:.2}\" rx=\"{CORNER_R}\" ry=\"{CORNER_R}\" \
             fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
            fill = rgb(color),
            fo = opacity_attr("fill-opacity", color),
            stroke = rgb(opts.node_stroke),
            so = opacity_attr("stroke-opacity", opts.node_stroke),
        );
        draw_text(&mut svg, x + COL_W / 2.0, period_top + period_h / 2.0, &p.label, fs, opts, false);

        // Events stacked below.
        let mut ey = events_top;
        for ev in &p.events {
            let lines = wrap_lines(ev, inner_w, fs);
            let bh = lines.len() as f32 * line_h + 2.0 * py;
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{ey:.2}\" width=\"{COL_W:.2}\" \
                 height=\"{bh:.2}\" rx=\"{CORNER_R}\" ry=\"{CORNER_R}\" \
                 fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
                fill = rgb(color),
                fo = opacity_attr("fill-opacity", color),
                stroke = rgb(opts.node_stroke),
                so = opacity_attr("stroke-opacity", opts.node_stroke),
            );
            // Centered, possibly multi-line label.
            let total = line_h * lines.len() as f32;
            let mut ty = ey + bh / 2.0 - total / 2.0 + line_h / 2.0;
            for ln in &lines {
                draw_text(&mut svg, x + COL_W / 2.0, ty, ln, fs, opts, false);
                ty += line_h;
            }
            ey += bh + EVENT_GAP;
        }
    }

    svg.push_str("</svg>");

    Ok(MermaidRender { svg, width_px: w, height_px: h })
}

/// Greedy word-wrap of `text` to fit `max_w` px at `font_size`, using the
/// font-free advance heuristic from [`text_size`]. Always returns >= 1 line; a
/// single word wider than `max_w` is kept whole on its own line.
fn wrap_lines(text: &str, max_w: f32, font_size: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if text_size(&candidate, font_size).0 <= max_w || cur.is_empty() {
            cur = candidate;
        } else {
            lines.push(cur);
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Draw a single centered text line. `bold` emphasises titles/section names.
fn draw_text(svg: &mut String, cx: f32, cy: f32, text: &str, font_size: f32, opts: &MermaidOptions, bold: bool) {
    let [tr, tg, tb, _] = opts.text_color;
    let weight = if bold { " font-weight=\"bold\"" } else { "" };
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" \
         dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{fs}\"{weight} \
         fill=\"rgb({tr},{tg},{tb})\">{txt}</text>",
        family = escape(&opts.font_family),
        fs = font_size,
        txt = escape(text),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "timeline
    title History of Social Media
    section 2002-2004
        2002 : LinkedIn
        2004 : Facebook : Google
    section 2005-2006
        2005 : Youtube
";

    #[test]
    fn parses_title_sections_periods() {
        let tl = parse_timeline(SAMPLE).expect("parse");
        assert_eq!(tl.title.as_deref(), Some("History of Social Media"));
        assert_eq!(tl.sections, vec!["2002-2004", "2005-2006"]);
        assert_eq!(tl.periods.len(), 3);
        assert_eq!(tl.periods[0].label, "2002");
        assert_eq!(tl.periods[1].label, "2004");
        assert_eq!(tl.periods[2].label, "2005");
    }

    #[test]
    fn colon_splits_events() {
        let tl = parse_timeline(SAMPLE).expect("parse");
        assert_eq!(tl.periods[0].events, vec!["LinkedIn"]);
        // Two events split on the second `:`.
        assert_eq!(tl.periods[1].events, vec!["Facebook", "Google"]);
    }

    #[test]
    fn periods_track_their_section() {
        let tl = parse_timeline(SAMPLE).expect("parse");
        assert_eq!(tl.periods[0].section, Some(0));
        assert_eq!(tl.periods[1].section, Some(0));
        assert_eq!(tl.periods[2].section, Some(1));
    }

    #[test]
    fn period_without_events() {
        let src = "timeline\n    2002\n";
        let tl = parse_timeline(src).expect("parse");
        assert_eq!(tl.periods.len(), 1);
        assert_eq!(tl.periods[0].label, "2002");
        assert!(tl.periods[0].events.is_empty());
        assert_eq!(tl.periods[0].section, None);
    }

    #[test]
    fn direction_token_in_header_is_ignored() {
        let src = "timeline LR\n    2002 : a\n";
        let tl = parse_timeline(src).expect("parse");
        assert_eq!(tl.periods.len(), 1);
    }

    #[test]
    fn render_well_formed_svg() {
        let r = render_timeline(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_has_axis_one_box_per_period_and_events() {
        let r = render_timeline(SAMPLE, &MermaidOptions::default()).expect("render");
        // One central axis line.
        assert_eq!(r.svg.matches("<line").count(), 1, "axis; svg={}", r.svg);
        // 2 section bands + 3 period boxes + 4 event boxes (LinkedIn, Facebook,
        // Google, Youtube) = 9 rects.
        assert_eq!(r.svg.matches("<rect").count(), 9, "boxes; svg={}", r.svg);
        // Labels present.
        assert!(r.svg.contains(">2002<"));
        assert!(r.svg.contains(">LinkedIn<"));
        assert!(r.svg.contains(">Facebook<"));
        assert!(r.svg.contains(">Google<"));
        // Section names + title.
        assert!(r.svg.contains(">2002-2004<"));
        assert!(r.svg.contains(">History of Social Media<"));
    }

    #[test]
    fn xml_escapes_label() {
        let src = "timeline\n    2002 : A & B <x>\n";
        let r = render_timeline(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("A &amp; B &lt;x&gt;"), "got: {}", r.svg);
        assert!(!r.svg.contains("A & B <x>"));
    }

    #[test]
    fn empty_input_errors() {
        match render_timeline("", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn missing_header_errors() {
        let r = render_timeline("graph TD\nA-->B\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    #[test]
    fn header_only_is_empty() {
        let r = render_timeline("timeline\n    title Just a title\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Empty)));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let a = render_timeline(SAMPLE, &opts).expect("a");
        let b = render_timeline(SAMPLE, &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
