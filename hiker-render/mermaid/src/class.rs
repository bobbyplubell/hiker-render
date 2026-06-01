//! `classDiagram` — UML class diagrams.
//!
//! Self-contained `parse → size → layout → draw` for the mermaid class diagram
//! subset. Classes become multi-compartment boxes (name / attributes / methods)
//! laid out with the [`hiker_graph`] layered (dagre) engine; relationships become
//! routed polylines with the appropriate UML end marker (inheritance triangle,
//! association/dependency arrow, aggregation/composition diamond), dashed for
//! `..` (dependency / realization) lines.
//!
//! Skipped (noted, not parsed specially): generics `~T~`, annotations
//! `<<interface>>`, namespaces, cardinality/multiplicity labels, and `note`.

use std::fmt::Write as _;

use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb, text_size, LINE_HEIGHT_EM};
use crate::{MermaidError, MermaidOptions, MermaidRender};

use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

// ── Model ───────────────────────────────────────────────────────────────────

/// A class member — an attribute (no parens) or a method (ends in `(...)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// Full displayed text, including any visibility sigil (`+ - # ~`).
    pub text: String,
    /// `true` if this is a method (had `(...)`), `false` for an attribute.
    pub is_method: bool,
}

/// A parsed class: a name plus its attribute and method compartments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Class {
    pub name: String,
    pub attributes: Vec<Member>,
    pub methods: Vec<Member>,
}

/// Which UML marker a relationship carries and at which end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelMarker {
    /// `<|` hollow triangle (inheritance / realization).
    Triangle,
    /// `o` hollow diamond (aggregation).
    DiamondHollow,
    /// `*` filled diamond (composition).
    DiamondFilled,
    /// `>`/`<` open arrow (association / dependency).
    Arrow,
    /// Plain link, no marker.
    None,
}

/// A relationship between two classes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    /// Index/name of the left class (source as written).
    pub from: String,
    /// Index/name of the right class (target as written).
    pub to: String,
    /// The marker and the end it sits at.
    pub marker: RelMarker,
    /// `true` if the marker is at the `to` end, `false` if at the `from` end.
    pub marker_at_to: bool,
    /// Dashed line (`..`), e.g. dependency / realization.
    pub dashed: bool,
    /// Optional `: label`.
    pub label: Option<String>,
}

/// A parsed class diagram. `classes` is in first-seen order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClassDiagram {
    pub classes: Vec<Class>,
    pub relations: Vec<Relation>,
}

impl ClassDiagram {
    /// Index of `name`, inserting an empty class if not present (auto-create).
    fn ensure(&mut self, name: &str) -> usize {
        if let Some(i) = self.classes.iter().position(|c| c.name == name) {
            return i;
        }
        self.classes.push(Class {
            name: name.to_string(),
            ..Class::default()
        });
        self.classes.len() - 1
    }

    fn add_member(&mut self, class: &str, raw: &str) {
        let m = parse_member(raw);
        let i = self.ensure(class);
        if m.is_method {
            self.classes[i].methods.push(m);
        } else {
            self.classes[i].attributes.push(m);
        }
    }
}

// ── Parse ───────────────────────────────────────────────────────────────────

/// Strip a trailing generic suffix like `~T~` / `~K, V~` from a bare class name
/// (generics are skipped — the name keeps its base).
fn strip_generic(name: &str) -> &str {
    match name.find('~') {
        Some(i) => name[..i].trim(),
        None => name.trim(),
    }
}

/// Parse one member text into a [`Member`], classifying method vs attribute.
fn parse_member(raw: &str) -> Member {
    let text = raw.trim().to_string();
    // A method ends with a `)` somewhere (has a parameter list). Attributes have
    // no parentheses.
    let is_method = text.contains('(') && text.contains(')');
    Member { text, is_method }
}

/// Parse a relationship line into the two endpoint names, marker, and label.
/// Returns `None` if no relationship token is present.
fn parse_relation_line(line: &str) -> Option<Relation> {
    // Split off a trailing `: label` (after the relationship). Cardinality
    // labels in quotes are ignored.
    let (rel_part, label) = match line.split_once(':') {
        Some((l, r)) => {
            let t = r.trim();
            (l.trim(), if t.is_empty() { None } else { Some(t.to_string()) })
        }
        None => (line.trim(), None),
    };

    let (tok, marker, marker_at_to, dashed) = match_relation_earliest(rel_part)?;
    let idx = rel_part.find(tok)?;
    let left = rel_part[..idx].trim();
    let right = rel_part[idx + tok.len()..].trim();
    let left = strip_cardinality(left);
    let right = strip_cardinality(right);
    let left = strip_generic(left);
    let right = strip_generic(right);
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some(Relation {
        from: left.to_string(),
        to: right.to_string(),
        marker,
        marker_at_to,
        dashed,
        label,
    })
}

/// Drop a trailing/leading quoted cardinality like `"1"` / `"0..*"` from an
/// endpoint token, returning the bare class name.
fn strip_cardinality(s: &str) -> &str {
    let s = s.trim();
    // Endpoint may look like `Foo "1"` or `"*" Bar`. Remove quoted runs.
    if let Some(q) = s.find('"') {
        // Take whichever side of the quotes is the (unquoted) identifier.
        let before = s[..q].trim();
        if !before.is_empty() {
            return before;
        }
        // Quote leads; the name is after the closing quote.
        if let Some(end) = s[q + 1..].find('"') {
            return s[q + 1 + end + 1..].trim();
        }
    }
    s
}

/// Scan all relationship tokens and return the one whose match starts earliest
/// in `s` (ties broken by longest token), to avoid `--` shadowing `--|>` etc.
fn match_relation_earliest(s: &str) -> Option<(&'static str, RelMarker, bool, bool)> {
    const TOKENS: &[(&str, RelMarker, bool, bool)] = &[
        ("..|>", RelMarker::Triangle, true, true),
        ("<|..", RelMarker::Triangle, false, true),
        ("..>", RelMarker::Arrow, true, true),
        ("<..", RelMarker::Arrow, false, true),
        ("--|>", RelMarker::Triangle, true, false),
        ("<|--", RelMarker::Triangle, false, false),
        ("-->", RelMarker::Arrow, true, false),
        ("<--", RelMarker::Arrow, false, false),
        ("--*", RelMarker::DiamondFilled, true, false),
        ("*--", RelMarker::DiamondFilled, false, false),
        ("--o", RelMarker::DiamondHollow, true, false),
        ("o--", RelMarker::DiamondHollow, false, false),
        ("..", RelMarker::None, true, true),
        ("--", RelMarker::None, true, false),
    ];
    let mut best: Option<(usize, &'static str, RelMarker, bool, bool)> = None;
    for &(tok, marker, at_to, dashed) in TOKENS {
        if let Some(pos) = s.find(tok) {
            let take = match best {
                None => true,
                Some((bpos, btok, ..)) => pos < bpos || (pos == bpos && tok.len() > btok.len()),
            };
            if take {
                best = Some((pos, tok, marker, at_to, dashed));
            }
        }
    }
    best.map(|(_, tok, m, at, d)| (tok, m, at, d))
}

/// Parse `classDiagram` source. Errors if the header is wrong.
pub fn parse(src: &str) -> Result<ClassDiagram, String> {
    let mut lines = src.lines().map(|l| l.split("%%").next().unwrap_or("")).peekable();

    // Header: first non-blank line must start with `classDiagram`.
    let header = loop {
        match lines.next() {
            Some(l) if l.trim().is_empty() => continue,
            Some(l) => break l.trim().to_string(),
            None => return Err("empty input".to_string()),
        }
    };
    if !header.starts_with("classDiagram") {
        return Err(format!("expected `classDiagram` header, got {header:?}"));
    }

    let mut diagram = ClassDiagram::default();
    // When inside a `class X {` block, the class we are appending members to.
    let mut open_block: Option<String> = None;

    for raw in lines {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        // Inside a `{ ... }` block: each line is a member until `}`.
        if let Some(cls) = open_block.clone() {
            if line == "}" || line.starts_with('}') {
                open_block = None;
                continue;
            }
            // Skip annotation lines inside the block.
            if line.starts_with("<<") {
                continue;
            }
            diagram.add_member(&cls, line);
            continue;
        }

        // Skip standalone directives we don't model.
        if line.starts_with("direction")
            || line.starts_with("note")
            || line.starts_with("namespace")
            || line.starts_with("<<")
            || line.starts_with("click")
            || line.starts_with("style")
            || line.starts_with("cssClass")
            || line.starts_with("callback")
            || line.starts_with("link")
        {
            continue;
        }

        // A `class Name {` or `class Name` definition.
        if let Some(rest) = line.strip_prefix("class ") {
            let rest = rest.trim();
            if let Some(brace) = rest.find('{') {
                let name = strip_generic(rest[..brace].trim());
                let after = rest[brace + 1..].trim();
                let i = diagram.ensure(name);
                let _ = i;
                // Members may follow on the same line, separated, but mermaid
                // normally puts them on their own lines. If the brace closes on
                // this line too, handle inline members.
                if let Some(close) = after.find('}') {
                    let inner = after[..close].trim();
                    if !inner.is_empty() {
                        for part in inner.split(';') {
                            let p = part.trim();
                            if !p.is_empty() {
                                diagram.add_member(name, p);
                            }
                        }
                    }
                } else {
                    open_block = Some(name.to_string());
                    if !after.is_empty() {
                        diagram.add_member(name, after);
                    }
                }
            } else {
                // `class Name` (no body). Also handles `class Name:::cssClass`.
                let name = rest.split(":::").next().unwrap_or(rest);
                let name = strip_generic(name.trim());
                if !name.is_empty() {
                    diagram.ensure(name);
                }
            }
            continue;
        }

        // A member line: `ClassName : +int age`.
        // But only if there's no relationship token (relationships may also have
        // a `:` for labels). Detect a relationship token first.
        if match_relation_earliest(line).is_some() {
            if let Some(rel) = parse_relation_line(line) {
                diagram.ensure(&rel.from);
                diagram.ensure(&rel.to);
                diagram.relations.push(rel);
                continue;
            }
        }

        if let Some((lhs, rhs)) = line.split_once(':') {
            let cls = strip_generic(lhs.trim());
            let member = rhs.trim();
            if !cls.is_empty() && !member.is_empty() {
                diagram.add_member(cls, member);
            }
            continue;
        }

        // Bare class reference like `ClassName` on its own line.
        let bare = strip_generic(line);
        if !bare.is_empty() && bare.chars().all(|c| c.is_alphanumeric() || c == '_') {
            diagram.ensure(bare);
        }
        // Otherwise ignore unrecognized line.
    }

    Ok(diagram)
}

// ── Sizing ──────────────────────────────────────────────────────────────────

/// Geometry for one class box: total size + the y-offsets of compartment
/// dividers, measured from the box top.
struct BoxGeom {
    w: f32,
    h: f32,
    /// Height of the name band.
    name_h: f32,
    /// Height of the attribute band.
    attr_h: f32,
}

/// A blank compartment still gets a short band, like UML.
fn band_height(n: usize, line_h: f32, pad_y: f32) -> f32 {
    if n == 0 {
        // Empty band: half a line plus padding.
        line_h * 0.5 + pad_y
    } else {
        n as f32 * line_h + pad_y
    }
}

/// Compute the 3-compartment box geometry for a class.
fn box_geom(c: &Class, opts: &MermaidOptions) -> BoxGeom {
    let fs = opts.font_size_px;
    let line_h = fs * LINE_HEIGHT_EM;
    let pad_x = opts.node_padding_x;
    let pad_y = opts.node_padding_y;

    // Width: widest of the name and every member line.
    let mut max_w = text_size(&c.name, fs).0;
    for m in c.attributes.iter().chain(c.methods.iter()) {
        max_w = max_w.max(text_size(&m.text, fs).0);
    }
    let w = max_w + 2.0 * pad_x;

    let name_h = line_h + pad_y;
    let attr_h = band_height(c.attributes.len(), line_h, pad_y);
    let method_h = band_height(c.methods.len(), line_h, pad_y);
    let h = name_h + attr_h + method_h;

    BoxGeom { w, h, name_h, attr_h }
}

// ── Layout ──────────────────────────────────────────────────────────────────

/// A class positioned by the layout engine.
struct Positioned {
    cx: f32,
    cy: f32,
    geom: BoxGeom,
    class_idx: usize,
}

/// A routed relationship.
struct RoutedRel {
    points: Vec<(f32, f32)>,
    rel_idx: usize,
    /// Position within its parallel group (unordered endpoint pair) and the
    /// group size, used to spread overlapping edge labels.
    label_index: usize,
    label_count: usize,
    /// Dagre's reserved label center, when it positioned one for this edge.
    dagre_label: Option<Vec2>,
}

struct Layout {
    boxes: Vec<Positioned>,
    rels: Vec<RoutedRel>,
    width: f32,
    height: f32,
}

fn layout(diagram: &ClassDiagram, opts: &MermaidOptions) -> Layout {
    let geoms: Vec<BoxGeom> = diagram.classes.iter().map(|c| box_geom(c, opts)).collect();
    let node_sizes: Vec<Vec2> = geoms.iter().map(|g| Vec2::new(g.w, g.h)).collect();

    // index_of for relationship endpoints.
    let mut index_of: std::collections::HashMap<&str, u32> =
        std::collections::HashMap::with_capacity(diagram.classes.len());
    for (i, c) in diagram.classes.iter().enumerate() {
        index_of.entry(c.name.as_str()).or_insert(i as u32);
    }

    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(diagram.relations.len());
    let mut kept: Vec<usize> = Vec::with_capacity(diagram.relations.len());
    // Per-edge label box size (aligned to `edges`) so dagre reserves a gap and
    // positions the label there; None for unlabeled relationships.
    let mut label_sizes: Vec<Option<Vec2>> = Vec::with_capacity(diagram.relations.len());
    for (j, r) in diagram.relations.iter().enumerate() {
        if let (Some(&a), Some(&b)) =
            (index_of.get(r.from.as_str()), index_of.get(r.to.as_str()))
        {
            edges.push((a, b));
            kept.push(j);
            label_sizes.push(
                r.label
                    .as_deref()
                    .filter(|l| !l.is_empty())
                    .map(|l| {
                        let (w, h) = text_size(l, opts.font_size_px);
                        Vec2::new(w + 10.0, h + 6.0)
                    }),
            );
        }
    }

    let engine = LayeredEngine {
        rankdir: RankDir::Tb,
        ranksep: opts.rank_sep,
        nodesep: opts.node_sep,
        edgesep: 20.0,
        default_node_size: Vec2::new(80.0, 60.0),
    };

    let out = engine.layout(&GraphInput {
        node_count: diagram.classes.len(),
        edges: &edges,
        node_sizes: Some(&node_sizes),
        edge_label_sizes: Some(&label_sizes),
        directed: true,
    });

    let mut geoms = geoms;
    let boxes: Vec<Positioned> = (0..diagram.classes.len())
        .map(|i| {
            let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
            // move geom out
            let geom = std::mem::replace(
                &mut geoms[i],
                BoxGeom { w: 0.0, h: 0.0, name_h: 0.0, attr_h: 0.0 },
            );
            Positioned { cx: pos.x, cy: pos.y, geom, class_idx: i }
        })
        .collect();

    // Group edges by unordered endpoint pair so parallel / bidirectional
    // relationships spread their labels instead of stacking at one midpoint.
    let mut pair_members: std::collections::HashMap<(u32, u32), Vec<usize>> =
        std::collections::HashMap::new();
    for (k, &(a, b)) in edges.iter().enumerate() {
        pair_members.entry((a.min(b), a.max(b))).or_default().push(k);
    }
    let mut group = vec![(0usize, 1usize); edges.len()];
    for members in pair_members.values() {
        let cnt = members.len();
        for (idx, &k) in members.iter().enumerate() {
            group[k] = (idx, cnt);
        }
    }

    let rels: Vec<RoutedRel> = kept
        .iter()
        .enumerate()
        .map(|(dagre_idx, &orig_idx)| {
            let points: Vec<(f32, f32)> = out
                .edge_routes
                .get(dagre_idx)
                .map(|r| r.iter().map(|p| (p.x, p.y)).collect())
                .unwrap_or_default();
            let (label_index, label_count) = group[dagre_idx];
            let dagre_label = out.edge_label_positions.get(dagre_idx).copied().flatten();
            RoutedRel {
                points,
                rel_idx: orig_idx,
                label_index,
                label_count,
                dagre_label,
            }
        })
        .collect();

    Layout {
        boxes,
        rels,
        width: out.size.x,
        height: out.size.y,
    }
}

// ── Draw ────────────────────────────────────────────────────────────────────

const STROKE_W: f32 = 1.5;
/// Marker triangle / diamond / arrow length, px.
const MARK_LEN: f32 = 12.0;
const MARK_HALF: f32 = 7.0;

fn fill_attrs(color: [u8; 4]) -> (String, String) {
    (rgb(color), opacity_attr("fill-opacity", color))
}
fn stroke_attrs(color: [u8; 4]) -> (String, String) {
    (rgb(color), opacity_attr("stroke-opacity", color))
}

/// Emit one class box (rect + dividers + three text bands).
fn emit_box(svg: &mut String, b: &Positioned, class: &Class, opts: &MermaidOptions) {
    let g = &b.geom;
    let x = b.cx - g.w / 2.0;
    let y = b.cy - g.h / 2.0;
    let (fill, fo) = fill_attrs(opts.node_fill);
    let (stroke, so) = stroke_attrs(opts.node_stroke);

    // Outer rect.
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        w = g.w,
        h = g.h,
    );

    // Divider lines between compartments.
    let div1 = y + g.name_h;
    let div2 = y + g.name_h + g.attr_h;
    for dy in [div1, div2] {
        let _ = write!(
            svg,
            "<line x1=\"{x1:.2}\" y1=\"{dy:.2}\" x2=\"{x2:.2}\" y2=\"{dy:.2}\" \
             stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
            x1 = x,
            x2 = x + g.w,
        );
    }

    // Name (centered, bold) in the top band.
    let (tfill, tfo) = fill_attrs(opts.text_color);
    let family = escape(&opts.font_family);
    let fs = opts.font_size_px;
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" font-weight=\"bold\" fill=\"{tfill}\"{tfo}>{}</text>",
        escape(&class.name),
        cx = b.cx,
        cy = y + g.name_h / 2.0,
    );

    // Attributes (left-aligned) in the middle band.
    let line_h = fs * LINE_HEIGHT_EM;
    let text_x = x + opts.node_padding_x;
    let attr_top = div1;
    emit_lines(svg, &class.attributes, text_x, attr_top, line_h, opts, &tfill, &tfo, &family, fs);

    // Methods (left-aligned) in the bottom band.
    let method_top = div2;
    emit_lines(svg, &class.methods, text_x, method_top, line_h, opts, &tfill, &tfo, &family, fs);
}

#[allow(clippy::too_many_arguments)]
fn emit_lines(
    svg: &mut String,
    members: &[Member],
    x: f32,
    band_top: f32,
    line_h: f32,
    opts: &MermaidOptions,
    fill: &str,
    fo: &str,
    family: &str,
    fs: f32,
) {
    let pad_y = opts.node_padding_y;
    for (i, m) in members.iter().enumerate() {
        let cy = band_top + pad_y / 2.0 + line_h * (i as f32 + 0.5);
        let _ = write!(
            svg,
            "<text x=\"{x:.2}\" y=\"{cy:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}>{}</text>",
            escape(&m.text),
        );
    }
}

/// Pull the marker end of a polyline back by `amount` so the marker tip lands on
/// the box border. `at_to` trims the last point, else the first.
fn pullback(pts: &mut [(f32, f32)], at_to: bool, amount: f32) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let (tip_i, prev_i) = if at_to { (n - 1, n - 2) } else { (0, 1) };
    let (tx, ty) = pts[tip_i];
    let (px, py) = pts[prev_i];
    let (dx, dy) = (tx - px, ty - py);
    let len = dx.hypot(dy);
    if len <= amount || len == 0.0 {
        return;
    }
    let t = (len - amount) / len;
    pts[tip_i] = (px + dx * t, py + dy * t);
}

/// Emit a relationship polyline + its end marker + optional label.
fn emit_relation(svg: &mut String, r: &RoutedRel, rel: &Relation, opts: &MermaidOptions) {
    if r.points.len() < 2 {
        return;
    }
    let (stroke, so) = stroke_attrs(opts.edge_stroke);

    let mut pts = r.points.clone();
    // The marker sits at the from-end when `marker_at_to` is false. dagre routes
    // source→target, i.e. points[0] is `from`, last is `to`.
    let has_marker = rel.marker != RelMarker::None;
    if has_marker {
        pullback(&mut pts, rel.marker_at_to, MARK_LEN);
    }

    let mut d = String::new();
    for (i, (px, py)) in pts.iter().enumerate() {
        let cmd = if i == 0 { 'M' } else { 'L' };
        let _ = write!(d, "{cmd}{px:.2},{py:.2} ");
    }
    let dash = if rel.dashed { " stroke-dasharray=\"5 4\"" } else { "" };
    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"{dash}/>",
        d.trim_end(),
    );

    // Marker: oriented along the terminal segment at the marker end.
    if has_marker {
        // Use the un-pulled-back original points for the tip & direction.
        let (tip, prev) = if rel.marker_at_to {
            (r.points[r.points.len() - 1], r.points[r.points.len() - 2])
        } else {
            (r.points[0], r.points[1])
        };
        emit_marker(svg, rel.marker, tip, prev, opts);
    }

    // Label at dagre's reserved center when available; otherwise the route
    // midpoint, nudged perpendicular for parallel groups.
    if let Some(label) = &rel.label {
        if !label.is_empty() {
            let anchor = match r.dagre_label {
                Some(p) => Some((p.x, p.y)),
                None => {
                    edge_label_anchor(&r.points, r.label_index, r.label_count, opts.font_size_px)
                }
            };
            if let Some((mx, my)) = anchor {
                emit_label(svg, label, mx, my, opts);
            }
        }
    }
}

/// Draw the UML marker polygon at `tip`, pointing from `prev → tip`.
fn emit_marker(
    svg: &mut String,
    marker: RelMarker,
    tip: (f32, f32),
    prev: (f32, f32),
    opts: &MermaidOptions,
) {
    let (dx, dy) = (tip.0 - prev.0, tip.1 - prev.1);
    let len = dx.hypot(dy);
    let (ux, uy) = if len > 0.0 { (dx / len, dy / len) } else { (1.0, 0.0) };
    // perpendicular
    let (perpx, perpy) = (-uy, ux);

    let (stroke, so) = stroke_attrs(opts.edge_stroke);
    // Base point: back along the line from the tip by MARK_LEN.
    let base = (tip.0 - ux * MARK_LEN, tip.1 - uy * MARK_LEN);
    let half = MARK_HALF;
    let b1 = (base.0 + perpx * half, base.1 + perpy * half);
    let b2 = (base.0 - perpx * half, base.1 - perpy * half);

    match marker {
        RelMarker::Triangle => {
            // Hollow triangle (inheritance / realization): tip + two base corners,
            // filled white (node background) so it reads as hollow.
            let _ = write!(
                svg,
                "<polygon points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" \
                 fill=\"rgb(255,255,255)\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
                tip.0, tip.1, b1.0, b1.1, b2.0, b2.1,
            );
        }
        RelMarker::Arrow => {
            // Open arrow: two strokes from base corners to tip (no fill).
            let _ = write!(
                svg,
                "<polyline points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" \
                 fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
                b1.0, b1.1, tip.0, tip.1, b2.0, b2.1,
            );
        }
        RelMarker::DiamondHollow | RelMarker::DiamondFilled => {
            // Diamond: tip, side1, far corner, side2. far = base extended one more
            // MARK_LEN back.
            let far = (tip.0 - ux * 2.0 * MARK_LEN, tip.1 - uy * 2.0 * MARK_LEN);
            let fill = if marker == RelMarker::DiamondFilled {
                stroke.clone()
            } else {
                "rgb(255,255,255)".to_string()
            };
            let _ = write!(
                svg,
                "<polygon points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" \
                 fill=\"{fill}\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
                tip.0, tip.1, b1.0, b1.1, far.0, far.1, b2.0, b2.1,
            );
        }
        RelMarker::None => {}
    }
}

fn emit_label(svg: &mut String, label: &str, cx: f32, cy: f32, opts: &MermaidOptions) {
    let fs = opts.font_size_px;
    let (w, h) = text_size(label, fs);
    let pad = 2.0;
    let bw = w + 2.0 * pad;
    let bh = h + 2.0 * pad;
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{bw:.2}\" height=\"{bh:.2}\" \
         fill=\"rgb(255,255,255)\" fill-opacity=\"0.85\"/>",
        x = cx - bw / 2.0,
        y = cy - bh / 2.0,
    );
    let (tfill, tfo) = fill_attrs(opts.text_color);
    let family = escape(&opts.font_family);
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{tfill}\"{tfo}>{}</text>",
        escape(label),
    );
}

fn draw(diagram: &ClassDiagram, lay: &Layout, opts: &MermaidOptions) -> String {
    let w = (lay.width.ceil() + 1.0).max(1.0);
    let h = (lay.height.ceil() + 1.0).max(1.0);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Relationships under boxes.
    for r in &lay.rels {
        emit_relation(&mut svg, r, &diagram.relations[r.rel_idx], opts);
    }
    // Class boxes on top.
    for b in &lay.boxes {
        emit_box(&mut svg, b, &diagram.classes[b.class_idx], opts);
    }

    svg.push_str("</svg>");
    svg
}

// ── Entry point ─────────────────────────────────────────────────────────────

/// Render a mermaid `classDiagram` to SVG.
pub fn render_class(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diagram = parse(src).map_err(MermaidError::Parse)?;
    if diagram.classes.is_empty() {
        return Err(MermaidError::Empty);
    }
    let lay = layout(&diagram, opts);
    let svg = draw(&diagram, &lay, opts);
    Ok(MermaidRender {
        svg,
        width_px: (lay.width.ceil() + 1.0).max(1.0),
        height_px: (lay.height.ceil() + 1.0).max(1.0),
    })
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    // ---- parse ----

    #[test]
    fn parse_block_class() {
        let src = "classDiagram\nclass Animal {\n+int age\n+String name\n+void eat()\n}\n";
        let d = parse(src).unwrap();
        assert_eq!(d.classes.len(), 1);
        let c = &d.classes[0];
        assert_eq!(c.name, "Animal");
        assert_eq!(c.attributes.len(), 2);
        assert_eq!(c.methods.len(), 1);
        assert_eq!(c.attributes[0].text, "+int age");
        assert_eq!(c.methods[0].text, "+void eat()");
    }

    #[test]
    fn parse_member_lines() {
        let src = "classDiagram\nAnimal : +int age\nAnimal : +void eat()\n";
        let d = parse(src).unwrap();
        assert_eq!(d.classes.len(), 1);
        let c = &d.classes[0];
        assert_eq!(c.name, "Animal");
        assert_eq!(c.attributes.len(), 1);
        assert_eq!(c.methods.len(), 1);
    }

    #[test]
    fn attribute_vs_method_classification() {
        assert!(!parse_member("+int age").is_method);
        assert!(parse_member("+void eat()").is_method);
        assert!(parse_member("-doStuff(int x)").is_method);
        assert!(!parse_member("#name").is_method);
    }

    #[test]
    fn visibility_sigils_preserved() {
        let src = "classDiagram\nFoo : +pub\nFoo : -priv\nFoo : #prot\nFoo : ~pkg\n";
        let d = parse(src).unwrap();
        let texts: Vec<&str> = d.classes[0].attributes.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, vec!["+pub", "-priv", "#prot", "~pkg"]);
    }

    #[test]
    fn auto_create_classes() {
        let src = "classDiagram\nAnimal <|-- Dog\n";
        let d = parse(src).unwrap();
        assert_eq!(d.classes.len(), 2);
        assert!(d.classes.iter().any(|c| c.name == "Animal"));
        assert!(d.classes.iter().any(|c| c.name == "Dog"));
        assert_eq!(d.relations.len(), 1);
    }

    #[test]
    fn relationship_kinds_and_markers() {
        let cases = [
            ("classDiagram\nAnimal <|-- Dog\n", RelMarker::Triangle, false, false),
            ("classDiagram\nA --> B\n", RelMarker::Arrow, true, false),
            ("classDiagram\nA -- B\n", RelMarker::None, true, false),
            ("classDiagram\nA o-- B\n", RelMarker::DiamondHollow, false, false),
            ("classDiagram\nA *-- B\n", RelMarker::DiamondFilled, false, false),
            ("classDiagram\nA ..> B\n", RelMarker::Arrow, true, true),
            ("classDiagram\nA ..|> B\n", RelMarker::Triangle, true, true),
        ];
        for (src, marker, at_to, dashed) in cases {
            let d = parse(src).unwrap();
            assert_eq!(d.relations.len(), 1, "src={src}");
            let r = &d.relations[0];
            assert_eq!(r.marker, marker, "marker src={src}");
            assert_eq!(r.marker_at_to, at_to, "at_to src={src}");
            assert_eq!(r.dashed, dashed, "dashed src={src}");
        }
    }

    #[test]
    fn relationship_label() {
        let src = "classDiagram\nA --> B : uses\n";
        let d = parse(src).unwrap();
        assert_eq!(d.relations[0].label.as_deref(), Some("uses"));
    }

    #[test]
    fn dashed_detection() {
        let d = parse("classDiagram\nA ..> B\n").unwrap();
        assert!(d.relations[0].dashed);
        let d2 = parse("classDiagram\nA --> B\n").unwrap();
        assert!(!d2.relations[0].dashed);
    }

    #[test]
    fn bad_header_errors() {
        assert!(parse("flowchart TD\nA-->B").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn empty_diagram_renders_err_empty() {
        // Header but no classes.
        let r = render_class("classDiagram\n", &opts());
        assert_eq!(r, Err(MermaidError::Empty));
    }

    #[test]
    fn bad_header_render_err_parse() {
        let r = render_class("nonsense\nA-->B", &opts());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    // ---- render ----

    fn sample() -> &'static str {
        "classDiagram\n\
         class Animal {\n+int age\n+String name\n+void eat()\n}\n\
         class Dog {\n+bark()\n}\n\
         Animal <|-- Dog\n"
    }

    #[test]
    fn renders_svg_envelope() {
        let r = render_class(sample(), &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn one_box_per_class_with_compartments() {
        let r = render_class(sample(), &opts()).unwrap();
        // 2 classes → 2 outer <rect> (label backgrounds are only for edge labels;
        // none here).
        assert_eq!(r.svg.matches("<rect").count(), 2);
        // Each class has >= 2 separator lines → >= 4 total.
        assert!(r.svg.matches("<line").count() >= 4);
    }

    #[test]
    fn member_text_present() {
        let r = render_class(sample(), &opts()).unwrap();
        assert!(r.svg.contains("+int age"));
        assert!(r.svg.contains("+void eat()"));
        assert!(r.svg.contains("+bark()"));
        assert!(r.svg.contains(">Animal<"));
        assert!(r.svg.contains(">Dog<"));
    }

    #[test]
    fn one_polyline_per_relationship() {
        let r = render_class(sample(), &opts()).unwrap();
        // The relationship line is a <path fill="none">.
        assert_eq!(r.svg.matches("fill=\"none\"").count() >= 1, true);
        // Inheritance → hollow triangle polygon present.
        assert!(r.svg.contains("<polygon"));
    }

    #[test]
    fn dashed_relationship_drawn_dashed() {
        let src = "classDiagram\nclass A\nclass B\nA ..|> B\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains("stroke-dasharray"));
    }

    #[test]
    fn markers_per_kind() {
        // arrow → polyline (open), diamond → polygon, triangle → polygon.
        let arrow = render_class("classDiagram\nA --> B\n", &opts()).unwrap();
        // edge path + arrow polyline both have fill="none". Arrow is a polyline.
        assert!(arrow.svg.contains("<polyline"));

        let comp = render_class("classDiagram\nA *-- B\n", &opts()).unwrap();
        assert!(comp.svg.contains("<polygon"));
        // filled diamond uses the edge-stroke color as fill (not white).
        assert!(comp.svg.contains(&rgb(opts().edge_stroke)));
    }

    #[test]
    fn relationship_label_rendered() {
        let r = render_class("classDiagram\nA --> B : uses\n", &opts()).unwrap();
        assert!(r.svg.contains(">uses<"));
    }

    #[test]
    fn bidirectional_relationship_labels_separated() {
        // A→B and B→A both labeled: labels must land at distinct anchors.
        let src = "classDiagram\nA --> B : up\nB --> A : down\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains(">up<"));
        assert!(r.svg.contains(">down<"));

        // Read the (x, y) anchor of each label's <text> element. The two must
        // differ in at least one coordinate (the route is vertical here so the
        // perpendicular nudge moves x).
        fn label_xy(svg: &str, text: &str) -> (f32, f32) {
            let needle = format!(">{text}<");
            let at = svg.find(&needle).expect("label text present");
            let tag_start = svg[..at].rfind("<text").expect("text tag");
            let tag = &svg[tag_start..at];
            let attr = |name: &str| {
                let k = tag.find(name).expect("attr") + name.len();
                let end = tag[k..].find('"').unwrap() + k;
                tag[k..end].parse::<f32>().unwrap()
            };
            (attr("x=\""), attr("y=\""))
        }
        let up = label_xy(&r.svg, "up");
        let down = label_xy(&r.svg, "down");
        assert!(
            (up.0 - down.0).abs() > 1.0 || (up.1 - down.1).abs() > 1.0,
            "bidirectional labels overlap: up={up:?}, down={down:?}"
        );
    }

    #[test]
    fn xml_escapes_member_text() {
        let src = "classDiagram\nFoo : +Map<K,V> data\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains("+Map&lt;K,V&gt; data"));
        assert!(!r.svg.contains("+Map<K,V>"));
    }

    #[test]
    fn empty_compartments_still_have_separators() {
        // A class with no members still gets 3 compartments / 2 separators.
        let r = render_class("classDiagram\nclass Lonely\n", &opts()).unwrap();
        assert_eq!(r.svg.matches("<line").count(), 2);
        assert_eq!(r.svg.matches("<rect").count(), 1);
    }

    #[test]
    fn deterministic() {
        let a = render_class(sample(), &opts()).unwrap();
        let b = render_class(sample(), &opts()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn all_relationship_kinds_render() {
        let src = "classDiagram\n\
            A <|-- B\nA --> C\nA -- D\nA o-- E\nA *-- F\nA ..> G\nA ..|> H\n";
        let r = render_class(src, &opts()).unwrap();
        // 8 classes A..H.
        assert_eq!(r.svg.matches("<rect").count(), 8);
        assert!(r.svg.starts_with("<svg"));
    }
}
