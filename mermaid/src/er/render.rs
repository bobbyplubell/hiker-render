//! ER-diagram drawing: size entity boxes, lay them out with the [`hiker_graph`]
//! layered (dagre) engine, then emit the SVG (entity boxes with attribute rows,
//! relationship lines with crow's-foot cardinality markers, edge labels) and the
//! per-entity hit regions.

use std::collections::HashMap;
use std::fmt::Write as _;

use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb, text_size};
use crate::{HitRegion, MermaidError, MermaidOptions, MermaidRender};

use super::model;
use super::parse::parse;

/// Header-bar height for an entity box, px.
const HEADER_PAD_Y: f32 = 8.0;
/// Per-attribute-row height factor (× font size).
const ROW_H_EM: f32 = 1.5;

/// Shared pipeline for [`render_er`] / [`render_er_with_regions`].
pub(super) fn render(
    src: &str,
    opts: &MermaidOptions,
) -> Result<(MermaidRender, Vec<HitRegion>), MermaidError> {
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

    // Size each entity box. Attribute rows are laid out as up to 4 columns —
    // `type | name | keys | comment` — each sized to its widest cell across all
    // rows. The box width is the larger of the name header and the summed
    // columns (+ padding); height = header + attr rows.
    let sizes: Vec<(f32, f32)> = diag
        .entities
        .iter()
        .map(|e| {
            let (name_w, name_h) = text_size(&e.name, fs);
            let cols = attr_columns(e, fs);
            let rows_w = cols.iter().sum::<f32>();
            let w = name_w.max(rows_w) + 2.0 * opts.node_padding_x;
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
        node_parents: None,
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

    let mut regions: Vec<HitRegion> = Vec::with_capacity(diag.entities.len());
    for (i, e) in diag.entities.iter().enumerate() {
        let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
        let (w, h) = sizes[i];
        emit_entity(&mut svg, e, pos.x, pos.y, w, h, opts);
        regions.push(HitRegion {
            id: e.name.clone(),
            x: pos.x - w / 2.0,
            y: pos.y - h / 2.0,
            w,
            h,
            link: e.link.clone(),
            callback: e.callback.clone(),
            tooltip: e.tooltip.clone(),
        });
    }

    svg.push_str("</svg>");

    Ok((
        MermaidRender {
            svg,
            width_px: width,
            height_px: height,
        },
        regions,
    ))
}

/// Inter-cell gap between attribute columns, px.
const CELL_GAP: f32 = 12.0;

/// Compute the four attribute-column widths for an entity:
/// `[type, name, keys, comment]`. Each is the widest cell in that column across
/// all rows; empty columns (e.g. no keys/comments anywhere) collapse to 0. A
/// non-empty column carries a trailing `CELL_GAP` so columns don't touch.
fn attr_columns(e: &model::Entity, fs: f32) -> [f32; 4] {
    let mut cols = [0.0_f32; 4];
    for a in &e.attrs {
        cols[0] = cols[0].max(text_size(&a.ty, fs).0);
        cols[1] = cols[1].max(text_size(&a.name, fs).0);
        let kt = a.keys_text();
        if !kt.is_empty() {
            cols[2] = cols[2].max(text_size(&kt, fs).0);
        }
        if let Some(c) = a.comment.as_deref() {
            cols[3] = cols[3].max(text_size(c, fs).0);
        }
    }
    // Add a trailing gap to every non-empty column except the last present one.
    for w in cols.iter_mut() {
        if *w > 0.0 {
            *w += CELL_GAP;
        }
    }
    cols
}

/// One relationship: a line (dashed for non-identifying), a crow's-foot
/// cardinality marker at each entity end, and an optional label spread off the
/// midpoint so parallel relationships don't collide.
fn emit_relationship(
    svg: &mut String,
    points: &[(f32, f32)],
    rel: &model::Relationship,
    index: usize,
    count: usize,
    label_pos: Option<(f32, f32)>,
    opts: &MermaidOptions,
) {
    if points.len() < 2 {
        return;
    }
    // Smooth curve through the route points (endpoints already clipped to the
    // entity borders); markers are placed separately from the original points.
    let d = crate::svgutil::smooth_path_d(points);
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
            // Backing only when the label-bg color is opaque; a transparent
            // canvas leaves the label to read directly over the surface (no box).
            crate::svgutil::label_bg_rect(
                svg,
                cx - bw / 2.0,
                cy - bh / 2.0,
                bw,
                bh,
                0.0,
                opts.edge_label_bg,
            );
            emit_text(svg, label, cx, cy, opts, opts.text_color, "");
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
    card: model::Cardinality,
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
    e: &model::Entity,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    opts: &MermaidOptions,
) {
    let x = cx - w / 2.0;
    let y = cy - h / 2.0;
    let header_h = opts.font_size_px * 1.2 + 2.0 * HEADER_PAD_Y;

    // Per-entity style overrides, falling back to theme defaults.
    let fill_c = e.style.fill.unwrap_or(opts.node_fill);
    let stroke_c = e.style.stroke.unwrap_or(opts.node_stroke);
    let text_c = e.style.text_color.unwrap_or(opts.text_color);
    let sw = e.style.stroke_width.unwrap_or(1.5);
    // `opacity:` fades the whole box; font-weight/style/decoration ride on the
    // label `<text>` (font-size deferred for ER: it would desync row layout).
    let op = crate::svgutil::element_opacity_attr(e.style.opacity);
    let text_attrs = crate::svgutil::text_style_attrs(&e.style);

    // Outer box.
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"{op}/>",
        fill = rgb(fill_c),
        fo = opacity_attr("fill-opacity", fill_c),
        stroke = rgb(stroke_c),
        so = opacity_attr("stroke-opacity", stroke_c),
    );

    // Header name centered in the header band.
    emit_text(svg, &e.name, cx, y + header_h / 2.0, opts, text_c, &text_attrs);

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
        stroke = rgb(stroke_c),
        so = opacity_attr("stroke-opacity", stroke_c),
    );

    let row_h = opts.font_size_px * ROW_H_EM;
    let fs = opts.font_size_px;
    let cols = attr_columns(e, fs);
    // A lighter color for comment text (blend text toward fill/background).
    let comment_c = lighten(text_c);
    for (i, a) in e.attrs.iter().enumerate() {
        let row_cy = y + header_h + row_h * (i as f32 + 0.5);
        // Walk the four columns left-to-right from the inner-left padding.
        let mut tx = x + opts.node_padding_x;
        // type
        emit_cell(svg, &a.ty, tx, row_cy, opts, text_c, a.is_pk(), &text_attrs);
        tx += cols[0];
        // name
        emit_cell(svg, &a.name, tx, row_cy, opts, text_c, a.is_pk(), &text_attrs);
        tx += cols[1];
        // keys (PK/FK/UK) — emphasized like the rest of a PK row.
        if cols[2] > 0.0 {
            let kt = a.keys_text();
            if !kt.is_empty() {
                emit_cell(svg, &kt, tx, row_cy, opts, text_c, true, &text_attrs);
            }
            tx += cols[2];
        }
        // comment — lighter color.
        if cols[3] > 0.0 {
            if let Some(c) = a.comment.as_deref() {
                emit_cell(svg, c, tx, row_cy, opts, comment_c, false, &text_attrs);
            }
        }
    }
}

/// One left-aligned attribute cell at baseline-centered `(tx, cy)`. `bold`
/// renders bolder text (used for PK rows and the key markers). `extra_attrs` is
/// the entity's classDef text overrides; a style-provided `font-weight` wins
/// over the PK `bold` so the two never collide as duplicate attributes.
fn emit_cell(
    svg: &mut String,
    text: &str,
    tx: f32,
    cy: f32,
    opts: &MermaidOptions,
    color: [u8; 4],
    bold: bool,
    extra_attrs: &str,
) {
    if text.is_empty() {
        return;
    }
    let weight = if bold && !extra_attrs.contains("font-weight") {
        " font-weight=\"bold\""
    } else {
        ""
    };
    let _ = write!(
        svg,
        "<text x=\"{tx:.2}\" y=\"{cy:.2}\" text-anchor=\"start\" \
         dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{fs}\"{weight}{extra_attrs} \
         fill=\"{fill}\"{fo}>{txt}</text>",
        family = escape(&opts.font_family),
        fs = opts.font_size_px,
        fill = rgb(color),
        fo = opacity_attr("fill-opacity", color),
        txt = escape(text),
    );
}

/// A lighter variant of `c` for de-emphasized comment text (move RGB halfway
/// toward mid-gray, keep alpha).
fn lighten(c: [u8; 4]) -> [u8; 4] {
    let mix = |v: u8| ((v as u16 + 128) / 2) as u8;
    [mix(c[0]), mix(c[1]), mix(c[2]), c[3]]
}

/// A centered single-line `<text>` in the given color. `extra_attrs` is a
/// space-prefixed run of pre-built attributes (e.g. font-weight) appended onto
/// the `<text>` tag; pass `""` for none.
fn emit_text(
    svg: &mut String,
    label: &str,
    cx: f32,
    cy: f32,
    opts: &MermaidOptions,
    color: [u8; 4],
    extra_attrs: &str,
) {
    if label.is_empty() {
        return;
    }
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}{extra_attrs}>{txt}</text>",
        family = escape(&opts.font_family),
        fs = opts.font_size_px,
        fill = rgb(color),
        fo = opacity_attr("fill-opacity", color),
        txt = escape(label),
    );
}
