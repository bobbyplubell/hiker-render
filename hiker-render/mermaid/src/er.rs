//! `er` (entity-relationship) diagram (`erDiagram`).
//!
//! Self-contained: parse → build a `hiker_graph` layered (dagre) graph → lay
//! out → draw one SVG document. Supported subset:
//!
//! * relationships `CUSTOMER ||--o{ ORDER : places` — two entity names, a
//!   cardinality token pair `<left>--<right>` (identifying / solid) or
//!   `<left>..<right>` (non-identifying / dashed), and an optional `: label`.
//!   Cardinality tokens: `||` exactly-one, `|{`/`}|` one-or-many, `o{`/`}o`
//!   zero-or-many, `o|`/`|o` zero-or-one.
//! * entities are auto-created in first-seen order.
//! * an entity attribute block `CUSTOMER { string name PK \n int age }` — each
//!   row is `type name [keys...]`; rows render under the entity's name header.
//!
//! Cardinality is rendered with proper **crow's-foot notation**: small
//! line/path marks drawn at each entity end of the relationship line, oriented
//! along that line's terminal segment (a double bar for exactly-one, an open
//! circle for the zero forms, a splayed crow's foot for the many forms).
//! Non-identifying relationships draw a dashed line.

use std::collections::HashMap;
use std::fmt::Write as _;

use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

/// One cardinality end of a relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cardinality {
    /// `||` exactly one.
    ExactlyOne,
    /// `|{` / `}|` one or more.
    OneOrMore,
    /// `o{` / `}o` zero or more.
    ZeroOrMore,
    /// `o|` / `|o` zero or one.
    ZeroOrOne,
}

impl Cardinality {
    /// True when this end's outer mark is an open circle (the `o…` forms,
    /// i.e. the "zero" cardinalities).
    fn has_circle(self) -> bool {
        matches!(self, Cardinality::ZeroOrMore | Cardinality::ZeroOrOne)
    }

    /// True when this end fans out into a crow's foot (the "many" forms).
    fn has_foot(self) -> bool {
        matches!(self, Cardinality::OneOrMore | Cardinality::ZeroOrMore)
    }
}

/// An attribute row inside an entity box: `type name [keys...]`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Attribute {
    ty: String,
    name: String,
    keys: String,
}

/// An entity (table). `attrs` empty → name-only box.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Entity {
    name: String,
    attrs: Vec<Attribute>,
}

/// A relationship between two entities.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Relationship {
    left: String,
    right: String,
    left_card: Cardinality,
    right_card: Cardinality,
    /// Non-identifying (`..`) → dashed line.
    dashed: bool,
    label: Option<String>,
}

/// Parsed ER diagram.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ErDiagram {
    /// Entities in first-seen order.
    entities: Vec<Entity>,
    relationships: Vec<Relationship>,
}

/// Parse an ER diagram source. Errors on a missing/wrong header.
fn parse(src: &str) -> Result<ErDiagram, String> {
    let mut diag = ErDiagram::default();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut saw_header = false;
    let mut pending_header = true;
    // When inside `ENTITY { ... }`, this holds the entity index.
    let mut in_block: Option<usize> = None;

    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if pending_header {
            let kw = line.split_whitespace().next().unwrap_or("");
            if kw != "erDiagram" {
                return Err(format!("expected `erDiagram` header, got {kw:?}"));
            }
            saw_header = true;
            pending_header = false;
            continue;
        }

        // Inside an attribute block.
        if let Some(ei) = in_block {
            if line == "}" {
                in_block = None;
                continue;
            }
            if let Some(attr) = parse_attribute(line) {
                diag.entities[ei].attrs.push(attr);
            }
            continue;
        }

        // Entity attribute-block open: `ENTITY {`.
        if let Some(name) = line.strip_suffix('{') {
            let name = name.trim();
            if !name.is_empty() {
                let ei = ensure_entity(name, &mut diag, &mut index_of);
                in_block = Some(ei);
                continue;
            }
        }

        // Relationship line.
        if let Some(rel) = parse_relationship(line) {
            ensure_entity(&rel.left, &mut diag, &mut index_of);
            ensure_entity(&rel.right, &mut diag, &mut index_of);
            diag.relationships.push(rel);
            continue;
        }

        // Bare entity declaration: a single token.
        let mut toks = line.split_whitespace();
        if let Some(name) = toks.next() {
            if toks.next().is_none() {
                ensure_entity(name, &mut diag, &mut index_of);
            }
        }
    }

    if !saw_header {
        return Err("empty input / no erDiagram header".to_string());
    }
    Ok(diag)
}

/// Upsert an entity by name, returning its index.
fn ensure_entity(
    name: &str,
    diag: &mut ErDiagram,
    index_of: &mut HashMap<String, usize>,
) -> usize {
    if let Some(&i) = index_of.get(name) {
        return i;
    }
    let i = diag.entities.len();
    index_of.insert(name.to_string(), i);
    diag.entities.push(Entity {
        name: name.to_string(),
        attrs: Vec::new(),
    });
    i
}

/// Parse `type name [keys...]` → an [`Attribute`]. `None` if it has no name.
fn parse_attribute(line: &str) -> Option<Attribute> {
    let mut toks = line.split_whitespace();
    let ty = toks.next()?.to_string();
    let name = toks.next()?.to_string();
    let keys = toks.collect::<Vec<_>>().join(" ");
    Some(Attribute { ty, name, keys })
}

/// Parse a relationship line `LEFT <card>--<card> RIGHT [: label]`. Returns
/// `None` if the line has no relationship token.
fn parse_relationship(line: &str) -> Option<Relationship> {
    // Split off an optional `: label`.
    let (body, label) = match line.split_once(':') {
        Some((b, l)) => (b.trim(), Some(l.trim().to_string())),
        None => (line, None),
    };

    // Find the cardinality connector: `--` (identifying) or `..` (non-id).
    // The connector is flanked by cardinality tokens with no spaces, e.g.
    // `||--o{`. We locate the connector inside the middle whitespace-delimited
    // token.
    let mut toks = body.split_whitespace();
    let left = toks.next()?.to_string();
    let mid = toks.next()?;
    let right = toks.next()?.to_string();
    if toks.next().is_some() {
        // Extra tokens → not a simple relationship.
        return None;
    }

    let (dashed, conn_at) = if let Some(i) = mid.find("--") {
        (false, i)
    } else if let Some(i) = mid.find("..") {
        (true, i)
    } else {
        return None;
    };

    let left_tok = &mid[..conn_at];
    let right_tok = &mid[conn_at + 2..];
    let left_card = parse_card(left_tok, true)?;
    let right_card = parse_card(right_tok, false)?;

    Some(Relationship {
        left,
        right,
        left_card,
        right_card,
        dashed,
        label: label.filter(|l| !l.is_empty()),
    })
}

/// Parse a cardinality token. `left` chooses the orientation for the
/// one-or-many / zero-or-many forms (`|{` vs `}|`).
fn parse_card(tok: &str, left: bool) -> Option<Cardinality> {
    match tok {
        "||" => Some(Cardinality::ExactlyOne),
        "|{" | "}|" => Some(Cardinality::OneOrMore),
        "o{" | "}o" => Some(Cardinality::ZeroOrMore),
        "o|" | "|o" => Some(Cardinality::ZeroOrOne),
        _ => {
            let _ = left;
            None
        }
    }
}

/// Header-bar height for an entity box, px.
const HEADER_PAD_Y: f32 = 8.0;
/// Per-attribute-row height factor (× font size).
const ROW_H_EM: f32 = 1.5;

/// Render a mermaid `er` diagram to SVG.
pub fn render_er(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diag = parse(src).map_err(MermaidError::Parse)?;
    if diag.entities.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let row_h = fs * ROW_H_EM;

    // id → node index (first-seen order matches dagre node indices).
    let index_of: HashMap<&str, u32> = diag
        .entities
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.as_str(), i as u32))
        .collect();

    // Size each entity box: width = widest of (name, attr rows) + padding;
    // height = header + attr rows.
    let sizes: Vec<(f32, f32)> = diag
        .entities
        .iter()
        .map(|e| {
            let (name_w, name_h) = text_size(&e.name, fs);
            let mut max_w = name_w;
            for a in &e.attrs {
                let row = attr_row_text(a);
                let (w, _) = text_size(&row, fs);
                max_w = max_w.max(w);
            }
            let w = max_w + 2.0 * opts.node_padding_x;
            let header_h = name_h + 2.0 * HEADER_PAD_Y;
            let h = header_h + e.attrs.len() as f32 * row_h;
            (w, h)
        })
        .collect();

    // Build the dagre edge list, plus a parallel list of edge-label box sizes so
    // dagre reserves space and positions each relationship label.
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(diag.relationships.len());
    let mut kept: Vec<usize> = Vec::with_capacity(diag.relationships.len());
    let mut label_sizes: Vec<Option<Vec2>> = Vec::with_capacity(diag.relationships.len());
    for (j, r) in diag.relationships.iter().enumerate() {
        if let (Some(&a), Some(&b)) =
            (index_of.get(r.left.as_str()), index_of.get(r.right.as_str()))
        {
            edges.push((a, b));
            kept.push(j);
            label_sizes.push(r.label.as_deref().filter(|l| !l.is_empty()).map(|l| {
                let (w, h) = text_size(l, fs);
                Vec2::new(w + 10.0, h + 6.0)
            }));
        }
    }

    let node_sizes: Vec<Vec2> = sizes.iter().map(|&(w, h)| Vec2::new(w, h)).collect();
    let engine = LayeredEngine {
        rankdir: RankDir::Tb,
        ranksep: opts.rank_sep,
        nodesep: opts.node_sep,
        edgesep: 20.0,
        default_node_size: Vec2::new(60.0, 40.0),
    };
    let out = engine.layout(&GraphInput {
        node_count: diag.entities.len(),
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

    // Group relationships by unordered entity pair so multiple relationships
    // between the same two entities spread their labels apart (index/count fed
    // to `edge_label_anchor`).
    let mut group_total: HashMap<(String, String), usize> = HashMap::new();
    for &orig in &kept {
        let r = &diag.relationships[orig];
        *group_total.entry(pair_key(&r.left, &r.right)).or_insert(0) += 1;
    }
    let mut group_seen: HashMap<(String, String), usize> = HashMap::new();

    // Relationship lines first, then entity boxes on top.
    for (dagre_idx, &orig) in kept.iter().enumerate() {
        let r = &diag.relationships[orig];
        let pts: Vec<(f32, f32)> = out
            .edge_routes
            .get(dagre_idx)
            .map(|route| route.iter().map(|p| (p.x, p.y)).collect())
            .unwrap_or_default();
        let key = pair_key(&r.left, &r.right);
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

    for (i, e) in diag.entities.iter().enumerate() {
        let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
        let (w, h) = sizes[i];
        emit_entity(&mut svg, e, pos.x, pos.y, w, h, opts);
    }

    svg.push_str("</svg>");

    Ok(MermaidRender {
        svg,
        width_px: width,
        height_px: height,
    })
}

/// Text of one attribute row (`type name keys`).
fn attr_row_text(a: &Attribute) -> String {
    if a.keys.is_empty() {
        format!("{} {}", a.ty, a.name)
    } else {
        format!("{} {} {}", a.ty, a.name, a.keys)
    }
}

/// One relationship: a line (dashed for non-identifying), a crow's-foot
/// cardinality marker at each entity end, and an optional label spread off the
/// midpoint so parallel relationships don't collide.
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
    let dash = if rel.dashed {
        " stroke-dasharray=\"4 3\""
    } else {
        ""
    };
    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\"{so}{dash}/>",
        d.trim_end(),
        stroke = rgb(opts.edge_stroke),
        so = opacity_attr("stroke-opacity", opts.edge_stroke),
    );

    // Crow's-foot markers at each entity end. The FROM marker sits at
    // `points[0]` pointing toward `points[1]`; the TO marker at the last point
    // pointing toward the previous one.
    let n = points.len();
    if let Some(dir) = unit(points[0], points[1]) {
        draw_crows_foot(svg, points[0], dir, rel.left_card, opts);
    }
    if let Some(dir) = unit(points[n - 1], points[n - 2]) {
        draw_crows_foot(svg, points[n - 1], dir, rel.right_card, opts);
    }

    // Relationship label, spread perpendicular off the midpoint by the edge's
    // slot within its (unordered) entity-pair group, on a light background.
    if let Some(label) = rel.label.as_deref().filter(|l| !l.is_empty()) {
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
                 fill=\"{bg}\" fill-opacity=\"0.85\"/>",
                x = cx - bw / 2.0,
                y = cy - bh / 2.0,
                bg = rgb(opts.background),
            );
            emit_text(svg, label, cx, cy, opts, opts.text_color);
        }
    }
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

/// Draw a crow's-foot cardinality marker at `tip` (the entity end of the line),
/// oriented along `dir` — the unit vector pointing *into* the line (away from
/// the entity, toward the relationship's interior). Marks are placed a few px
/// back from `tip` so they don't overlap the entity border.
///
/// Layout, walking from the entity outward into the line:
/// * `o` forms (`has_circle`): a small open circle nearest the entity.
/// * `{`/`}` forms (`has_foot`): a crow's foot splaying toward the entity, plus
///   one perpendicular tick further in.
/// * one forms (no foot): one or two perpendicular ticks (a double bar for
///   exactly-one).
fn draw_crows_foot(
    svg: &mut String,
    tip: (f32, f32),
    dir: (f32, f32),
    card: Cardinality,
    opts: &MermaidOptions,
) {
    let stroke = rgb(opts.edge_stroke);
    let so = opacity_attr("stroke-opacity", opts.edge_stroke);
    // Perpendicular to the line direction.
    let perp = (-dir.1, dir.0);
    let half = 5.0_f32; // tick / foot half-width
    let circle_r = 3.5_f32; // open-circle radius

    // A point `t` px along the line from `tip` (into the diagram interior).
    let along = |t: f32| (tip.0 + dir.0 * t, tip.1 + dir.1 * t);
    // A short perpendicular tick centered on the line at distance `t`.
    let tick = |svg: &mut String, t: f32| {
        let (cx, cy) = along(t);
        let _ = write!(
            svg,
            "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" \
             stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
            cx - perp.0 * half,
            cy - perp.1 * half,
            cx + perp.0 * half,
            cy + perp.1 * half,
        );
    };

    if card.has_foot() {
        // Crow's foot: three lines from an apex (further into the line) splaying
        // out to the entity edge near `tip`. Plus one tick just past the apex.
        let apex = along(12.0);
        let base_t = 2.0;
        let (bx, by) = along(base_t);
        // Three splay endpoints near the entity: center + two perpendicular.
        let center = (bx, by);
        let up = (bx + perp.0 * half, by + perp.1 * half);
        let down = (bx - perp.0 * half, by - perp.1 * half);
        for end in [center, up, down] {
            let _ = write!(
                svg,
                "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" \
                 stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
                apex.0, apex.1, end.0, end.1,
            );
        }
        // Perpendicular tick just past the apex.
        tick(svg, 14.0);
        // Open circle for the zero-or-many form, beyond the tick.
        if card.has_circle() {
            let (cx, cy) = along(14.0 + circle_r + 1.5);
            let _ = write!(
                svg,
                "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{circle_r:.2}\" fill=\"none\" \
                 stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
            );
        }
    } else if card.has_circle() {
        // Zero-or-one: one tick toward the entity, an open circle further in.
        tick(svg, 5.0);
        let (cx, cy) = along(5.0 + circle_r + 3.0);
        let _ = write!(
            svg,
            "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{circle_r:.2}\" fill=\"none\" \
             stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
        );
    } else {
        // Exactly-one: a double bar (two perpendicular ticks).
        tick(svg, 4.0);
        tick(svg, 8.0);
    }
}

/// An order-independent key for an entity pair, used to group parallel
/// relationships so their labels spread apart.
fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// An entity box: a header bar with the name, then attribute rows beneath.
fn emit_entity(
    svg: &mut String,
    e: &Entity,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    opts: &MermaidOptions,
) {
    let x = cx - w / 2.0;
    let y = cy - h / 2.0;
    let header_h = opts.font_size_px * 1.2 + 2.0 * HEADER_PAD_Y;

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

    // Header name centered in the header band.
    emit_text(svg, &e.name, cx, y + header_h / 2.0, opts, opts.text_color);

    if e.attrs.is_empty() {
        return;
    }

    // Separator under the header.
    let _ = write!(
        svg,
        "<line x1=\"{x:.2}\" y1=\"{hy:.2}\" x2=\"{x2:.2}\" y2=\"{hy:.2}\" \
         stroke=\"{stroke}\"{so} stroke-width=\"1\"/>",
        hy = y + header_h,
        x2 = x + w,
        stroke = rgb(opts.node_stroke),
        so = opacity_attr("stroke-opacity", opts.node_stroke),
    );

    let row_h = opts.font_size_px * ROW_H_EM;
    for (i, a) in e.attrs.iter().enumerate() {
        let row_cy = y + header_h + row_h * (i as f32 + 0.5);
        // Left-aligned attribute text.
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
            txt = escape(&attr_row_text(a)),
        );
    }
}

/// A centered single-line `<text>` in the given color.
fn emit_text(
    svg: &mut String,
    label: &str,
    cx: f32,
    cy: f32,
    opts: &MermaidOptions,
    color: [u8; 4],
) {
    if label.is_empty() {
        return;
    }
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}>{txt}</text>",
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
    fn parse_relationship_basic() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places";
        let d = parse(src).unwrap();
        assert_eq!(d.entities.len(), 2);
        assert_eq!(d.entities[0].name, "CUSTOMER");
        assert_eq!(d.entities[1].name, "ORDER");
        assert_eq!(d.relationships.len(), 1);
        let r = &d.relationships[0];
        assert_eq!(r.left_card, Cardinality::ExactlyOne);
        assert_eq!(r.right_card, Cardinality::ZeroOrMore);
        assert!(!r.dashed);
        assert_eq!(r.label.as_deref(), Some("places"));
    }

    #[test]
    fn parse_all_cardinalities() {
        let src = "erDiagram\n  A |{--}o B\n  C o|--|o D";
        let d = parse(src).unwrap();
        assert_eq!(d.relationships[0].left_card, Cardinality::OneOrMore);
        assert_eq!(d.relationships[0].right_card, Cardinality::ZeroOrMore);
        assert_eq!(d.relationships[1].left_card, Cardinality::ZeroOrOne);
        assert_eq!(d.relationships[1].right_card, Cardinality::ZeroOrOne);
    }

    #[test]
    fn parse_non_identifying_is_dashed() {
        let src = "erDiagram\n  A ||..o{ B";
        let d = parse(src).unwrap();
        assert!(d.relationships[0].dashed);
    }

    #[test]
    fn parse_attribute_block() {
        let src = "erDiagram\n  CUSTOMER {\n    string name PK\n    int age\n  }";
        let d = parse(src).unwrap();
        assert_eq!(d.entities.len(), 1);
        let attrs = &d.entities[0].attrs;
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].ty, "string");
        assert_eq!(attrs[0].name, "name");
        assert_eq!(attrs[0].keys, "PK");
        assert_eq!(attrs[1].ty, "int");
        assert_eq!(attrs[1].name, "age");
        assert_eq!(attrs[1].keys, "");
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
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_entity_and_relationship_counts() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE : has";
        let r = render_er(src, &opts()).unwrap();
        // Three entities → at least three entity <rect> (label bg rects add more).
        assert!(r.svg.matches("<rect").count() >= 3);
        // Two relationships → two edge <path>s (fill="none"). Crow's-foot
        // circles also use fill="none", so count the <path…fill="none"> form.
        assert_eq!(r.svg.matches("<path d=").count(), 2);
        // Relationship labels present.
        assert!(r.svg.contains(">places<"));
        assert!(r.svg.contains(">has<"));
    }

    #[test]
    fn dashed_line_for_non_identifying() {
        let src = "erDiagram\n  A ||..o{ B";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains("stroke-dasharray=\"4 3\""));
    }

    #[test]
    fn solid_line_has_no_dash() {
        let src = "erDiagram\n  A ||--o{ B";
        let r = render_er(src, &opts()).unwrap();
        assert!(!r.svg.contains("stroke-dasharray"));
    }

    #[test]
    fn attribute_rows_rendered() {
        let src = "erDiagram\n  CUSTOMER {\n    string name PK\n  }\n  CUSTOMER ||--o{ ORDER : x";
        let r = render_er(src, &opts()).unwrap();
        // Attribute row text appears.
        assert!(r.svg.contains("string name PK"));
        // Separator <line> under the header.
        assert!(r.svg.contains("<line"));
    }

    #[test]
    fn crows_foot_marks_drawn() {
        // `||` (exactly-one) → a double bar of perpendicular ticks; `o{`
        // (zero-or-many) → a crow's foot plus an open circle. No textual glyphs.
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER";
        let r = render_er(src, &opts()).unwrap();
        // Old textual cardinality glyphs are gone.
        assert!(!r.svg.contains(">1</text>"));
        assert!(!r.svg.contains(">0+</text>"));
        // The zero-or-many end draws an open circle.
        assert!(r.svg.contains("<circle"), "zero-cardinality end has a circle");
        // Crow's-foot / tick marks are <line> elements (beyond the relationship
        // <path>). There should be several.
        assert!(r.svg.matches("<line").count() >= 4, "expected tick/foot lines: {}", r.svg);
    }

    #[test]
    fn exactly_one_has_no_circle() {
        // `||--||` → both ends are exactly-one (double bars), no circles.
        let src = "erDiagram\n  A ||--|| B";
        let r = render_er(src, &opts()).unwrap();
        assert!(!r.svg.contains("<circle"), "exactly-one ends draw no circle");
    }

    #[test]
    fn zero_or_one_end_has_circle() {
        // `o|` → zero-or-one: an open circle marker.
        let src = "erDiagram\n  A o|--|| B";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains("<circle"));
    }

    #[test]
    fn xml_escapes_label() {
        let src = "erDiagram\n  A ||--o{ B : a & b < c";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains("a &amp; b &lt; c"));
        assert!(!r.svg.contains("a & b"));
    }

    #[test]
    fn empty_diagram_errors() {
        assert_eq!(render_er("erDiagram\n", &opts()), Err(MermaidError::Empty));
    }

    #[test]
    fn deterministic() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE : has";
        let a = render_er(src, &opts()).unwrap();
        let b = render_er(src, &opts()).unwrap();
        assert_eq!(a, b);
    }
}
