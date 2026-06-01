//! `requirement` diagram (`requirementDiagram`).
//!
//! Self-contained: parse → build a `hiker_graph` layered (dagre) graph → lay
//! out → draw one SVG document. Supported subset:
//!
//! * **Requirement blocks** —
//!   ```text
//!   requirement <name> {
//!     id: <id>
//!     text: <quoted or bare>
//!     risk: <Low|Medium|High>
//!     verifymethod: <Analysis|Inspection|Test|Demonstration>
//!   }
//!   ```
//!   where the type keyword is one of `requirement | functionalRequirement |
//!   performanceRequirement | interfaceRequirement | physicalRequirement |
//!   designConstraint`. All inner key:value lines are optional.
//! * **Element blocks** — `element <name> { type: <text> \n docref: <text> }`.
//! * **Relationships** — `<src> - <relType> -> <dst>` (edge src→dst) and the
//!   reverse `<dst> <- <relType> - <src>` (edge src→dst, where `src` is the
//!   tail of the arrow). `<relType>` ∈ `contains | copies | derives | satisfies
//!   | verifies | refines | traces`.
//!
//! Each requirement / element is a node; relationships are directed edges. A
//! node is drawn as a box with a stacked header (`«<<type>>»` band, then the
//! bold **name**), followed by left-aligned rows for the present attributes.
//! Relationships are polylines with an open arrowhead at the destination end and
//! the relationship-type label placed (de-collided) via `edge_label_anchor`.
//!
//! Skipped (noted): styling, `class`/`classDef`, `direction`, and requirement
//! id cross-referencing beyond plain display.

use std::collections::HashMap;
use std::fmt::Write as _;

use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

/// A node is either a requirement or an element; both render as a header + rows
/// box and participate in the layered layout.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Node {
    /// The unique name / id used to reference this node in relationships.
    name: String,
    /// The `«<<...>>»` header band text (e.g. `requirement`, `element`).
    kind: String,
    /// Attribute rows (`label: value`) shown beneath the name, in order.
    rows: Vec<(String, String)>,
}

/// A directed relationship `src → dst` of a given type.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Relationship {
    src: String,
    dst: String,
    rel_type: String,
}

/// Parsed requirement diagram.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RequirementDiagram {
    /// Nodes (requirements + elements) in first-seen order.
    nodes: Vec<Node>,
    relationships: Vec<Relationship>,
}

/// The requirement type keywords mapped to their display label.
fn requirement_type(kw: &str) -> Option<&'static str> {
    Some(match kw {
        "requirement" => "Requirement",
        "functionalRequirement" => "Functional Requirement",
        "performanceRequirement" => "Performance Requirement",
        "interfaceRequirement" => "Interface Requirement",
        "physicalRequirement" => "Physical Requirement",
        "designConstraint" => "Design Constraint",
        _ => return None,
    })
}

/// The relationship type keywords (the set we accept).
fn relationship_type(kw: &str) -> Option<&'static str> {
    Some(match kw {
        "contains" => "contains",
        "copies" => "copies",
        "derives" => "derives",
        "satisfies" => "satisfies",
        "verifies" => "verifies",
        "refines" => "refines",
        "traces" => "traces",
        _ => return None,
    })
}

/// Parse a requirement-diagram source. Errors on a missing/wrong header.
fn parse(src: &str) -> Result<RequirementDiagram, String> {
    let mut diag = RequirementDiagram::default();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut pending_header = true;
    // While inside a `{ ... }` block, the node index being filled.
    let mut in_block: Option<usize> = None;

    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if pending_header {
            let kw = line.split_whitespace().next().unwrap_or("");
            if kw != "requirementDiagram" {
                return Err(format!("expected `requirementDiagram` header, got {kw:?}"));
            }
            pending_header = false;
            // Allow trailing tokens on the header line to be ignored.
            continue;
        }

        // Inside a requirement / element block: collect `key: value` rows.
        if let Some(ni) = in_block {
            if line == "}" || line.starts_with('}') {
                in_block = None;
                continue;
            }
            if let Some((key, val)) = parse_kv(line) {
                let label = row_label(&key);
                diag.nodes[ni].rows.push((label, val));
            }
            continue;
        }

        // Block openers: `<reqType> <name> {` or `element <name> {`. The body
        // may continue on following lines (multi-line form) or be inlined on
        // the same line (`requirement a { id: 1 }` — trivial single-line form).
        if let Some((kind, name, inline)) = parse_block_open(line) {
            let ni = ensure_node(&name, &kind, &mut diag, &mut index_of);
            // A block declaration sets the kind and replaces any prior rows.
            diag.nodes[ni].rows.clear();
            diag.nodes[ni].kind = kind;
            match inline {
                // Single-line: parse the inlined body, stay out of block mode.
                Some(body) => {
                    for part in body.split(['\n', ';']) {
                        let part = part.trim();
                        if part.is_empty() {
                            continue;
                        }
                        if let Some((key, val)) = parse_kv(part) {
                            let label = row_label(&key);
                            diag.nodes[ni].rows.push((label, val));
                        }
                    }
                }
                // Multi-line: subsequent lines fill the body until `}`.
                None => in_block = Some(ni),
            }
            continue;
        }

        // Relationship line.
        if let Some(rel) = parse_relationship(line) {
            ensure_node(&rel.src, "Requirement", &mut diag, &mut index_of);
            ensure_node(&rel.dst, "Requirement", &mut diag, &mut index_of);
            diag.relationships.push(rel);
            continue;
        }

        // Otherwise: ignore (styling / class / direction / unknown).
    }

    if pending_header {
        return Err("empty input / no requirementDiagram header".to_string());
    }
    Ok(diag)
}

/// Upsert a node by name, returning its index. A later block declaration can
/// upgrade an auto-created node's kind.
fn ensure_node(
    name: &str,
    kind: &str,
    diag: &mut RequirementDiagram,
    index_of: &mut HashMap<String, usize>,
) -> usize {
    if let Some(&i) = index_of.get(name) {
        return i;
    }
    let i = diag.nodes.len();
    index_of.insert(name.to_string(), i);
    diag.nodes.push(Node {
        name: name.to_string(),
        kind: kind.to_string(),
        rows: Vec::new(),
    });
    i
}

/// Try to parse a block-opener line. Two accepted forms:
/// * multi-line: `<keyword> <name> {`  → returns inline body `None`.
/// * single-line: `<keyword> <name> { <body> }` → returns the inline body text.
///
/// Returns the header label (`<<...>>` band text), the node name, and the
/// optional inline body. `None` if the line is not a block open.
fn parse_block_open(line: &str) -> Option<(String, String, Option<String>)> {
    // Must contain a `{` to be a block open.
    let brace = line.find('{')?;
    let head = line[..brace].trim();
    let after = line[brace + 1..].trim();
    // Single-line form closes with `}` on the same line.
    let inline = if let Some(body) = after.strip_suffix('}') {
        Some(body.trim().to_string())
    } else if after.is_empty() {
        None
    } else {
        // Trailing content with no close brace: treat as multi-line open and let
        // the (rare) inline remainder be ignored.
        None
    };

    let mut toks = head.split_whitespace();
    let kw = toks.next()?;
    let rest = head[kw.len()..].trim();
    if rest.is_empty() {
        return None;
    }
    let name = unquote(rest);
    if kw == "element" {
        Some(("element".to_string(), name, inline))
    } else {
        let label = requirement_type(kw)?;
        Some((label.to_string(), name, inline))
    }
}

/// Parse a `key: value` body line. The value may be quoted (kept unquoted) or
/// bare. Returns `None` if there is no colon.
fn parse_kv(line: &str) -> Option<(String, String)> {
    let (key, val) = line.split_once(':')?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return None;
    }
    let val = unquote(val.trim());
    Some((key, val))
}

/// The display label for a body key (`verifymethod` → `Verify Method`, etc.).
fn row_label(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "id" => "Id".to_string(),
        "text" => "Text".to_string(),
        "risk" => "Risk".to_string(),
        "verifymethod" => "Verify Method".to_string(),
        "type" => "Type".to_string(),
        "docref" => "Doc Ref".to_string(),
        _ => key.to_string(),
    }
}

/// Strip a single pair of surrounding double quotes, if present.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parse a relationship line:
/// * `<src> - <relType> -> <dst>`  → edge `src → dst`.
/// * `<dst> <- <relType> - <src>`  → edge `src → dst` (arrow tail is `src`).
///
/// Returns `None` if the line is not a relationship.
fn parse_relationship(line: &str) -> Option<Relationship> {
    // Forward form: `A - contains -> B`.
    if let Some((left, rest)) = line.split_once('-') {
        if let Some((mid, right)) = rest.split_once("->") {
            let mid = mid.trim();
            // Guard against the reverse form being misread: reverse form has
            // `<-` which would put a `<` at the end of `left`.
            if !left.trim_end().ends_with('<') {
                let src = left.trim();
                let dst = right.trim();
                if let Some(rt) = relationship_type(mid) {
                    if !src.is_empty() && !dst.is_empty() {
                        return Some(Relationship {
                            src: unquote(src),
                            dst: unquote(dst),
                            rel_type: rt.to_string(),
                        });
                    }
                }
            }
        }
    }

    // Reverse form: `B <- contains - A` → edge A → B.
    if let Some((left, rest)) = line.split_once("<-") {
        if let Some((mid, right)) = rest.rsplit_once('-') {
            let mid = mid.trim();
            let dst = left.trim();
            let src = right.trim();
            if let Some(rt) = relationship_type(mid) {
                if !src.is_empty() && !dst.is_empty() {
                    return Some(Relationship {
                        src: unquote(src),
                        dst: unquote(dst),
                        rel_type: rt.to_string(),
                    });
                }
            }
        }
    }

    None
}

/// Header band height padding, px.
const HEADER_PAD_Y: f32 = 6.0;
/// Per-row height factor (× font size).
const ROW_H_EM: f32 = 1.4;

/// Render a mermaid `requirement` diagram to SVG.
pub fn render_requirement(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diag = parse(src).map_err(MermaidError::Parse)?;
    if diag.nodes.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let row_h = fs * ROW_H_EM;
    // Header band holds two stacked lines: `«<<kind>>»` then the bold name.
    let header_line_h = fs * 1.3;
    let header_h = 2.0 * header_line_h + 2.0 * HEADER_PAD_Y;

    // name → node index (first-seen order matches dagre node indices).
    let index_of: HashMap<&str, u32> = diag
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.name.as_str(), i as u32))
        .collect();

    // Size each box: width = widest of (header `<<kind>>`, name, rows) +
    // padding; height = header band + rows.
    let sizes: Vec<(f32, f32)> = diag
        .nodes
        .iter()
        .map(|n| {
            let (kind_w, _) = text_size(&kind_header(&n.kind), fs);
            let (name_w, _) = text_size(&n.name, fs);
            let mut max_w = kind_w.max(name_w);
            for r in &n.rows {
                let (w, _) = text_size(&row_text(r), fs);
                max_w = max_w.max(w);
            }
            let w = max_w + 2.0 * opts.node_padding_x;
            let h = header_h + n.rows.len() as f32 * row_h;
            (w, h)
        })
        .collect();

    // Build the dagre edge list (dropping any relationship whose endpoints are
    // unknown — they are auto-created, so this never drops in practice). Also
    // build a parallel list of edge-label box sizes so dagre reserves space and
    // positions each relationship-type label.
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(diag.relationships.len());
    let mut kept: Vec<usize> = Vec::with_capacity(diag.relationships.len());
    let mut label_sizes: Vec<Option<Vec2>> = Vec::with_capacity(diag.relationships.len());
    for (j, r) in diag.relationships.iter().enumerate() {
        if let (Some(&a), Some(&b)) =
            (index_of.get(r.src.as_str()), index_of.get(r.dst.as_str()))
        {
            edges.push((a, b));
            kept.push(j);
            label_sizes.push(if r.rel_type.is_empty() {
                None
            } else {
                let (w, h) = text_size(&r.rel_type, fs);
                Some(Vec2::new(w + 10.0, h + 6.0))
            });
        }
    }

    let node_sizes: Vec<Vec2> = sizes.iter().map(|&(w, h)| Vec2::new(w, h)).collect();
    let engine = LayeredEngine {
        rankdir: RankDir::Tb,
        ranksep: opts.rank_sep,
        nodesep: opts.node_sep,
        edgesep: 20.0,
        default_node_size: Vec2::new(120.0, 60.0),
    };
    let out = engine.layout(&GraphInput {
        node_count: diag.nodes.len(),
        edges: &edges,
        node_sizes: Some(&node_sizes),
        edge_label_sizes: Some(&label_sizes),
        directed: true,
    });

    let width = (out.size.x.ceil() + 1.0).max(1.0);
    let height = (out.size.y.ceil() + 1.0).max(1.0);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\">"
    );

    // Group relationships by unordered node pair so multiple/bidirectional
    // relationships spread their labels apart.
    let mut group_total: HashMap<(String, String), usize> = HashMap::new();
    for &orig in &kept {
        let r = &diag.relationships[orig];
        *group_total.entry(pair_key(&r.src, &r.dst)).or_insert(0) += 1;
    }
    let mut group_seen: HashMap<(String, String), usize> = HashMap::new();

    // Relationship polylines first, then node boxes on top.
    for (dagre_idx, &orig) in kept.iter().enumerate() {
        let r = &diag.relationships[orig];
        let pts: Vec<(f32, f32)> = out
            .edge_routes
            .get(dagre_idx)
            .map(|route| route.iter().map(|p| (p.x, p.y)).collect())
            .unwrap_or_default();
        let key = pair_key(&r.src, &r.dst);
        let count = group_total.get(&key).copied().unwrap_or(1);
        let slot = group_seen.entry(key).or_insert(0);
        let index = *slot;
        *slot += 1;
        let label_pos = out
            .edge_label_positions
            .get(dagre_idx)
            .copied()
            .flatten()
            .map(|p| (p.x, p.y));
        emit_relationship(&mut svg, &pts, r, index, count, label_pos, opts);
    }

    for (i, n) in diag.nodes.iter().enumerate() {
        let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
        let (w, h) = sizes[i];
        emit_node(&mut svg, n, pos.x, pos.y, w, h, header_h, header_line_h, row_h, opts);
    }

    svg.push_str("</svg>");

    Ok(MermaidRender {
        svg,
        width_px: width,
        height_px: height,
    })
}

/// The `«<<kind>>»`-style header band text.
fn kind_header(kind: &str) -> String {
    format!("<<{kind}>>")
}

/// The text of one attribute row (`label: value`).
fn row_text(r: &(String, String)) -> String {
    format!("{}: {}", r.0, r.1)
}

/// An order-independent key for a node pair, used to group parallel /
/// bidirectional relationships so their labels spread apart.
fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// One relationship: a polyline, an open arrowhead at the `dst` end, and the
/// relationship-type label spread off the route on a light background.
fn emit_relationship(
    svg: &mut String,
    points: &[(f32, f32)],
    rel: &Relationship,
    index: usize,
    count: usize,
    label_pos: Option<(f32, f32)>,
    opts: &MermaidOptions,
) {
    if points.len() < 2 {
        return;
    }
    let mut d = String::new();
    for (i, (x, y)) in points.iter().enumerate() {
        let cmd = if i == 0 { 'M' } else { 'L' };
        let _ = write!(d, "{cmd}{x:.2},{y:.2} ");
    }
    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\"{so}/>",
        d.trim_end(),
        stroke = rgb(opts.edge_stroke),
        so = opacity_attr("stroke-opacity", opts.edge_stroke),
    );

    // Open arrowhead at the destination end (last point), oriented along the
    // terminal segment.
    let n = points.len();
    if let Some(dir) = unit(points[n - 2], points[n - 1]) {
        draw_open_arrow(svg, points[n - 1], dir, opts);
    }

    // Relationship-type label, de-collided via `edge_label_anchor`.
    let label = &rel.rel_type;
    if !label.is_empty() {
        // Prefer the dagre-reserved label center; fall back to the perpendicular
        // midpoint anchor when dagre didn't position it.
        let anchor = label_pos
            .or_else(|| edge_label_anchor(points, index, count, opts.font_size_px));
        if let Some((cx, cy)) = anchor {
            let chars = label.chars().count() as f32;
            let tw = chars * opts.font_size_px * 0.6;
            let th = opts.font_size_px * 1.2;
            let bw = tw + 4.0;
            let bh = th + 4.0;
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{bw:.2}\" height=\"{bh:.2}\" \
                 fill=\"rgb(255,255,255)\" fill-opacity=\"0.85\"/>",
                x = cx - bw / 2.0,
                y = cy - bh / 2.0,
            );
            emit_centered_text(svg, label, cx, cy, opts, opts.text_color, false);
        }
    }
}

/// Draw an **open** (two-stroke V) arrowhead at `tip`, pointing along `dir`
/// (the unit vector into the arrow tip).
fn draw_open_arrow(svg: &mut String, tip: (f32, f32), dir: (f32, f32), opts: &MermaidOptions) {
    let stroke = rgb(opts.edge_stroke);
    let so = opacity_attr("stroke-opacity", opts.edge_stroke);
    let len = 10.0_f32;
    let half = 4.0_f32;
    // Back along the line, then splay out perpendicular.
    let back = (tip.0 - dir.0 * len, tip.1 - dir.1 * len);
    let perp = (-dir.1, dir.0);
    let a = (back.0 + perp.0 * half, back.1 + perp.1 * half);
    let b = (back.0 - perp.0 * half, back.1 - perp.1 * half);
    let _ = write!(
        svg,
        "<path d=\"M{ax:.2},{ay:.2} L{tx:.2},{ty:.2} L{bx:.2},{by:.2}\" fill=\"none\" \
         stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
        ax = a.0,
        ay = a.1,
        tx = tip.0,
        ty = tip.1,
        bx = b.0,
        by = b.1,
    );
}

/// Unit vector from `a` toward `b`, or `None` for a degenerate segment.
fn unit(a: (f32, f32), b: (f32, f32)) -> Option<(f32, f32)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    if len <= 1e-3 {
        None
    } else {
        Some((dx / len, dy / len))
    }
}

/// A node box: a header band (`«<<kind>>»` over the bold name), a separator,
/// then left-aligned attribute rows.
#[allow(clippy::too_many_arguments)]
fn emit_node(
    svg: &mut String,
    node: &Node,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    header_h: f32,
    header_line_h: f32,
    row_h: f32,
    opts: &MermaidOptions,
) {
    let x = cx - w / 2.0;
    let y = cy - h / 2.0;

    // Outer box.
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
        fill = rgb(opts.node_fill),
        fo = opacity_attr("fill-opacity", opts.node_fill),
        stroke = rgb(opts.node_stroke),
        so = opacity_attr("stroke-opacity", opts.node_stroke),
    );

    // Header: `«<<kind>>»` (italic-ish via font-style) then the bold name.
    let kind_cy = y + HEADER_PAD_Y + header_line_h * 0.5;
    emit_styled_text(
        svg,
        &kind_header(&node.kind),
        cx,
        kind_cy,
        opts,
        opts.text_color,
        Some("italic"),
        None,
    );
    let name_cy = y + HEADER_PAD_Y + header_line_h * 1.5;
    emit_styled_text(
        svg,
        &node.name,
        cx,
        name_cy,
        opts,
        opts.text_color,
        None,
        Some("bold"),
    );

    // Separator under the header band.
    let _ = write!(
        svg,
        "<line x1=\"{x:.2}\" y1=\"{hy:.2}\" x2=\"{x2:.2}\" y2=\"{hy:.2}\" \
         stroke=\"{stroke}\"{so} stroke-width=\"1\"/>",
        hy = y + header_h,
        x2 = x + w,
        stroke = rgb(opts.node_stroke),
        so = opacity_attr("stroke-opacity", opts.node_stroke),
    );

    // Left-aligned attribute rows.
    for (i, r) in node.rows.iter().enumerate() {
        let row_cy = y + header_h + row_h * (i as f32 + 0.5);
        let tx = x + opts.node_padding_x;
        let _ = write!(
            svg,
            "<text x=\"{tx:.2}\" y=\"{row_cy:.2}\" text-anchor=\"start\" \
             dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{fs}\" \
             fill=\"{fill}\"{fo}>{txt}</text>",
            family = escape(&opts.font_family),
            fs = opts.font_size_px,
            fill = rgb(opts.text_color),
            fo = opacity_attr("fill-opacity", opts.text_color),
            txt = escape(&row_text(r)),
        );
    }
}

/// A centered single-line `<text>` (optionally bold via `emit_styled_text`).
fn emit_centered_text(
    svg: &mut String,
    label: &str,
    cx: f32,
    cy: f32,
    opts: &MermaidOptions,
    color: [u8; 4],
    bold: bool,
) {
    emit_styled_text(
        svg,
        label,
        cx,
        cy,
        opts,
        color,
        None,
        if bold { Some("bold") } else { None },
    );
}

/// A centered single-line `<text>` with optional `font-style` / `font-weight`.
#[allow(clippy::too_many_arguments)]
fn emit_styled_text(
    svg: &mut String,
    label: &str,
    cx: f32,
    cy: f32,
    opts: &MermaidOptions,
    color: [u8; 4],
    font_style: Option<&str>,
    font_weight: Option<&str>,
) {
    if label.is_empty() {
        return;
    }
    let style = font_style
        .map(|s| format!(" font-style=\"{s}\""))
        .unwrap_or_default();
    let weight = font_weight
        .map(|w| format!(" font-weight=\"{w}\""))
        .unwrap_or_default();
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\"{style}{weight} fill=\"{fill}\"{fo}>{txt}</text>",
        family = escape(&opts.font_family),
        fs = opts.font_size_px,
        fill = rgb(color),
        fo = opacity_attr("fill-opacity", color),
        txt = escape(label),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    #[test]
    fn parse_requirement_block() {
        let src = "requirementDiagram\n\
            requirement test_req {\n\
            id: 1\n\
            text: the test text.\n\
            risk: high\n\
            verifymethod: test\n\
            }";
        let d = parse(src).unwrap();
        assert_eq!(d.nodes.len(), 1);
        let n = &d.nodes[0];
        assert_eq!(n.name, "test_req");
        assert_eq!(n.kind, "Requirement");
        assert_eq!(n.rows.len(), 4);
        assert_eq!(n.rows[0], ("Id".to_string(), "1".to_string()));
        assert_eq!(n.rows[1], ("Text".to_string(), "the test text.".to_string()));
        assert_eq!(n.rows[2], ("Risk".to_string(), "high".to_string()));
        assert_eq!(n.rows[3], ("Verify Method".to_string(), "test".to_string()));
    }

    #[test]
    fn parse_requirement_type_variants() {
        let src = "requirementDiagram\n\
            functionalRequirement fr { }\n\
            performanceRequirement pr { }\n\
            interfaceRequirement ir { }\n\
            physicalRequirement phr { }\n\
            designConstraint dc { }";
        let d = parse(src).unwrap();
        assert_eq!(d.nodes.len(), 5);
        assert_eq!(d.nodes[0].kind, "Functional Requirement");
        assert_eq!(d.nodes[1].kind, "Performance Requirement");
        assert_eq!(d.nodes[2].kind, "Interface Requirement");
        assert_eq!(d.nodes[3].kind, "Physical Requirement");
        assert_eq!(d.nodes[4].kind, "Design Constraint");
    }

    #[test]
    fn parse_quoted_text() {
        let src = "requirementDiagram\n\
            requirement r {\n\
            text: \"a quoted text\"\n\
            }";
        let d = parse(src).unwrap();
        assert_eq!(d.nodes[0].rows[0], ("Text".to_string(), "a quoted text".to_string()));
    }

    #[test]
    fn parse_element_block() {
        let src = "requirementDiagram\n\
            element test_entity {\n\
            type: simulation\n\
            docref: reqs/test_entity\n\
            }";
        let d = parse(src).unwrap();
        assert_eq!(d.nodes.len(), 1);
        let n = &d.nodes[0];
        assert_eq!(n.name, "test_entity");
        assert_eq!(n.kind, "element");
        assert_eq!(n.rows.len(), 2);
        assert_eq!(n.rows[0], ("Type".to_string(), "simulation".to_string()));
        assert_eq!(n.rows[1], ("Doc Ref".to_string(), "reqs/test_entity".to_string()));
    }

    #[test]
    fn parse_all_relationship_types() {
        for rt in ["contains", "copies", "derives", "satisfies", "verifies", "refines", "traces"] {
            let src = format!("requirementDiagram\n  a - {rt} -> b");
            let d = parse(&src).unwrap();
            assert_eq!(d.relationships.len(), 1, "type {rt}");
            assert_eq!(d.relationships[0].src, "a");
            assert_eq!(d.relationships[0].dst, "b");
            assert_eq!(d.relationships[0].rel_type, rt);
        }
    }

    #[test]
    fn parse_reverse_arrow_form() {
        // `b <- contains - a` means the arrow tail is `a`, so the edge is a → b.
        let src = "requirementDiagram\n  b <- contains - a";
        let d = parse(src).unwrap();
        assert_eq!(d.relationships.len(), 1);
        assert_eq!(d.relationships[0].src, "a");
        assert_eq!(d.relationships[0].dst, "b");
        assert_eq!(d.relationships[0].rel_type, "contains");
    }

    #[test]
    fn auto_create_undeclared_endpoints() {
        let src = "requirementDiagram\n  foo - satisfies -> bar";
        let d = parse(src).unwrap();
        assert_eq!(d.nodes.len(), 2);
        assert_eq!(d.nodes[0].name, "foo");
        assert_eq!(d.nodes[1].name, "bar");
        // Auto-created nodes default to the requirement kind.
        assert_eq!(d.nodes[0].kind, "Requirement");
    }

    #[test]
    fn declared_block_upgrades_auto_node() {
        // Relationship first auto-creates `r` as a requirement; a later element
        // block upgrades its kind.
        let src = "requirementDiagram\n\
            r - contains -> e\n\
            element e {\n\
            type: t\n\
            }";
        let d = parse(src).unwrap();
        let e = d.nodes.iter().find(|n| n.name == "e").unwrap();
        assert_eq!(e.kind, "element");
        assert_eq!(e.rows.len(), 1);
    }

    #[test]
    fn bad_header_errors() {
        assert!(parse("graph TD\n a --> b").is_err());
    }

    #[test]
    fn no_header_errors() {
        assert!(parse("\n\n").is_err());
    }

    #[test]
    fn render_wellformed_svg() {
        let src = "requirementDiagram\n\
            requirement test_req {\n\
            id: 1\n\
            text: the test text.\n\
            risk: high\n\
            verifymethod: test\n\
            }\n\
            element test_entity {\n\
            type: simulation\n\
            }\n\
            test_entity - satisfies -> test_req";
        let r = render_requirement(src, &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_box_per_node_with_header_and_name() {
        let src = "requirementDiagram\n\
            requirement test_req {\n\
            id: 1\n\
            }\n\
            element test_entity {\n\
            type: simulation\n\
            }\n\
            test_entity - satisfies -> test_req";
        let r = render_requirement(src, &opts()).unwrap();
        // Two node boxes (plus a label background rect).
        assert!(r.svg.matches("<rect").count() >= 2);
        // Headers and names present.
        assert!(r.svg.contains("&lt;&lt;Requirement&gt;&gt;"));
        assert!(r.svg.contains("&lt;&lt;element&gt;&gt;"));
        assert!(r.svg.contains(">test_req<"));
        assert!(r.svg.contains(">test_entity<"));
    }

    #[test]
    fn render_relationship_polyline_arrow_and_label() {
        let src = "requirementDiagram\n\
            requirement a { }\n\
            element b { }\n\
            b - satisfies -> a";
        let r = render_requirement(src, &opts()).unwrap();
        // One edge polyline (fill="none" path) plus one arrowhead path.
        assert_eq!(r.svg.matches("<path d=").count(), 2);
        // Relationship label present.
        assert!(r.svg.contains(">satisfies<"));
    }

    #[test]
    fn render_rows_present() {
        let src = "requirementDiagram\n\
            requirement r {\n\
            id: 7\n\
            risk: medium\n\
            }";
        let r = render_requirement(src, &opts()).unwrap();
        assert!(r.svg.contains("Id: 7"));
        assert!(r.svg.contains("Risk: medium"));
        // Header separator line under the band.
        assert!(r.svg.contains("<line"));
    }

    #[test]
    fn xml_escapes_text() {
        let src = "requirementDiagram\n\
            requirement r {\n\
            text: a & b < c\n\
            }";
        let r = render_requirement(src, &opts()).unwrap();
        assert!(r.svg.contains("a &amp; b &lt; c"));
        assert!(!r.svg.contains("a & b"));
    }

    #[test]
    fn empty_diagram_errors() {
        assert_eq!(
            render_requirement("requirementDiagram\n", &opts()),
            Err(MermaidError::Empty)
        );
    }

    #[test]
    fn deterministic() {
        let src = "requirementDiagram\n\
            requirement a { id: 1 }\n\
            element b { type: t }\n\
            b - satisfies -> a\n\
            a - traces -> b";
        let x = render_requirement(src, &opts()).unwrap();
        let y = render_requirement(src, &opts()).unwrap();
        assert_eq!(x, y);
    }

    #[test]
    fn bidirectional_pair_labels_separated() {
        // Two relationships between the same pair (one each direction) should
        // have their labels placed at distinct anchors.
        let src = "requirementDiagram\n\
            requirement a { }\n\
            element b { }\n\
            a - contains -> b\n\
            b - copies -> a";
        let r = render_requirement(src, &opts()).unwrap();
        assert!(r.svg.contains(">contains<"));
        assert!(r.svg.contains(">copies<"));
        // The two label backgrounds must sit at distinct anchors. Dagre now
        // reserves the labels in the rank gap and orders them apart (here in x),
        // so check separation in either axis rather than y specifically.
        let pts = label_rect_centers(&r.svg);
        assert_eq!(pts.len(), 2, "two label backgrounds");
        let (dx, dy) = ((pts[0].0 - pts[1].0).abs(), (pts[0].1 - pts[1].1).abs());
        assert!(dx > 1.0 || dy > 1.0, "labels separated: {pts:?}");
    }

    /// Parse the `(x+w/2, y+h/2)` centers of each white label-background rect.
    fn label_rect_centers(svg: &str) -> Vec<(f32, f32)> {
        svg.match_indices("fill=\"rgb(255,255,255)\"")
            .filter_map(|(i, _)| {
                let rect_start = svg[..i].rfind("<rect")?;
                let seg = &svg[rect_start..i];
                let attr = |name: &str| -> Option<f32> {
                    let s = seg.find(name)? + name.len();
                    let rest = &seg[s..];
                    let end = rest.find('"')?;
                    rest[..end].parse::<f32>().ok()
                };
                Some((
                    attr(" x=\"")? + attr(" width=\"")? / 2.0,
                    attr(" y=\"")? + attr(" height=\"")? / 2.0,
                ))
            })
            .collect()
    }
}
