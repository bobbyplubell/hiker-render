//! Cynefin framework diagram (self-contained: parse + draw, no graph layout).
//!
//! Mermaid header is `cynefin-beta` (a bare keyword, or the colon form
//! `cynefin-beta:`). The Cynefin framework is a 2×2 grid of four sense-making
//! domains plus a central "Confusion" (a.k.a. Disorder) zone. The fixed
//! arrangement (matching the upstream renderer's `getDomainLayouts`) is:
//!
//! ```text
//!   +-----------------+-----------------+
//!   |     Complex     |   Complicated   |
//!   +--------+--------+--------+--------+
//!   |        |   ( Confusion )         |
//!   +--------+--------+--------+--------+
//!   |     Chaotic     |      Clear      |
//!   +-----------------+-----------------+
//! ```
//!
//! i.e. top-left = **Complex**, top-right = **Complicated**, bottom-left =
//! **Chaotic**, bottom-right = **Clear**, center = **Confusion**.
//!
//! ## Syntax (from `cynefin.langium`)
//! ```text
//! cynefin-beta
//!     title Decision making
//!     complex
//!         "Item one"
//!         "Item two"
//!     clear
//!         "Stable item"
//!     complex --> complicated: Pattern found
//! ```
//! A `title <text>` line; then **domain blocks** introduced by a bare domain
//! keyword (`complex` | `complicated` | `clear` | `chaotic` | `confusion`),
//! each followed by zero or more quoted-string **items** (one per line); and
//! optional **transitions** `<domain> --> <domain>[: <label>]` drawn as curved
//! arrows between domain centers (self-loops are skipped). Blank lines and `%%`
//! comments are ignored.
//!
//! Layout/draw: a square split into four quadrant cells (each faintly tinted
//! from `series_palette` / themed colors) with the domain NAME centered (bold)
//! in each cell, a central ellipse for Confusion, and each item drawn as a small
//! rounded badge stacked vertically within its domain. Title centered on top.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/cynefin/` for the
//! upstream parser/renderer this mirrors.

use std::fmt::Write as _;

use crate::svgutil::{escape, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// The five Cynefin domains, in their canonical order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Domain {
    Complex,
    Complicated,
    Chaotic,
    Clear,
    Confusion,
}

impl Domain {
    /// Parse a bare domain keyword (lowercase, as in the grammar).
    fn parse(s: &str) -> Option<Domain> {
        match s {
            "complex" => Some(Domain::Complex),
            "complicated" => Some(Domain::Complicated),
            "chaotic" => Some(Domain::Chaotic),
            "clear" => Some(Domain::Clear),
            "confusion" => Some(Domain::Confusion),
            _ => None,
        }
    }

    /// Display name (capitalized) shown as the cell label.
    fn label(self) -> &'static str {
        match self {
            Domain::Complex => "Complex",
            Domain::Complicated => "Complicated",
            Domain::Chaotic => "Chaotic",
            Domain::Clear => "Clear",
            Domain::Confusion => "Confusion",
        }
    }

    /// The four quadrant domains, in draw order.
    const QUADRANTS: [Domain; 4] = [
        Domain::Complex,
        Domain::Complicated,
        Domain::Chaotic,
        Domain::Clear,
    ];

    /// Stable index for cycling the series palette / addressing arrays.
    fn index(self) -> usize {
        match self {
            Domain::Complex => 0,
            Domain::Complicated => 1,
            Domain::Chaotic => 2,
            Domain::Clear => 3,
            Domain::Confusion => 4,
        }
    }
}

/// A transition arrow between two domains, with an optional label.
#[derive(Clone, Debug, PartialEq)]
struct Transition {
    from: Domain,
    to: Domain,
    label: Option<String>,
}

/// A parsed cynefin diagram: a title, per-domain item lists, and transitions.
#[derive(Clone, Debug, Default, PartialEq)]
struct Cynefin {
    title: Option<String>,
    /// Items per domain, indexed by [`Domain::index`].
    items: [Vec<String>; 5],
    /// Domains that were explicitly declared (even if empty).
    declared: [bool; 5],
    transitions: Vec<Transition>,
}

impl Cynefin {
    fn items(&self, d: Domain) -> &[String] {
        &self.items[d.index()]
    }

    /// True when nothing renderable was declared.
    fn is_empty(&self) -> bool {
        self.declared.iter().all(|d| !d) && self.transitions.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Strip one layer of surrounding quotes (`"…"` or `'…'`) and unescape `\"`/`\\`.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        let inner = &s[1..s.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut esc = false;
        for c in inner.chars() {
            if esc {
                out.push(c);
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else {
                out.push(c);
            }
        }
        out
    } else {
        s.to_string()
    }
}

/// Parse cynefin source into a [`Cynefin`] model.
///
/// Returns `Err(String)` on a missing/invalid header. The header must be
/// `cynefin-beta` (optionally with a trailing `:`).
fn parse(src: &str) -> Result<Cynefin, String> {
    let mut lines = src.lines().map(strip_comment).filter(|l| !l.trim().is_empty());

    // Header.
    let header = lines.next().ok_or_else(|| "empty input".to_string())?;
    let header = header.trim().trim_end_matches(':').trim();
    if header != "cynefin-beta" && header != "cynefin" {
        return Err(format!("expected `cynefin-beta` header, found {header:?}"));
    }

    let mut model = Cynefin::default();
    // The domain a subsequent quoted item belongs to.
    let mut current: Option<Domain> = None;

    for raw in lines {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        // `title <text>`
        if let Some(rest) = strip_keyword(line, "title") {
            model.title = Some(rest.trim().to_string());
            continue;
        }

        // Transition: `<domain> --> <domain>[: <label>]`
        if let Some((lhs, rhs)) = line.split_once("-->") {
            if let Some(from) = Domain::parse(lhs.trim()) {
                let (to_str, label) = match rhs.split_once(':') {
                    Some((t, l)) => (t.trim(), Some(unquote(l))),
                    None => (rhs.trim(), None),
                };
                if let Some(to) = Domain::parse(to_str) {
                    // Skip self-loops (they're not meaningful), like upstream.
                    if from != to {
                        model.transitions.push(Transition { from, to, label });
                    }
                    continue;
                }
            }
            // Not a valid transition — fall through to other handling.
        }

        // A bare domain keyword opens a domain block.
        if let Some(d) = Domain::parse(line) {
            model.declared[d.index()] = true;
            current = Some(d);
            continue;
        }

        // Otherwise: a quoted (or bare) item for the current domain.
        if let Some(d) = current {
            model.declared[d.index()] = true;
            model.items[d.index()].push(unquote(line));
        }
        // Items before any domain block are ignored.
    }

    Ok(model)
}

/// Remove a trailing `%%` comment from a line.
fn strip_comment(line: &str) -> &str {
    line.split("%%").next().unwrap_or("")
}

/// If `line` starts with `kw` followed by whitespace (or is exactly `kw`),
/// return the remainder; else `None`.
fn strip_keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    if rest.is_empty() {
        Some(rest)
    } else if rest.starts_with(char::is_whitespace) {
        Some(rest)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Draw
// ---------------------------------------------------------------------------

/// Blend a straight-RGBA color toward white by `t` (0 = unchanged, 1 = white).
fn tint(c: [u8; 4], t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    let mix = |v: u8| (v as f32 + (255.0 - v as f32) * t).round().clamp(0.0, 255.0) as u8;
    [mix(c[0]), mix(c[1]), mix(c[2]), 255]
}

/// The faint background fill for a domain cell, derived from the series palette
/// (cycled by domain index) or, as a fallback, from the node fill.
fn domain_fill(d: Domain, opts: &MermaidOptions) -> [u8; 4] {
    let base = if opts.series_palette.is_empty() {
        opts.node_fill
    } else {
        opts.series_palette[d.index() % opts.series_palette.len()]
    };
    // Lighten so labels/items stay legible on top.
    tint(base, 0.55)
}

/// Geometry of a quadrant cell or the confusion zone.
struct Cell {
    /// Cell rect.
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// Cell center.
    cx: f32,
    cy: f32,
}

/// Render a cynefin diagram to SVG.
pub fn render_cynefin(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let model = parse(src).map_err(MermaidError::Parse)?;
    if model.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;

    // Square plot. Padding leaves room for the title on top.
    let plot = 520.0_f32;
    let pad = 24.0_f32;
    let title_h = if model.title.is_some() { fs * 1.6 + 8.0 } else { 0.0 };

    let ox = pad; // plot origin x
    let oy = pad + title_h; // plot origin y
    let width = plot + pad * 2.0;
    let height = plot + pad * 2.0 + title_h;

    let hw = plot / 2.0;
    let hh = plot / 2.0;

    // Quadrant + confusion geometry (in plot-local coords, mirrors upstream).
    let cells = |d: Domain| -> Cell {
        match d {
            Domain::Complex => Cell { x: 0.0, y: 0.0, w: hw, h: hh, cx: hw / 2.0, cy: hh / 2.0 },
            Domain::Complicated => {
                Cell { x: hw, y: 0.0, w: hw, h: hh, cx: hw + hw / 2.0, cy: hh / 2.0 }
            }
            Domain::Chaotic => {
                Cell { x: 0.0, y: hh, w: hw, h: hh, cx: hw / 2.0, cy: hh + hh / 2.0 }
            }
            Domain::Clear => {
                Cell { x: hw, y: hh, w: hw, h: hh, cx: hw + hw / 2.0, cy: hh + hh / 2.0 }
            }
            Domain::Confusion => Cell {
                x: hw * 0.7,
                y: hh * 0.7,
                w: hw * 0.6,
                h: hh * 0.6,
                cx: hw,
                cy: hh,
            },
        }
    };

    let mut s = String::with_capacity(2048);
    let _ = write!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" \
         height=\"{height:.0}\" viewBox=\"0 0 {width:.0} {height:.0}\">",
    );

    // Open a translate group so all cell coords are plot-local.
    let _ = write!(s, "<g transform=\"translate({ox:.1},{oy:.1})\">");

    let stroke = rgb(opts.node_stroke);
    let edge = rgb(opts.edge_stroke);
    let text = rgb(opts.text_color);

    // 1. Quadrant background rects.
    for d in Domain::QUADRANTS {
        let c = cells(d);
        let fill = rgb(domain_fill(d, opts));
        let _ = write!(
            s,
            "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" \
             fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1\"/>",
            c.x, c.y, c.w, c.h,
        );
    }

    // 2. Confusion ellipse (center overlay).
    {
        let c = cells(Domain::Confusion);
        let rx = plot * 0.15;
        let ry = plot * 0.15;
        let fill = rgb(domain_fill(Domain::Confusion, opts));
        let _ = write!(
            s,
            "<ellipse cx=\"{:.1}\" cy=\"{:.1}\" rx=\"{rx:.1}\" ry=\"{ry:.1}\" \
             fill=\"{fill}\" fill-opacity=\"0.85\" stroke=\"{stroke}\" stroke-width=\"1\"/>",
            c.cx, c.cy,
        );
    }

    // 3. Domain name labels (bold, centered in each cell).
    let label_fs = fs * 1.1;
    for d in Domain::QUADRANTS {
        let c = cells(d);
        // Anchor the label near the top of the cell so items stack below it.
        let ly = c.y + label_fs * 1.4;
        let _ = write!(
            s,
            "<text x=\"{:.1}\" y=\"{ly:.1}\" text-anchor=\"middle\" \
             font-family=\"{ff}\" font-size=\"{label_fs:.1}\" font-weight=\"bold\" \
             fill=\"{text}\">{}</text>",
            c.cx,
            escape(d.label()),
            ff = escape(&opts.font_family),
        );
    }
    // Confusion label (centered in the ellipse).
    {
        let c = cells(Domain::Confusion);
        let _ = write!(
            s,
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" \
             font-family=\"{ff}\" font-size=\"{label_fs:.1}\" font-weight=\"bold\" \
             fill=\"{text}\">{}</text>",
            c.cx,
            c.cy - label_fs * 0.2,
            escape(Domain::Confusion.label()),
            ff = escape(&opts.font_family),
        );
    }

    // 4. Items as rounded badges, stacked vertically within each domain.
    let item_h = fs * 1.5;
    let item_gap = 4.0;
    let pad_x = 10.0;
    for d in [
        Domain::Complex,
        Domain::Complicated,
        Domain::Chaotic,
        Domain::Clear,
        Domain::Confusion,
    ] {
        let items = model.items(d);
        if items.is_empty() {
            continue;
        }
        let c = cells(d);
        // Stack below the domain label.
        let start_y = if d == Domain::Confusion {
            c.cy + label_fs * 0.8
        } else {
            c.y + label_fs * 2.4
        };
        let fill = rgb(domain_fill(d, opts));
        for (i, item) in items.iter().enumerate() {
            let (tw, _) = text_size(item, fs);
            let bw = tw + pad_x * 2.0;
            let bx = c.cx - bw / 2.0;
            let by = start_y + i as f32 * (item_h + item_gap);
            let _ = write!(
                s,
                "<rect x=\"{bx:.1}\" y=\"{by:.1}\" width=\"{bw:.1}\" height=\"{item_h:.1}\" \
                 rx=\"4\" ry=\"4\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1\"/>",
            );
            let _ = write!(
                s,
                "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" \
                 font-family=\"{ff}\" font-size=\"{fs:.1}\" fill=\"{text}\">{}</text>",
                c.cx,
                by + item_h / 2.0 + fs * 0.35,
                escape(item),
                ff = escape(&opts.font_family),
            );
        }
    }

    // 5. Transition arrows between domain centers (curved), with optional labels.
    if !model.transitions.is_empty() {
        // Arrowhead marker.
        let _ = write!(
            s,
            "<defs><marker id=\"cynefin-arrow\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" \
             markerWidth=\"6\" markerHeight=\"6\" orient=\"auto-start-reverse\">\
             <path d=\"M 0 0 L 10 5 L 0 10 z\" fill=\"{edge}\"/></marker></defs>",
        );
        for t in &model.transitions {
            let a = cells(t.from);
            let b = cells(t.to);
            let (x1, y1, x2, y2) = (a.cx, a.cy, b.cx, b.cy);
            let dx = x2 - x1;
            let dy = y2 - y1;
            let len = (dx * dx + dy * dy).sqrt().max(1e-3);
            let off = len * 0.15;
            let (mx, my) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
            let (nx, ny) = (-dy / len, dx / len);
            let (cpx, cpy) = (mx + nx * off, my + ny * off);
            let _ = write!(
                s,
                "<path d=\"M{x1:.1},{y1:.1} Q{cpx:.1},{cpy:.1} {x2:.1},{y2:.1}\" \
                 fill=\"none\" stroke=\"{edge}\" stroke-width=\"1.5\" \
                 marker-end=\"url(#cynefin-arrow)\"/>",
            );
            if let Some(label) = &t.label {
                if !label.is_empty() {
                    let _ = write!(
                        s,
                        "<text x=\"{cpx:.1}\" y=\"{:.1}\" text-anchor=\"middle\" \
                         font-family=\"{ff}\" font-size=\"{fs:.1}\" fill=\"{text}\">{}</text>",
                        cpy - 6.0,
                        escape(label),
                        ff = escape(&opts.font_family),
                    );
                }
            }
        }
    }

    let _ = write!(s, "</g>"); // close translate group

    // 6. Title (centered on top, in absolute coords).
    if let Some(title) = &model.title {
        let _ = write!(
            s,
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" \
             font-family=\"{ff}\" font-size=\"{tfs:.1}\" font-weight=\"bold\" \
             fill=\"{text}\">{}</text>",
            width / 2.0,
            pad + fs,
            escape(title),
            ff = escape(&opts.font_family),
            tfs = fs * 1.3,
        );
    }

    s.push_str("</svg>");

    Ok(MermaidRender { svg: s, width_px: width, height_px: height })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"cynefin-beta
    title Decision Making
    complex
        "Climate change"
        "Pandemic response"
    complicated
        "Engineering a bridge"
    chaotic
        "Crisis"
    clear
        "Known procedures"
    confusion
        "Unclear"
    complex --> complicated: Pattern found
"#;

    #[test]
    fn parses_title_items_and_domain_mapping() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(m.title.as_deref(), Some("Decision Making"));
        assert_eq!(m.items(Domain::Complex), &["Climate change", "Pandemic response"]);
        assert_eq!(m.items(Domain::Complicated), &["Engineering a bridge"]);
        assert_eq!(m.items(Domain::Chaotic), &["Crisis"]);
        assert_eq!(m.items(Domain::Clear), &["Known procedures"]);
        assert_eq!(m.items(Domain::Confusion), &["Unclear"]);
    }

    #[test]
    fn parses_transitions_and_skips_self_loops() {
        let src = "cynefin-beta\ncomplex\nchaotic --> chaotic\ncomplex --> chaotic: move\n";
        let m = parse(src).unwrap();
        assert_eq!(m.transitions.len(), 1);
        assert_eq!(m.transitions[0].from, Domain::Complex);
        assert_eq!(m.transitions[0].to, Domain::Chaotic);
        assert_eq!(m.transitions[0].label.as_deref(), Some("move"));
    }

    #[test]
    fn accepts_colon_header_form() {
        let m = parse("cynefin-beta:\nclear\n  \"x\"\n").unwrap();
        assert_eq!(m.items(Domain::Clear), &["x"]);
    }

    #[test]
    fn bad_header_is_parse_error() {
        assert!(parse("flowchart TD\nA-->B\n").is_err());
        assert!(matches!(
            render_cynefin("nope", &MermaidOptions::default()),
            Err(MermaidError::Parse(_))
        ));
    }

    #[test]
    fn empty_diagram_is_empty_error() {
        // Header only, no domains/items/transitions.
        let r = render_cynefin("cynefin-beta\n", &MermaidOptions::default());
        assert_eq!(r, Err(MermaidError::Empty));
    }

    #[test]
    fn renders_well_formed_svg_with_cells_labels_center_and_markers() {
        let r = render_cynefin(SAMPLE, &MermaidOptions::default()).unwrap();
        let svg = &r.svg;
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);

        // Four quadrant labels + center confusion zone + title.
        for name in ["Complex", "Complicated", "Chaotic", "Clear", "Confusion"] {
            assert!(svg.contains(name), "missing domain label {name}");
        }
        assert!(svg.contains("<ellipse"), "missing confusion center zone");
        assert!(svg.contains("Decision Making"), "missing title");

        // Four quadrant background cells (rects). At least 4.
        assert!(svg.matches("<rect").count() >= 4);

        // One badge per item: count item labels appearing as text.
        for item in [
            "Climate change",
            "Pandemic response",
            "Engineering a bridge",
            "Crisis",
            "Known procedures",
            "Unclear",
        ] {
            assert!(svg.contains(item), "missing item badge {item}");
        }

        // Transition arrow marker + curve.
        assert!(svg.contains("cynefin-arrow"));
        assert!(svg.contains("Pattern found"));
    }

    #[test]
    fn xml_escapes_text() {
        let src = "cynefin-beta\ntitle A & B\ncomplex\n  \"x < y > z\"\n";
        let r = render_cynefin(src, &MermaidOptions::default()).unwrap();
        assert!(r.svg.contains("A &amp; B"));
        assert!(r.svg.contains("x &lt; y &gt; z"));
        assert!(!r.svg.contains("x < y"));
    }

    #[test]
    fn deterministic() {
        let a = render_cynefin(SAMPLE, &MermaidOptions::default()).unwrap();
        let b = render_cynefin(SAMPLE, &MermaidOptions::default()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn unquote_handles_quotes_and_escapes() {
        assert_eq!(unquote("\"hi\""), "hi");
        assert_eq!(unquote("'hi'"), "hi");
        assert_eq!(unquote("bare"), "bare");
        assert_eq!(unquote("\"a\\\"b\""), "a\"b");
    }
}
