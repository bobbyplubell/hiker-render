//! `info` diagram (self-contained: parse + draw).
//!
//! Mermaid's `info` diagram is intentionally trivial — upstream it simply shows
//! the library version. Syntax is just the header line `info`, optionally
//! followed by a `showInfo` line; there are no data rows. We render a small
//! rounded "about" card with the library name on top and a smaller version line
//! beneath it, both centered and using the active theme colors.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/info/` (header detector
//! is `/^\s*info/`).

use std::fmt::Write as _;

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

/// Library name shown on the card's first line.
const INFO_NAME: &str = "hiker-mermaid";
/// Version string shown on the card's second (smaller) line.
const INFO_VERSION: &str = "v0.1.0";

/// Margin around the card, px.
const MARGIN: f32 = 16.0;
/// Inner padding between the card edge and its text, px.
const PAD_X: f32 = 28.0;
const PAD_Y: f32 = 18.0;
/// Corner radius of the rounded card, px.
const CORNER_R: f32 = 10.0;
/// Card border / stroke width, px.
const STROKE_W: f32 = 1.5;
/// Vertical gap between the name line and the version line, px.
const LINE_GAP: f32 = 6.0;

/// Validate that `src` is an `info` diagram. Mirrors upstream's `/^\s*info/`:
/// the first non-blank, non-comment line's first token must be `info`. Extra
/// lines (e.g. `showInfo`) are accepted and ignored. Returns `Err` otherwise.
fn parse_info(src: &str) -> Result<(), String> {
    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let first = line.split_whitespace().next().unwrap_or("");
        if first == "info" {
            return Ok(());
        }
        return Err(format!("expected 'info' header, got: {line:?}"));
    }
    Err("empty input / no 'info' header".to_string())
}

/// Render a mermaid `info` diagram to SVG.
pub fn render_info(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    parse_info(src).map_err(MermaidError::Parse)?;

    // Two lines of text: the name at the configured size, the version smaller.
    let name_fs = opts.font_size_px;
    let ver_fs = (opts.font_size_px * 0.75).max(8.0);
    let (name_w, name_h) = text_size(INFO_NAME, name_fs);
    let (ver_w, ver_h) = text_size(INFO_VERSION, ver_fs);

    let text_w = name_w.max(ver_w);
    let text_h = name_h + LINE_GAP + ver_h;

    let card_w = text_w + 2.0 * PAD_X;
    let card_h = text_h + 2.0 * PAD_Y;

    let width = card_w + 2.0 * MARGIN;
    let height = card_h + 2.0 * MARGIN;
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);

    let card_x = MARGIN;
    let card_y = MARGIN;
    let cx = card_x + card_w / 2.0;

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // The rounded card.
    let _ = write!(
        svg,
        "<rect x=\"{card_x:.2}\" y=\"{card_y:.2}\" width=\"{card_w:.2}\" height=\"{card_h:.2}\" \
         rx=\"{CORNER_R}\" ry=\"{CORNER_R}\" fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} \
         stroke-width=\"{STROKE_W}\"/>",
        fill = rgb(opts.node_fill),
        fo = opacity_attr("fill-opacity", opts.node_fill),
        stroke = rgb(opts.node_stroke),
        so = opacity_attr("stroke-opacity", opts.node_stroke),
    );

    let text_color = rgb(opts.text_color);
    let to = opacity_attr("fill-opacity", opts.text_color);

    // Name line (top), baseline centered within its line box.
    let name_y = card_y + PAD_Y + name_h / 2.0;
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{name_y:.2}\" text-anchor=\"middle\" \
         dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{name_fs}\" \
         font-weight=\"bold\" fill=\"{text_color}\"{to}>{name}</text>",
        family = escape(&opts.font_family),
        name = escape(INFO_NAME),
    );

    // Version line (below the name).
    let ver_y = card_y + PAD_Y + name_h + LINE_GAP + ver_h / 2.0;
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{ver_y:.2}\" text-anchor=\"middle\" \
         dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{ver_fs}\" \
         fill=\"{text_color}\"{to}>{ver}</text>",
        family = escape(&opts.font_family),
        ver = escape(INFO_VERSION),
    );

    svg.push_str("</svg>");

    Ok(MermaidRender { svg, width_px: w, height_px: h })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_info_header() {
        assert!(parse_info("info").is_ok());
        assert!(parse_info("  info\n").is_ok());
        assert!(parse_info("info\nshowInfo\n").is_ok());
        // Comments and leading blank lines are skipped.
        assert!(parse_info("\n%% a comment\ninfo\n").is_ok());
    }

    #[test]
    fn rejects_bad_header() {
        assert!(parse_info("graph TD\nA-->B\n").is_err());
        assert!(parse_info("information").is_err());
        assert!(parse_info("").is_err());
    }

    #[test]
    fn bad_header_yields_parse_error() {
        match render_info("pie\n", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn render_well_formed_svg() {
        let r = render_info("info", &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"), "got: {}", &r.svg[..40.min(r.svg.len())]);
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_contains_info_text() {
        let r = render_info("info", &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains(">hiker-mermaid<"), "name; svg={}", r.svg);
        assert!(r.svg.contains(">v0.1.0<"), "version; svg={}", r.svg);
        // Exactly two text lines and one card rect.
        assert_eq!(r.svg.matches("<text").count(), 2);
        assert_eq!(r.svg.matches("<rect").count(), 1);
    }

    #[test]
    fn showinfo_line_is_accepted() {
        let r = render_info("info\nshowInfo\n", &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains(">hiker-mermaid<"));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let a = render_info("info", &opts).expect("a");
        let b = render_info("info", &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
