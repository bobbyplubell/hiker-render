//! `packet` (network packet) diagram — self-contained: parse + grid layout +
//! draw, with NO dagre. Lays bit-fields out on a fixed `bitsPerRow`-column grid.
//!
//! Mermaid packet syntax (the subset we support):
//! ```text
//! packet-beta
//! title TCP Header
//! 0-15: "Source Port"
//! 16-31: "Destination Port"
//! 32-63: "Sequence Number"
//! 96: "Reserved"
//! ```
//! The header line is `packet` or `packet-beta`. `title <text>` may follow the
//! header or appear on its own line. Field lines are one of:
//! - `<start>-<end>: "<label>"` — a multi-bit range (inclusive),
//! - `<start>: "<label>"` — a single bit (`start == end`),
//! - `+<bits>: "<label>"` — `bits` bits starting right after the previous field.
//!
//! When `start` is omitted (`+bits` form) the field begins at `last_bit + 1`.
//! Quotes around the label are tolerated (and optional). Blank lines and `%%`
//! comments are ignored.
//!
//! ## Grid layout (no dagre)
//! Bits are laid out on a grid of `BITS_PER_ROW` (32) columns. A field spanning
//! `[start, end]` occupies the bit columns it covers; if it crosses a row
//! boundary it is split into one segment per row (matching mermaid's
//! `getNextFittingBlock`). Each row is a horizontal band of height `row_h`; each
//! bit is a column of width `cell_w`. We draw, per segment, a filled `<rect>`
//! spanning its columns with the field label centered inside, plus a bit-position
//! ruler (start/end bit indices) above each segment. An optional title sits on
//! top.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/packet/renderer.ts` and
//! `.../parser.ts` (the `populate`/`getNextFittingBlock` row-wrap logic this
//! mirrors), and `.../parser/src/language/packet/packet.langium` (grammar).

use std::fmt::Write as _;

use crate::svgutil::{escape, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

/// Bits per grid row (mermaid's `bitsPerRow` default).
const BITS_PER_ROW: usize = 32;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One parsed packet field: an inclusive bit range `[start, end]` and a label.
/// A single-bit field has `start == end`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Field {
    start: usize,
    end: usize,
    label: String,
}

/// One drawn segment of a field that fits entirely within a single grid row.
/// A field that crosses a row boundary produces multiple segments.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Segment {
    /// Grid row index (0-based).
    row: usize,
    /// Inclusive bit range within the whole packet (always within one row).
    start: usize,
    end: usize,
    label: String,
    /// Index of the originating field (drives the fill palette).
    field_index: usize,
}

/// A parsed packet diagram: optional title and its fields.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Packet {
    title: Option<String>,
    fields: Vec<Field>,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Strip optional surrounding double quotes from a label and trim it.
fn unquote(s: &str) -> String {
    let t = s.trim();
    let t = t.strip_prefix('"').unwrap_or(t);
    let t = t.strip_suffix('"').unwrap_or(t);
    t.to_string()
}

/// Parse mermaid packet source into a [`Packet`]. Returns `Err(message)` when
/// the header is missing/malformed. Malformed *field* lines are skipped
/// leniently; an end-before-start range is also skipped.
fn parse_packet(src: &str) -> Result<Packet, String> {
    let mut title: Option<String> = None;
    let mut fields: Vec<Field> = Vec::new();
    let mut saw_header = false;
    let mut last_bit: isize = -1;

    for raw in src.lines() {
        // Strip `%%` comments and surrounding whitespace.
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if !saw_header {
            // The first non-blank line must be the packet header keyword.
            let kw = line.split_whitespace().next().unwrap_or("");
            if kw != "packet" && kw != "packet-beta" {
                return Err(format!("expected `packet`/`packet-beta` header, found {kw:?}"));
            }
            saw_header = true;
            // A `title ...` may share the header line's trailing text.
            let rest = line[kw.len()..].trim();
            if let Some(t) = rest.strip_prefix("title") {
                let t = t.trim();
                if !t.is_empty() {
                    title = Some(t.to_string());
                }
            }
            continue;
        }

        // `title <text>` on its own line.
        if let Some(rest) = line.strip_prefix("title") {
            // Only treat as a title directive if followed by whitespace/end (so
            // an unquoted label starting with "title" isn't mis-parsed — fields
            // always contain a ':').
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                let t = rest.trim();
                title = if t.is_empty() { None } else { Some(t.to_string()) };
                continue;
            }
        }

        // accTitle/accDescr accessibility directives — ignore.
        if line.starts_with("accTitle") || line.starts_with("accDescr") {
            continue;
        }

        // Field line: `<spec> : <label>`. Split on the FIRST colon.
        let Some((spec, label_part)) = line.split_once(':') else {
            // Not a field line — skip leniently.
            continue;
        };
        let spec = spec.trim();
        let label = unquote(label_part);

        // Resolve the bit range. Three forms: `+bits`, `start-end`, `start`.
        let (start, end): (usize, usize) = if let Some(bits_str) = spec.strip_prefix('+') {
            let Ok(bits) = bits_str.trim().parse::<usize>() else {
                continue;
            };
            if bits == 0 {
                continue; // zero-bit field is invalid
            }
            let s = (last_bit + 1) as usize;
            (s, s + bits - 1)
        } else if let Some((a, b)) = spec.split_once('-') {
            let (Ok(s), Ok(e)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) else {
                continue;
            };
            if e < s {
                continue; // invalid: end before start
            }
            (s, e)
        } else {
            let Ok(s) = spec.parse::<usize>() else {
                continue;
            };
            (s, s)
        };

        last_bit = end as isize;
        fields.push(Field { start, end, label });
    }

    if !saw_header {
        return Err("empty input / missing packet header".to_string());
    }
    Ok(Packet { title, fields })
}

// ---------------------------------------------------------------------------
// Layout (grid wrap)
// ---------------------------------------------------------------------------

/// Split fields into per-row [`Segment`]s on a `BITS_PER_ROW`-column grid. A
/// field that crosses a row boundary is split so each segment lies within one
/// row (mirrors mermaid's `getNextFittingBlock`).
fn segments(fields: &[Field]) -> Vec<Segment> {
    let mut segs = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let mut cur = f.start;
        while cur <= f.end {
            let row = cur / BITS_PER_ROW;
            let row_last_bit = (row + 1) * BITS_PER_ROW - 1;
            let seg_end = f.end.min(row_last_bit);
            segs.push(Segment {
                row,
                start: cur,
                end: seg_end,
                label: f.label.clone(),
                field_index: i,
            });
            cur = seg_end + 1;
        }
    }
    segs
}

/// Palette of field fill colors (cycled by field index). Mermaid's default
/// packet theme uses a single fill; we vary hues so adjacent fields read apart.
const PALETTE: [[u8; 3]; 6] = [
    [236, 236, 255],
    [255, 236, 236],
    [236, 255, 236],
    [255, 250, 224],
    [236, 248, 255],
    [248, 236, 255],
];

// ---------------------------------------------------------------------------
// Draw
// ---------------------------------------------------------------------------

/// Render a parsed packet to a single self-contained SVG document plus its size.
fn draw(packet: &Packet, opts: &MermaidOptions) -> MermaidRender {
    let fs = opts.font_size_px;
    let cell_w = fs * 1.1;
    let row_h = fs * 2.0;
    let ruler_h = fs * 1.1; // band above each row for bit-index numbers
    let title_h = if packet.title.is_some() { fs * 1.8 } else { 0.0 };
    let margin = fs * 0.5;

    let segs = segments(&packet.fields);
    let row_count = segs.iter().map(|s| s.row + 1).max().unwrap_or(0);

    let grid_w = BITS_PER_ROW as f32 * cell_w;
    let width = grid_w + 2.0 * margin;
    // Each row band = ruler + the field rect row.
    let rows_h = row_count as f32 * (ruler_h + row_h);
    let height = title_h + rows_h + 2.0 * margin;

    let stroke = rgb(opts.node_stroke);
    let text_col = rgb(opts.text_color);

    let mut body = String::new();

    // Title centered on top.
    if let Some(title) = &packet.title {
        let ty = margin + title_h * 0.6;
        let _ = write!(
            body,
            "<text x=\"{:.2}\" y=\"{:.2}\" font-family=\"{}\" font-size=\"{:.2}\" \
             font-weight=\"bold\" text-anchor=\"middle\" dominant-baseline=\"middle\" \
             fill=\"{}\">{}</text>",
            width / 2.0,
            ty,
            escape(&opts.font_family),
            fs * 1.1,
            text_col,
            escape(title),
        );
    }

    let grid_top = title_h + margin;

    for seg in &segs {
        let band_top = grid_top + seg.row as f32 * (ruler_h + row_h);
        let rect_top = band_top + ruler_h;
        let col_start = seg.start % BITS_PER_ROW;
        let span = seg.end - seg.start + 1;
        let x = margin + col_start as f32 * cell_w;
        let w = span as f32 * cell_w;

        // Field rectangle, palette by field index.
        let pal = PALETTE[seg.field_index % PALETTE.len()];
        let _ = write!(
            body,
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" \
             fill=\"{}\" stroke=\"{}\" stroke-width=\"1\"/>",
            x,
            rect_top,
            w,
            row_h,
            rgb([pal[0], pal[1], pal[2], 255]),
            stroke,
        );

        // Centered label — omit if the cell is too narrow for even ~2 chars.
        let (lbl_w, _) = text_size(&seg.label, fs);
        if !seg.label.is_empty() && w >= fs * 1.6 {
            let label = fit_label(&seg.label, w, fs, lbl_w);
            if !label.is_empty() {
                let _ = write!(
                    body,
                    "<text x=\"{:.2}\" y=\"{:.2}\" font-family=\"{}\" font-size=\"{:.2}\" \
                     text-anchor=\"middle\" dominant-baseline=\"middle\" fill=\"{}\">{}</text>",
                    x + w / 2.0,
                    rect_top + row_h / 2.0,
                    escape(&opts.font_family),
                    fs * 0.85,
                    text_col,
                    escape(&label),
                );
            }
        }

        // Bit-position ruler above the segment: start index (left) and, for a
        // multi-bit segment, end index (right).
        let ruler_y = band_top + ruler_h * 0.75;
        let ruler_fs = fs * 0.7;
        let single = seg.start == seg.end;
        let _ = write!(
            body,
            "<text x=\"{:.2}\" y=\"{:.2}\" font-family=\"{}\" font-size=\"{:.2}\" \
             text-anchor=\"{}\" fill=\"{}\">{}</text>",
            if single { x + w / 2.0 } else { x },
            ruler_y,
            escape(&opts.font_family),
            ruler_fs,
            if single { "middle" } else { "start" },
            text_col,
            seg.start,
        );
        if !single {
            let _ = write!(
                body,
                "<text x=\"{:.2}\" y=\"{:.2}\" font-family=\"{}\" font-size=\"{:.2}\" \
                 text-anchor=\"end\" fill=\"{}\">{}</text>",
                x + w,
                ruler_y,
                escape(&opts.font_family),
                ruler_fs,
                text_col,
                seg.end,
            );
        }
    }

    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.2}\" height=\"{:.2}\" \
         viewBox=\"0 0 {:.2} {:.2}\">{}</svg>",
        width, height, width, height, body,
    );

    MermaidRender {
        svg,
        width_px: width,
        height_px: height,
    }
}

/// Truncate `label` (adding an ellipsis) so it fits in width `w`; returns empty
/// if not even a single character + ellipsis fits. `lbl_w` is the full label's
/// measured width.
fn fit_label(label: &str, w: f32, fs: f32, lbl_w: f32) -> String {
    // The label text is drawn at 0.85*fs; text_size measured at fs, so scale.
    let avail = w * 0.92;
    let scaled_full = lbl_w * 0.85;
    if scaled_full <= avail {
        return label.to_string();
    }
    // Approximate per-char advance at the draw font size.
    let per_char = (fs * 0.85 * crate::svgutil::CHAR_ADVANCE_EM).max(0.1);
    let max_chars = (avail / per_char).floor() as usize;
    let chars: Vec<char> = label.chars().collect();
    if max_chars == 0 {
        return String::new();
    }
    if max_chars >= chars.len() {
        return label.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let keep: String = chars[..max_chars - 1].iter().collect();
    format!("{keep}…")
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Render a mermaid `packet` diagram to SVG.
pub fn render_packet(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let packet = parse_packet(src).map_err(MermaidError::Parse)?;
    if packet.fields.is_empty() {
        return Err(MermaidError::Empty);
    }
    Ok(draw(&packet, opts))
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

    #[test]
    fn parses_ranges_and_single_bit() {
        let src = "packet-beta\n0-15: \"A\"\n16-31: \"B\"\n32: \"C\"\n";
        let p = parse_packet(src).unwrap();
        assert_eq!(p.fields.len(), 3);
        assert_eq!(p.fields[0], Field { start: 0, end: 15, label: "A".into() });
        assert_eq!(p.fields[1], Field { start: 16, end: 31, label: "B".into() });
        // Single bit → start == end.
        assert_eq!(p.fields[2], Field { start: 32, end: 32, label: "C".into() });
    }

    #[test]
    fn accepts_plain_packet_header_and_title() {
        let src = "packet\ntitle My Header\n0: \"X\"\n";
        let p = parse_packet(src).unwrap();
        assert_eq!(p.title.as_deref(), Some("My Header"));
        assert_eq!(p.fields.len(), 1);
        assert_eq!(p.fields[0].start, 0);
        assert_eq!(p.fields[0].end, 0);
    }

    #[test]
    fn title_on_header_line() {
        let p = parse_packet("packet-beta title Inline\n0-3: \"a\"\n").unwrap();
        assert_eq!(p.title.as_deref(), Some("Inline"));
    }

    #[test]
    fn plus_bits_form_is_contiguous() {
        let src = "packet-beta\n0-7: \"a\"\n+8: \"b\"\n+16: \"c\"\n";
        let p = parse_packet(src).unwrap();
        assert_eq!(p.fields[0], Field { start: 0, end: 7, label: "a".into() });
        assert_eq!(p.fields[1], Field { start: 8, end: 15, label: "b".into() });
        assert_eq!(p.fields[2], Field { start: 16, end: 31, label: "c".into() });
    }

    #[test]
    fn unquoted_labels_tolerated() {
        let p = parse_packet("packet\n0-3: hello\n").unwrap();
        assert_eq!(p.fields[0].label, "hello");
    }

    #[test]
    fn ignores_blank_and_comment_lines() {
        let src = "packet-beta\n\n%% a comment\n0-3: \"a\"\n  %% another\n4-7: \"b\"\n";
        let p = parse_packet(src).unwrap();
        assert_eq!(p.fields.len(), 2);
    }

    #[test]
    fn bad_header_is_err() {
        assert!(matches!(parse_packet("graph TD\nA-->B\n"), Err(_)));
        assert!(matches!(parse_packet(""), Err(_)));
    }

    #[test]
    fn skips_malformed_field_lines() {
        // end < start, non-numeric, zero-bit `+0` are all skipped; valid kept.
        let src = "packet\n10-2: \"bad\"\nfoo-bar: \"x\"\n+0: \"z\"\n0-3: \"ok\"\n";
        let p = parse_packet(src).unwrap();
        assert_eq!(p.fields.len(), 1);
        assert_eq!(p.fields[0].label, "ok");
    }

    #[test]
    fn empty_fields_is_empty_err() {
        // Valid header but no fields → Err(Empty).
        let r = render_packet("packet-beta\n", &opts());
        assert_eq!(r, Err(MermaidError::Empty));
    }

    #[test]
    fn field_crossing_row_boundary_splits_into_two_segments() {
        let fields = vec![Field { start: 30, end: 33, label: "X".into() }];
        let segs = segments(&fields);
        assert_eq!(segs.len(), 2);
        // Row 0: bits 30..=31.
        assert_eq!((segs[0].row, segs[0].start, segs[0].end), (0, 30, 31));
        // Row 1: bits 32..=33.
        assert_eq!((segs[1].row, segs[1].start, segs[1].end), (1, 32, 33));
        // Both keep the same field index/label.
        assert_eq!(segs[0].field_index, segs[1].field_index);
        assert_eq!(segs[1].label, "X");
    }

    #[test]
    fn field_spanning_multiple_rows_splits_per_row() {
        // 0..=95 spans 3 full rows of 32.
        let fields = vec![Field { start: 0, end: 95, label: "big".into() }];
        let segs = segments(&fields);
        assert_eq!(segs.len(), 3);
        assert_eq!((segs[0].row, segs[0].start, segs[0].end), (0, 0, 31));
        assert_eq!((segs[1].row, segs[1].start, segs[1].end), (1, 32, 63));
        assert_eq!((segs[2].row, segs[2].start, segs[2].end), (2, 64, 95));
    }

    #[test]
    fn renders_well_formed_svg_with_rect_per_segment() {
        let src = "packet-beta\n0-15: \"Src\"\n16-31: \"Dst\"\n32-63: \"Seq\"\n";
        let r = render_packet(src, &opts()).unwrap();
        let svg = &r.svg;
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.contains("viewBox="));
        // 3 fields, none crossing a 32-bit boundary → 3 rects.
        assert_eq!(svg.matches("<rect").count(), 3);
        // Labels present.
        assert!(svg.contains(">Src<"));
        assert!(svg.contains(">Dst<"));
        assert!(svg.contains(">Seq<"));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn crossing_field_draws_two_rects() {
        let src = "packet-beta\n0-29: \"a\"\n30-33: \"X\"\n";
        let r = render_packet(src, &opts()).unwrap();
        // Field "a" = 1 segment, field "X" crosses boundary = 2 segments → 3 rects.
        assert_eq!(r.svg.matches("<rect").count(), 3);
    }

    #[test]
    fn bit_ruler_numbers_present() {
        let src = "packet-beta\n0-15: \"A\"\n16-31: \"B\"\n";
        let r = render_packet(src, &opts()).unwrap();
        // Ruler shows start/end indices: 0, 15, 16, 31 should all appear.
        for n in ["0", "15", "16", "31"] {
            assert!(r.svg.contains(&format!(">{n}<")), "missing ruler index {n}");
        }
    }

    #[test]
    fn xml_escapes_labels_and_title() {
        let src = "packet-beta\ntitle A & B\n0-7: \"x < y\"\n";
        let r = render_packet(src, &opts()).unwrap();
        assert!(r.svg.contains("A &amp; B"));
        assert!(r.svg.contains("x &lt; y"));
        // No raw unescaped ampersand-text/angle leaked into content.
        assert!(!r.svg.contains("A & B"));
    }

    #[test]
    fn deterministic_output() {
        let src = "packet-beta\ntitle T\n0-15: \"A\"\n16-31: \"B\"\n32-63: \"C\"\n";
        let a = render_packet(src, &opts()).unwrap();
        let b = render_packet(src, &opts()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn narrow_single_bit_omits_or_truncates_label() {
        // A long label in a single 1-bit cell should be omitted (too narrow),
        // but the rect and ruler index must still be drawn.
        let src = "packet-beta\n0: \"VeryLongLabelThatCannotFit\"\n";
        let r = render_packet(src, &opts()).unwrap();
        assert_eq!(r.svg.matches("<rect").count(), 1);
        assert!(r.svg.contains(">0<"));
    }
}
