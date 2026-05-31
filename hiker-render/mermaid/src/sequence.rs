//! Sequence diagram (self-contained: parse + layout + draw, no dagre).
//!
//! Mermaid sequence syntax (subset supported here):
//! ```text
//! sequenceDiagram
//!     participant A
//!     participant B as Bob
//!     actor C
//!     A->>B: Hello Bob
//!     B-->>A: Hi Alice
//!     A-)B: async
//!     A->>A: think
//! ```
//! Self-layout: participants become **columns** (x positions) with vertical
//! dashed lifelines; messages become **horizontal arrows** at increasing y.
//!
//! ## Supported
//! - Header `sequenceDiagram`.
//! - `participant <id>` / `participant <id> as <Label>` / `actor <id>`.
//! - Auto-created participants (first appearance order) for ids used in a
//!   message but never declared.
//! - Messages with arrow tokens between two ids and an optional `: text`:
//!   `->>`/`-->>` (filled arrowhead, solid/**dashed**), `->`/`-->` (open V),
//!   `-)`/`--)` (async open V), `-x`/`--x` (cross end). The `--` variants are
//!   dashed.
//! - Self-messages (`A->>A: text`) draw a small loop to the right of A's
//!   lifeline.
//!
//! ## Skipped in v1 (intentionally)
//! Notes (`Note over/right of`), `loop`/`alt`/`opt` blocks, activations
//! (`+`/`-`), and `autonumber` are ignored — `Note`/`loop`/`alt`/`opt`/`end`
//! lines are silently skipped so a diagram using them still renders its
//! participants + plain messages.

use std::fmt::Write as _;

use crate::{MermaidError, MermaidOptions, MermaidRender};

// ----------------------------------------------------------------------------
// Model
// ----------------------------------------------------------------------------

/// A participant column: its id (used for matching in messages) and the label
/// drawn in its box (defaults to the id when no `as` alias is given).
#[derive(Clone, Debug, PartialEq)]
struct Participant {
    id: String,
    label: String,
}

/// The visual style of an arrow's line + head, decoded from the arrow token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArrowStyle {
    /// `->>` / `-->>` — solid filled triangle head.
    Filled,
    /// `->` / `-->` — open V (line, no fill).
    Open,
    /// `-)` / `--)` — async, open V (same draw as `Open` here).
    Async,
    /// `-x` / `--x` — a small cross at the end.
    Cross,
}

/// One message between two participants.
#[derive(Clone, Debug, PartialEq)]
struct Message {
    from: String,
    to: String,
    text: String,
    style: ArrowStyle,
    /// Dashed line (the `--` arrow variants).
    dashed: bool,
}

/// A fully parsed sequence diagram (participants in column order + ordered
/// messages).
#[derive(Clone, Debug, PartialEq)]
struct SequenceDiagram {
    participants: Vec<Participant>,
    messages: Vec<Message>,
}

// ----------------------------------------------------------------------------
// Parse
// ----------------------------------------------------------------------------

/// Lines that introduce block constructs we don't render in v1; skipped so the
/// surrounding participants/messages still render.
const SKIP_PREFIXES: [&str; 8] =
    ["note ", "loop ", "alt ", "opt ", "par ", "rect ", "critical ", "break "];

/// Parse `sequenceDiagram` source into a [`SequenceDiagram`].
///
/// Participants are collected in declaration order; any id referenced by a
/// message but not declared is appended in first-appearance order. Returns the
/// raw parse error string on malformed input.
fn parse_sequence(src: &str) -> Result<SequenceDiagram, String> {
    let mut participants: Vec<Participant> = Vec::new();
    let mut messages: Vec<Message> = Vec::new();
    let mut seen_header = false;

    // Track explicitly-declared ids so auto-created ones don't duplicate.
    let mut have: std::collections::HashSet<String> = std::collections::HashSet::new();

    for raw in src.lines() {
        // Strip `%%` comments, trailing `;`, and surrounding whitespace.
        let line = raw.split("%%").next().unwrap_or("").trim().trim_end_matches(';').trim();
        if line.is_empty() {
            continue;
        }

        let lower = line.to_ascii_lowercase();

        if !seen_header {
            if lower == "sequencediagram" || lower.starts_with("sequencediagram ") {
                seen_header = true;
                continue;
            }
            return Err(format!("expected 'sequenceDiagram' header, found {line:?}"));
        }

        // Block constructs / directives we skip in v1.
        if lower == "end" || lower == "autonumber" || lower.starts_with("autonumber ") {
            continue;
        }
        if SKIP_PREFIXES.iter().any(|p| lower.starts_with(p)) {
            continue;
        }

        // `participant <id> [as <label>]` or `actor <id> [as <label>]`.
        if let Some(rest) = strip_keyword(line, "participant").or_else(|| strip_keyword(line, "actor")) {
            let (id, label) = parse_participant_decl(rest)?;
            declare(&mut participants, &mut have, &id, label);
            continue;
        }

        // Otherwise it must be a message line.
        let msg = parse_message(line)?;
        // Auto-create endpoints in first-appearance order.
        for end in [&msg.from, &msg.to] {
            if !have.contains(end) {
                declare(&mut participants, &mut have, end, None);
            }
        }
        messages.push(msg);
    }

    if !seen_header {
        return Err("missing 'sequenceDiagram' header".to_string());
    }

    Ok(SequenceDiagram { participants, messages })
}

/// Add a participant (id + optional explicit label) once, recording it in the
/// `have` set. A later `as` alias for an already-seen id updates its label.
fn declare(
    participants: &mut Vec<Participant>,
    have: &mut std::collections::HashSet<String>,
    id: &str,
    label: Option<String>,
) {
    if let Some(p) = participants.iter_mut().find(|p| p.id == id) {
        if let Some(l) = label {
            p.label = l;
        }
        return;
    }
    have.insert(id.to_string());
    participants.push(Participant {
        id: id.to_string(),
        label: label.unwrap_or_else(|| id.to_string()),
    });
}

/// If `line` starts with the whole word `kw` (followed by whitespace), return
/// the trimmed remainder; else `None`.
fn strip_keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    let mut chars = rest.chars();
    match chars.next() {
        Some(c) if c.is_whitespace() => Some(rest.trim()),
        _ => None,
    }
}

/// Parse the part after `participant`/`actor`: `<id>` or `<id> as <label>`.
fn parse_participant_decl(rest: &str) -> Result<(String, Option<String>), String> {
    // Split on ` as ` (case-insensitive), once.
    let lower = rest.to_ascii_lowercase();
    if let Some(pos) = lower.find(" as ") {
        let id = rest[..pos].trim();
        let label = rest[pos + 4..].trim();
        if id.is_empty() {
            return Err(format!("participant declaration missing id: {rest:?}"));
        }
        if label.is_empty() {
            return Err(format!("participant alias missing label: {rest:?}"));
        }
        Ok((id.to_string(), Some(label.to_string())))
    } else {
        let id = rest.trim();
        // A bare id must be a single token.
        if id.is_empty() || id.split_whitespace().count() != 1 {
            return Err(format!("invalid participant declaration: {rest:?}"));
        }
        Ok((id.to_string(), None))
    }
}

/// Parse one message line `A<arrow>B[: text]`. Lenient about spaces around the
/// arrow and the colon.
fn parse_message(line: &str) -> Result<Message, String> {
    // Split off the optional `: text`.
    let (link_part, text) = match line.split_once(':') {
        Some((l, t)) => (l.trim(), t.trim().to_string()),
        None => (line.trim(), String::new()),
    };

    let (from, arrow, to) = split_arrow(link_part)
        .ok_or_else(|| format!("not a valid message (no arrow): {line:?}"))?;

    if from.is_empty() || to.is_empty() {
        return Err(format!("message missing a participant id: {line:?}"));
    }

    let (style, dashed) = decode_arrow(arrow)
        .ok_or_else(|| format!("unrecognized arrow token {arrow:?} in {line:?}"))?;

    Ok(Message { from, to, text, style, dashed })
}

/// Find the arrow token inside `link` and return `(from_id, arrow, to_id)`.
///
/// Scans for the longest known arrow token (so `-->>` wins over `->`). Ids are
/// whatever sits on each side (trimmed). Returns `None` when no token matches.
fn split_arrow(link: &str) -> Option<(String, &str, String)> {
    // Arrow tokens, longest first so we match greedily.
    const ARROWS: [&str; 10] = ["-->>", "--x", "--)", "-->", "->>", "-x", "-)", "->", "--", ">>"];
    // Search left-to-right for the earliest position where any token matches;
    // among ties at a position, prefer the longest token.
    let mut best: Option<(usize, &str)> = None;
    for (i, _) in link.char_indices() {
        for tok in ARROWS {
            if link[i..].starts_with(tok) {
                // Prefer the earliest start; at the same start, the longest.
                let better = match best {
                    None => true,
                    Some((bi, bt)) => i < bi || (i == bi && tok.len() > bt.len()),
                };
                if better {
                    best = Some((i, tok));
                }
            }
        }
        if best.is_some() {
            // Earliest position found; lock it in (longest already chosen).
            let (bi, bt) = best.unwrap();
            let from = link[..bi].trim().to_string();
            let to = link[bi + bt.len()..].trim().to_string();
            return Some((from, bt, to));
        }
    }
    None
}

/// Decode an arrow token into `(style, dashed)`. `--` prefix ⇒ dashed.
fn decode_arrow(arrow: &str) -> Option<(ArrowStyle, bool)> {
    let style = match arrow {
        "->>" | "-->>" => ArrowStyle::Filled,
        "->" | "-->" | "--" => ArrowStyle::Open,
        "-)" | "--)" => ArrowStyle::Async,
        "-x" | "--x" => ArrowStyle::Cross,
        _ => return None,
    };
    let dashed = arrow.starts_with("--");
    Some((style, dashed))
}

// ----------------------------------------------------------------------------
// Layout constants
// ----------------------------------------------------------------------------

/// Per-char advance fraction of font size (kept in sync with `measure`).
const CHAR_ADVANCE_EM: f32 = 0.6;
/// Outer canvas margin, px.
const MARGIN: f32 = 16.0;
/// Participant box vertical padding (each side), px.
const BOX_PAD_Y: f32 = 8.0;
/// Participant box horizontal padding (each side), px.
const BOX_PAD_X: f32 = 12.0;
/// Minimum participant box width, px.
const MIN_BOX_W: f32 = 40.0;
/// Horizontal gap between adjacent participant boxes, px.
const COL_GAP: f32 = 40.0;
/// Vertical gap between consecutive message rows, px.
const MESSAGE_GAP: f32 = 40.0;
/// Gap between the participant boxes and the first message row, px.
const TOP_GAP: f32 = 30.0;
/// Width of a self-message loop, px.
const SELF_LOOP_W: f32 = 36.0;
/// Height of a self-message loop, px.
const SELF_LOOP_H: f32 = 28.0;
/// Arrowhead length / half-width, px.
const ARROW_LEN: f32 = 9.0;
const ARROW_HALF: f32 = 4.0;
/// Stroke width, px.
const STROKE_W: f32 = 1.5;

/// Heuristic label width (font-free), matching the flowchart `measure` rule.
fn label_width(label: &str, font_size: f32) -> f32 {
    label.chars().count() as f32 * font_size * CHAR_ADVANCE_EM
}

/// Participant box width for a label (with padding + minimum).
fn box_width(label: &str, font_size: f32) -> f32 {
    (label_width(label, font_size) + 2.0 * BOX_PAD_X).max(MIN_BOX_W)
}

// ----------------------------------------------------------------------------
// Render
// ----------------------------------------------------------------------------

/// Render mermaid sequence-diagram source to an SVG document.
pub fn render_sequence(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diagram = parse_sequence(src).map_err(MermaidError::Parse)?;
    if diagram.participants.is_empty() {
        return Err(MermaidError::Empty);
    }
    Ok(draw_sequence(&diagram, opts))
}

/// Lay out + draw the parsed diagram into an SVG document.
fn draw_sequence(diagram: &SequenceDiagram, opts: &MermaidOptions) -> MermaidRender {
    let fs = opts.font_size_px;
    let box_h = fs * 1.2 + 2.0 * BOX_PAD_Y;

    // --- Columns: each participant gets a center x. Uniform column width =
    // widest box, so lifelines are evenly spaced. ---
    let widths: Vec<f32> = diagram.participants.iter().map(|p| box_width(&p.label, fs)).collect();
    let col_w = widths.iter().cloned().fold(MIN_BOX_W, f32::max);

    let n = diagram.participants.len();
    let mut centers: Vec<f32> = Vec::with_capacity(n);
    for i in 0..n {
        let cx = MARGIN + col_w / 2.0 + i as f32 * (col_w + COL_GAP);
        centers.push(cx);
    }

    // Index id → column.
    let col_of = |id: &str| -> Option<usize> {
        diagram.participants.iter().position(|p| p.id == id)
    };

    // --- Vertical extents ---
    let box_top = MARGIN;
    let box_bottom = box_top + box_h;
    let first_row_y = box_bottom + TOP_GAP;

    // Each message gets a y; self-messages take a bit more vertical room.
    let mut row_y: Vec<f32> = Vec::with_capacity(diagram.messages.len());
    let mut y = first_row_y;
    for m in &diagram.messages {
        row_y.push(y);
        if m.from == m.to {
            y += SELF_LOOP_H + MESSAGE_GAP * 0.5;
        } else {
            y += MESSAGE_GAP;
        }
    }
    let messages_bottom = row_y.last().copied().unwrap_or(first_row_y) + MESSAGE_GAP * 0.5;

    // --- Canvas size ---
    let last_cx = centers.last().copied().unwrap_or(MARGIN + col_w / 2.0);
    // Account for self-loops poking out to the right of the last column.
    let right_extent = last_cx + col_w / 2.0 + SELF_LOOP_W + MARGIN;
    let width = right_extent.max(MARGIN * 2.0 + col_w);
    let height = messages_bottom + MARGIN;

    let mut svg = String::new();
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Shared filled-arrowhead marker.
    emit_defs(&mut svg, opts);

    // --- Lifelines (dashed verticals) ---
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    for &cx in &centers {
        let _ = write!(
            svg,
            "<line x1=\"{cx:.2}\" y1=\"{y1:.2}\" x2=\"{cx:.2}\" y2=\"{y2:.2}\" \
             stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\" stroke-dasharray=\"3 3\"/>",
            y1 = box_bottom,
            y2 = messages_bottom,
        );
    }

    // --- Participant boxes (rect + centered label) ---
    let (fill, fo) = fill_attrs(opts.node_fill);
    let (nstroke, nso) = stroke_attrs(opts.node_stroke);
    for (i, p) in diagram.participants.iter().enumerate() {
        let cx = centers[i];
        let bw = widths[i].max(MIN_BOX_W);
        let x = cx - bw / 2.0;
        let _ = write!(
            svg,
            "<rect x=\"{x:.2}\" y=\"{box_top:.2}\" width=\"{bw:.2}\" height=\"{box_h:.2}\" \
             rx=\"3\" ry=\"3\" fill=\"{fill}\"{fo} stroke=\"{nstroke}\"{nso} stroke-width=\"{STROKE_W}\"/>",
        );
        emit_text(&mut svg, &p.label, cx, box_top + box_h / 2.0, opts);
    }

    // --- Messages ---
    for (mi, m) in diagram.messages.iter().enumerate() {
        let my = row_y[mi];
        let (Some(fc), Some(tc)) = (col_of(&m.from), col_of(&m.to)) else {
            continue; // shouldn't happen: endpoints are auto-declared
        };
        if fc == tc {
            emit_self_message(&mut svg, centers[fc], my, m, opts);
        } else {
            emit_message(&mut svg, centers[fc], centers[tc], my, m, opts);
        }
    }

    svg.push_str("</svg>");

    MermaidRender { svg, width_px: w, height_px: h }
}

/// `<defs>` with the filled-triangle end marker (oriented along the path).
fn emit_defs(svg: &mut String, opts: &MermaidOptions) {
    let (fill, fo) = fill_attrs(opts.edge_stroke);
    let _ = write!(
        svg,
        "<defs><marker id=\"seq-arrow\" markerWidth=\"{len}\" markerHeight=\"{w}\" \
         refX=\"{len}\" refY=\"{half}\" orient=\"auto\" markerUnits=\"userSpaceOnUse\">\
         <path d=\"M0,0 L{len},{half} L0,{w} Z\" fill=\"{fill}\"{fo}/></marker></defs>",
        len = ARROW_LEN,
        w = ARROW_HALF * 2.0,
        half = ARROW_HALF,
    );
}

/// A normal (cross-lifeline) message: horizontal line + head + centered text.
fn emit_message(svg: &mut String, x_from: f32, x_to: f32, y: f32, m: &Message, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let dash = if m.dashed { " stroke-dasharray=\"4 3\"" } else { "" };

    // Pull the line back from the target so the head's tip lands on the lifeline.
    let dir = if x_to >= x_from { 1.0 } else { -1.0 };
    let line_end = match m.style {
        ArrowStyle::Filled => x_to - dir * ARROW_LEN,
        _ => x_to,
    };
    let marker = if m.style == ArrowStyle::Filled {
        " marker-end=\"url(#seq-arrow)\""
    } else {
        ""
    };

    let _ = write!(
        svg,
        "<line x1=\"{x_from:.2}\" y1=\"{y:.2}\" x2=\"{line_end:.2}\" y2=\"{y:.2}\" \
         stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"{dash}{marker}/>",
    );

    // Open / async heads: a small V at the target. Cross: an ✗.
    match m.style {
        ArrowStyle::Open | ArrowStyle::Async => emit_open_head(svg, x_to, y, dir, opts),
        ArrowStyle::Cross => emit_cross(svg, x_to, y, opts),
        ArrowStyle::Filled => {}
    }

    // Text centered above the line.
    if !m.text.is_empty() {
        let cx = (x_from + x_to) / 2.0;
        let ty = y - opts.font_size_px * 0.4;
        emit_text(svg, &m.text, cx, ty, opts);
    }
}

/// A self-message: a small rectangular loop to the right of the lifeline, with
/// a head returning to the lifeline and the text to the loop's right.
fn emit_self_message(svg: &mut String, cx: f32, y: f32, m: &Message, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let dash = if m.dashed { " stroke-dasharray=\"4 3\"" } else { "" };

    let right = cx + SELF_LOOP_W;
    let y0 = y;
    let y1 = y + SELF_LOOP_H;

    // Out from lifeline, down, back toward lifeline (stop short for the head).
    let back_end = match m.style {
        ArrowStyle::Filled => cx + ARROW_LEN,
        _ => cx,
    };
    let marker = if m.style == ArrowStyle::Filled {
        " marker-end=\"url(#seq-arrow)\""
    } else {
        ""
    };

    let _ = write!(
        svg,
        "<path d=\"M{cx:.2},{y0:.2} L{right:.2},{y0:.2} L{right:.2},{y1:.2} L{back_end:.2},{y1:.2}\" \
         fill=\"none\" stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"{dash}{marker}/>",
    );

    // Heads pointing left (dir = -1) back at the lifeline on the lower segment.
    match m.style {
        ArrowStyle::Open | ArrowStyle::Async => emit_open_head(svg, cx, y1, -1.0, opts),
        ArrowStyle::Cross => emit_cross(svg, cx, y1, opts),
        ArrowStyle::Filled => {}
    }

    if !m.text.is_empty() {
        let tx = right + 4.0;
        let ty = (y0 + y1) / 2.0;
        emit_text_left(svg, &m.text, tx, ty, opts);
    }
}

/// An open ("V") arrowhead at `(x, y)` pointing in `dir` (+1 right / -1 left).
fn emit_open_head(svg: &mut String, x: f32, y: f32, dir: f32, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let bx = x - dir * ARROW_LEN;
    let _ = write!(
        svg,
        "<path d=\"M{bx:.2},{ty:.2} L{x:.2},{y:.2} L{bx:.2},{by:.2}\" \
         fill=\"none\" stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"/>",
        ty = y - ARROW_HALF,
        by = y + ARROW_HALF,
    );
}

/// A small cross (`✗`) end-marker centered at `(x, y)`.
fn emit_cross(svg: &mut String, x: f32, y: f32, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let r = ARROW_HALF;
    let _ = write!(
        svg,
        "<path d=\"M{x0:.2},{y0:.2} L{x1:.2},{y1:.2} M{x0:.2},{y1:.2} L{x1:.2},{y0:.2}\" \
         fill=\"none\" stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"/>",
        x0 = x - r,
        x1 = x + r,
        y0 = y - r,
        y1 = y + r,
    );
}

// ----------------------------------------------------------------------------
// Text + color helpers (mirrors draw.rs conventions)
// ----------------------------------------------------------------------------

/// A centered single-line `<text>` at `(cx, cy)`.
fn emit_text(svg: &mut String, label: &str, cx: f32, cy: f32, opts: &MermaidOptions) {
    if label.is_empty() {
        return;
    }
    let (fill, fo) = fill_attrs(opts.text_color);
    let family = escape(&opts.font_family);
    let fs = opts.font_size_px;
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}>{}</text>",
        escape(label),
    );
}

/// A left-anchored single-line `<text>` at `(x, cy)` (for self-message labels).
fn emit_text_left(svg: &mut String, label: &str, x: f32, cy: f32, opts: &MermaidOptions) {
    if label.is_empty() {
        return;
    }
    let (fill, fo) = fill_attrs(opts.text_color);
    let family = escape(&opts.font_family);
    let fs = opts.font_size_px;
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{cy:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}>{}</text>",
        escape(label),
    );
}

/// `fill="rgb(r,g,b)"` plus optional ` fill-opacity`.
fn fill_attrs(color: [u8; 4]) -> (String, String) {
    let [r, g, b, a] = color;
    let fill = format!("rgb({r},{g},{b})");
    let opacity = if a < 255 {
        format!(" fill-opacity=\"{:.4}\"", a as f32 / 255.0)
    } else {
        String::new()
    };
    (fill, opacity)
}

/// Same as [`fill_attrs`] but the opacity attribute is `stroke-opacity`.
fn stroke_attrs(color: [u8; 4]) -> (String, String) {
    let [r, g, b, a] = color;
    let stroke = format!("rgb({r},{g},{b})");
    let opacity = if a < 255 {
        format!(" stroke-opacity=\"{:.4}\"", a as f32 / 255.0)
    } else {
        String::new()
    };
    (stroke, opacity)
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

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    // --- Parsing ---

    #[test]
    fn parses_declared_and_alias_participants() {
        let d = parse_sequence(
            "sequenceDiagram\n participant A\n participant B as Bob\n actor C\n A->>B: hi\n",
        )
        .unwrap();
        assert_eq!(d.participants.len(), 3);
        assert_eq!(d.participants[0], Participant { id: "A".into(), label: "A".into() });
        assert_eq!(d.participants[1], Participant { id: "B".into(), label: "Bob".into() });
        assert_eq!(d.participants[2], Participant { id: "C".into(), label: "C".into() });
    }

    #[test]
    fn auto_creates_undeclared_participants_in_first_appearance_order() {
        let d = parse_sequence("sequenceDiagram\n X->>Y: hi\n Y-->>Z: bye\n").unwrap();
        let ids: Vec<_> = d.participants.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["X", "Y", "Z"]);
    }

    #[test]
    fn parses_each_arrow_kind() {
        let d = parse_sequence(
            "sequenceDiagram\n A->>B: a\n A-->>B: b\n A->B: c\n A-->B: d\n A-)B: e\n A-xB: f\n A--xB: g\n",
        )
        .unwrap();
        assert_eq!(d.messages.len(), 7);
        assert_eq!(d.messages[0].style, ArrowStyle::Filled);
        assert!(!d.messages[0].dashed);
        assert_eq!(d.messages[1].style, ArrowStyle::Filled);
        assert!(d.messages[1].dashed);
        assert_eq!(d.messages[2].style, ArrowStyle::Open);
        assert_eq!(d.messages[3].style, ArrowStyle::Open);
        assert!(d.messages[3].dashed);
        assert_eq!(d.messages[4].style, ArrowStyle::Async);
        assert_eq!(d.messages[5].style, ArrowStyle::Cross);
        assert_eq!(d.messages[6].style, ArrowStyle::Cross);
        assert!(d.messages[6].dashed);
    }

    #[test]
    fn parses_message_text_and_endpoints() {
        let d = parse_sequence("sequenceDiagram\n Alice ->> Bob : Hello Bob\n").unwrap();
        let m = &d.messages[0];
        assert_eq!(m.from, "Alice");
        assert_eq!(m.to, "Bob");
        assert_eq!(m.text, "Hello Bob");
    }

    #[test]
    fn parses_self_message() {
        let d = parse_sequence("sequenceDiagram\n A->>A: think\n").unwrap();
        assert_eq!(d.messages.len(), 1);
        assert_eq!(d.messages[0].from, "A");
        assert_eq!(d.messages[0].to, "A");
        // Only one participant created.
        assert_eq!(d.participants.len(), 1);
    }

    #[test]
    fn skips_blocks_and_notes() {
        let d = parse_sequence(
            "sequenceDiagram\n participant A\n participant B\n loop every minute\n A->>B: ping\n end\n Note over A,B: hi\n",
        )
        .unwrap();
        assert_eq!(d.participants.len(), 2);
        assert_eq!(d.messages.len(), 1);
    }

    #[test]
    fn alias_updates_label_for_message_first_id() {
        // B appears in a message first, then a declaration aliases it.
        let d = parse_sequence("sequenceDiagram\n A->>B: hi\n participant B as Bob\n").unwrap();
        let b = d.participants.iter().find(|p| p.id == "B").unwrap();
        assert_eq!(b.label, "Bob");
    }

    #[test]
    fn missing_header_is_parse_error() {
        assert!(parse_sequence("participant A\n A->>B: hi\n").is_err());
    }

    #[test]
    fn malformed_message_is_parse_error() {
        // No arrow token.
        assert!(parse_sequence("sequenceDiagram\n A B C\n").is_err());
    }

    // --- Rendering ---

    #[test]
    fn renders_svg_envelope() {
        let r = render_sequence("sequenceDiagram\n A->>B: hi\n", &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"), "got {}", &r.svg[..r.svg.len().min(40)]);
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn one_box_and_lifeline_per_participant() {
        let r = render_sequence(
            "sequenceDiagram\n participant A\n participant B\n A->>B: hi\n",
            &opts(),
        )
        .unwrap();
        // Two participant <rect>s.
        assert_eq!(r.svg.matches("<rect").count(), 2);
        // Two dashed lifelines (3 3 dash); message dashes use 4 3.
        assert_eq!(r.svg.matches("stroke-dasharray=\"3 3\"").count(), 2);
    }

    #[test]
    fn message_line_and_text_present() {
        let r = render_sequence("sequenceDiagram\n A->>B: Hello\n", &opts()).unwrap();
        // At least one <line> for the message (plus 2 lifelines).
        assert!(r.svg.matches("<line").count() >= 3);
        assert!(r.svg.contains(">Hello</text>"));
    }

    #[test]
    fn filled_arrow_uses_marker() {
        let r = render_sequence("sequenceDiagram\n A->>B: hi\n", &opts()).unwrap();
        assert!(r.svg.contains("marker-end=\"url(#seq-arrow)\""));
        assert!(r.svg.contains("<marker id=\"seq-arrow\""));
    }

    #[test]
    fn dashed_message_has_dash_pattern() {
        let r = render_sequence("sequenceDiagram\n A-->>B: hi\n", &opts()).unwrap();
        // Message dash pattern (distinct from lifeline 3 3).
        assert!(r.svg.contains("stroke-dasharray=\"4 3\""));
    }

    #[test]
    fn solid_message_has_no_message_dash() {
        let r = render_sequence("sequenceDiagram\n A->>B: hi\n", &opts()).unwrap();
        assert!(!r.svg.contains("stroke-dasharray=\"4 3\""));
    }

    #[test]
    fn cross_arrow_draws_a_cross() {
        let r = render_sequence("sequenceDiagram\n A-xB: bye\n", &opts()).unwrap();
        // The cross has two strokes joined by an `M` move within one path.
        assert!(r.svg.contains(" M"), "expected a cross path with a move-to: {}", r.svg);
    }

    #[test]
    fn self_message_renders_loop_path() {
        let r = render_sequence("sequenceDiagram\n A->>A: think\n", &opts()).unwrap();
        // The loop is a multi-segment <path>.
        assert!(r.svg.contains("<path d=\"M"));
        assert!(r.svg.contains(">think</text>"));
    }

    #[test]
    fn auto_created_participant_from_message_renders() {
        // Z never declared; should still get a box.
        let r = render_sequence("sequenceDiagram\n participant A\n A->>Z: hi\n", &opts()).unwrap();
        assert_eq!(r.svg.matches("<rect").count(), 2);
        assert!(r.svg.contains(">Z</text>"));
    }

    #[test]
    fn xml_escapes_message_text() {
        let r = render_sequence("sequenceDiagram\n A->>B: a & b < c\n", &opts()).unwrap();
        assert!(r.svg.contains("a &amp; b &lt; c"));
        assert!(!r.svg.contains("a & b"));
    }

    #[test]
    fn empty_input_is_error() {
        // No header at all → a Parse error (not Empty).
        assert!(matches!(render_sequence("", &opts()), Err(MermaidError::Parse(_))));
    }

    #[test]
    fn no_participants_is_empty_error() {
        // Header only, no participants / messages.
        assert!(matches!(render_sequence("sequenceDiagram\n", &opts()), Err(MermaidError::Empty)));
    }

    #[test]
    fn deterministic_output() {
        let src = "sequenceDiagram\n participant A\n participant B as Bob\n A->>B: hi\n B-->>A: yo\n A->>A: think\n";
        let a = render_sequence(src, &opts()).unwrap();
        let b = render_sequence(src, &opts()).unwrap();
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
