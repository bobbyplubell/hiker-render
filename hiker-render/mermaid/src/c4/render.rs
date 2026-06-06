//! C4-diagram drawing: size element/boundary boxes, lay them out with the
//! [`hiker_graph`] layered (dagre) engine (boundaries via the cluster API), then
//! emit the SVG (person/system/container/component boxes, dashed boundary
//! rectangles, relationship arrows with technology labels).

use std::collections::HashMap;
use std::fmt::Write as _;

use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb};
use crate::{MermaidError, MermaidOptions, MermaidRender};

use super::model;
use super::parse::parse;

/// Boundary rectangle stroke (dashed, dark grey) and a faint themed fill.
const BOUNDARY_STROKE: [u8; 4] = [68, 68, 68, 255];
/// Boundary label / type text color.
const BOUNDARY_TEXT: [u8; 4] = [68, 68, 68, 255];

/// Vertical padding inside a box for the head/circle of a person, px.
const PERSON_HEAD_R: f32 = 9.0;
/// Per text line height factor (× font size).
const LINE_H_EM: f32 = 1.3;

/// External elements get a greyer fill to distinguish them.
pub(super) const EXTERNAL_FILL: [u8; 4] = [153, 153, 153, 255];
const EXTERNAL_STROKE: [u8; 4] = [102, 102, 102, 255];
/// Person elements get a distinct (blue-ish) fill.
pub(super) const PERSON_FILL: [u8; 4] = [8, 67, 124, 255];
const PERSON_STROKE: [u8; 4] = [7, 51, 99, 255];
/// Person/external text is light so it reads on the darker fill.
const LIGHT_TEXT: [u8; 4] = [255, 255, 255, 255];

/// The fill / stroke / text colors for an element box.
fn element_colors(elem: &model::Element, opts: &MermaidOptions) -> ([u8; 4], [u8; 4], [u8; 4]) {
    if elem.external {
        (EXTERNAL_FILL, EXTERNAL_STROKE, LIGHT_TEXT)
    } else if elem.kind == model::ElemKind::Person {
        (PERSON_FILL, PERSON_STROKE, LIGHT_TEXT)
    } else {
        (opts.node_fill, opts.node_stroke, opts.text_color)
    }
}

/// The stacked text lines of an element box: bold name, `[Type: tech]`, then the
/// wrapped description lines.
fn element_lines(elem: &model::Element) -> Vec<(String, bool)> {
    let mut lines: Vec<(String, bool)> = Vec::new();
    lines.push((elem.label.clone(), true)); // bold name
    lines.push((elem.type_label(), false));
    for l in elem.descr.split('\n') {
        if !l.is_empty() {
            lines.push((l.to_string(), false));
        }
    }
    lines
}

/// Render a mermaid `c4` diagram to SVG.
pub(super) fn render(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diag = parse(src).map_err(MermaidError::Parse)?;
    if diag.elements.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let line_h = fs * LINE_H_EM;

    // id → node index (first-seen order matches dagre node indices).
    let index_of: HashMap<&str, u32> = diag
        .elements
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.as_str(), i as u32))
        .collect();

    // Size each box from its stacked lines + padding. Persons reserve a little
    // extra headroom for the head circle.
    let sizes: Vec<(f32, f32)> = diag
        .elements
        .iter()
        .map(|e| {
            let lines = element_lines(e);
            let mut max_w = 0.0_f32;
            for (txt, _) in &lines {
                let (w, _) = crate::label::measure(txt, fs);
                max_w = max_w.max(w);
            }
            let w = max_w + 2.0 * opts.node_padding_x;
            let mut h = lines.len() as f32 * line_h + 2.0 * opts.node_padding_y;
            if e.kind == model::ElemKind::Person {
                h += PERSON_HEAD_R * 2.0;
            }
            (w.max(60.0), h.max(40.0))
        })
        .collect();

    // Build the dagre edge list (dropping any relationship whose endpoints are
    // unknown — i.e. reference an id that was never declared as an element).
    // Also build a parallel list of edge-label box sizes so dagre reserves space
    // and positions each relationship label.
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(diag.relationships.len());
    let mut kept: Vec<usize> = Vec::with_capacity(diag.relationships.len());
    let mut label_sizes: Vec<Option<Vec2>> = Vec::with_capacity(diag.relationships.len());
    for (j, r) in diag.relationships.iter().enumerate() {
        if let (Some(&a), Some(&b)) =
            (index_of.get(r.from.as_str()), index_of.get(r.to.as_str()))
        {
            edges.push((a, b));
            kept.push(j);
            let text = relationship_text(r);
            label_sizes.push(if text.is_empty() {
                None
            } else {
                let (w, h) = crate::label::measure(&text, fs);
                Some(Vec2::new(w + 10.0, h + 6.0))
            });
        }
    }

    // Dagre node list = elements (indices `0..n`) then one synthetic container
    // node per boundary (indices `n..n+b`). Container nodes get size (0,0) — the
    // engine computes their bounding rectangle from their members.
    let n = diag.elements.len();
    let b = diag.boundaries.len();
    let mut node_sizes: Vec<Vec2> = sizes.iter().map(|&(w, h)| Vec2::new(w, h)).collect();
    node_sizes.resize(n + b, Vec2::ZERO);

    // `node_parents[i]` = the dagre index of the boundary container holding node
    // `i`, or `None` for a top-level node. Built only when there are boundaries
    // (so the no-boundary path passes `None` and is byte-for-byte unchanged).
    let node_parents: Option<Vec<Option<usize>>> = if b == 0 {
        None
    } else {
        let mut parents: Vec<Option<usize>> = vec![None; n + b];
        for (j, bd) in diag.boundaries.iter().enumerate() {
            // Each member element → this boundary's container index.
            for id in &bd.member_elems {
                if let Some(&fi) = index_of.get(id.as_str()) {
                    parents[fi as usize] = Some(n + j);
                }
            }
            // Nested boundary → its parent boundary's container index.
            if let Some(p) = bd.parent {
                parents[n + j] = Some(n + p);
            }
        }
        Some(parents)
    };

    let engine = LayeredEngine {
        rankdir: RankDir::Tb,
        ranksep: opts.rank_sep,
        nodesep: opts.node_sep,
        edgesep: 20.0,
        default_node_size: Vec2::new(120.0, 60.0),
    };
    let out = engine.layout(&GraphInput {
        node_count: n + b,
        edges: &edges,
        node_sizes: Some(&node_sizes),
        edge_label_sizes: Some(&label_sizes),
        node_parents: node_parents.as_deref(),
        directed: true,
    });

    let base_width = (out.size.x.ceil() + 1.0).max(1.0);
    let base_height = (out.size.y.ceil() + 1.0).max(1.0);

    // Boundary labels are drawn ABOVE each box's top edge (using the gap that
    // already exists above nested clusters), so children never paint over them.
    // Reserve a top margin for the outermost boundary, and widen the canvas for
    // any label that extends past the right edge.
    let label_block_h = fs * LINE_H_EM + fs + 4.0;
    let mut top_pad = 0.0f32;
    let mut right_pad = 0.0f32;
    for j in 0..diag.boundaries.len() {
        let k = n + j;
        match (out.positions.get(k), out.node_sizes.get(k)) {
            (Some(c), Some(s)) if s.x > 0.0 && s.y > 0.0 => {
                let left = c.x - s.x / 2.0;
                let top = c.y - s.y / 2.0;
                top_pad = top_pad.max(label_block_h - top);
                let lw = boundary_label_width(&diag.boundaries[j], fs);
                right_pad = right_pad.max(left + lw - base_width);
            }
            _ => {}
        }
    }
    let top_pad = top_pad.max(0.0).ceil();
    let right_pad = right_pad.max(0.0).ceil();
    let width = base_width + right_pad;
    let height = base_height + top_pad;

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\">"
    );
    // Shift all diagram content down by `top_pad` so above-box boundary labels
    // (and anything at y≈0) stay on-canvas.
    let _ = write!(svg, "<g transform=\"translate(0,{top_pad:.2})\">");

    // Boundary rectangles first — behind relationships and element boxes. Read
    // back each boundary's rect (center from `out.positions`, size from
    // `out.node_sizes`) and draw outermost-first (by nesting depth) so a nested
    // boundary's rect paints over its enclosing parent.
    let mut bidx: Vec<usize> = (0..diag.boundaries.len()).collect();
    bidx.sort_by_key(|&j| boundary_depth(&diag.boundaries, j));
    for &j in &bidx {
        let k = n + j;
        let center = match out.positions.get(k).copied() {
            Some(c) => c,
            None => continue,
        };
        let size = out.node_sizes.get(k).copied().unwrap_or(Vec2::ZERO);
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        emit_boundary(&mut svg, &diag.boundaries[j], center, size, fs, opts);
    }

    // Group relationships by unordered node pair so parallel / bidirectional
    // relationships spread their labels apart.
    let mut group_total: HashMap<(String, String), usize> = HashMap::new();
    for &orig in &kept {
        let r = &diag.relationships[orig];
        *group_total.entry(pair_key(&r.from, &r.to)).or_insert(0) += 1;
    }
    let mut group_seen: HashMap<(String, String), usize> = HashMap::new();

    // Relationship polylines first, then element boxes on top.
    for (dagre_idx, &orig) in kept.iter().enumerate() {
        let r = &diag.relationships[orig];
        let pts: Vec<(f32, f32)> = out
            .edge_routes
            .get(dagre_idx)
            .map(|route| route.iter().map(|p| (p.x, p.y)).collect())
            .unwrap_or_default();
        let key = pair_key(&r.from, &r.to);
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

    for (i, e) in diag.elements.iter().enumerate() {
        let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
        let (w, h) = sizes[i];
        emit_element(&mut svg, e, pos.x, pos.y, w, h, line_h, opts);
    }

    svg.push_str("</g></svg>");

    Ok(MermaidRender {
        svg,
        width_px: width,
        height_px: height,
    })
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

/// One relationship: a polyline, an open arrowhead at the `to` end, and the
/// label (plus tech in parens) placed off the route on a light background.
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
    // Smooth curve through the route points; the open arrowhead is drawn
    // separately from the original points so it still lands on the border.
    let d = crate::svgutil::smooth_path_d(points);
    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\"{so}/>",
        d.trim_end(),
        stroke = rgb(opts.edge_stroke),
        so = opacity_attr("stroke-opacity", opts.edge_stroke),
    );

    // Open arrowhead at the destination end (last point).
    let n = points.len();
    if let Some(dir) = unit(points[n - 2], points[n - 1]) {
        draw_open_arrow(svg, points[n - 1], dir, opts);
    }

    // Label (+ tech in parens), de-collided via `edge_label_anchor`.
    let label = relationship_text(rel);
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
                 fill=\"{bg}\" fill-opacity=\"0.85\"/>",
                x = cx - bw / 2.0,
                y = cy - bh / 2.0,
                bg = rgb(opts.background),
            );
            // Relationship label is a single centered string: route it through
            // the rich-label renderer for markdown/math support.
            crate::label::emit(
                svg,
                &label,
                cx,
                cy,
                crate::label::Anchor::Middle,
                opts.font_size_px,
                opts.text_color,
                &opts.font_family,
            );
        }
    }
}

/// The drawn text for a relationship: the label with the tech appended in
/// parentheses (if present).
fn relationship_text(rel: &model::Relationship) -> String {
    match (rel.label.is_empty(), rel.tech.is_empty()) {
        (true, true) => String::new(),
        (false, true) => rel.label.clone(),
        (true, false) => format!("[{}]", rel.tech),
        (false, false) => format!("{} [{}]", rel.label, rel.tech),
    }
}

/// Draw an **open** (two-stroke V) arrowhead at `tip`, pointing along `dir`.
fn draw_open_arrow(svg: &mut String, tip: (f32, f32), dir: (f32, f32), opts: &MermaidOptions) {
    let stroke = rgb(opts.edge_stroke);
    let so = opacity_attr("stroke-opacity", opts.edge_stroke);
    let len = 10.0_f32;
    let half = 4.0_f32;
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

/// The nesting depth of boundary `j` (0 = top-level), following `parent` links.
/// Used to order drawing outermost-first.
fn boundary_depth(boundaries: &[model::Boundary], j: usize) -> usize {
    let mut depth = 0;
    let mut cur = boundaries[j].parent;
    while let Some(p) = cur {
        depth += 1;
        cur = boundaries[p].parent;
    }
    depth
}

/// One boundary: a dashed rounded rectangle with a faint themed fill, plus the
/// `«Type»` line and the boundary name stacked at the top-left.
fn emit_boundary(
    svg: &mut String,
    boundary: &model::Boundary,
    center: Vec2,
    size: Vec2,
    fs: f32,
    opts: &MermaidOptions,
) {
    let x = center.x - size.x / 2.0;
    let y = center.y - size.y / 2.0;

    // Dashed rounded rectangle, faint themed fill (background nudged so it reads
    // as a subtle tint without obscuring members).
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" rx=\"2.5\" ry=\"2.5\" \
         fill=\"{fill}\" fill-opacity=\"0.06\" stroke=\"{stroke}\"{so} stroke-width=\"1\" \
         stroke-dasharray=\"7,7\"/>",
        w = size.x,
        h = size.y,
        fill = rgb(opts.node_fill),
        stroke = rgb(BOUNDARY_STROKE),
        so = opacity_attr("stroke-opacity", BOUNDARY_STROKE),
    );

    // `«Type»` then the bold name, stacked just ABOVE the top edge (left-anchored)
    // so member boxes — which sit at the very top of the cluster — never cover
    // them. The caller reserves the headroom (top margin / sibling gap).
    let tx = x + 2.0;
    let name_y = y - fs * 0.5 - 2.0;
    let type_y = name_y - fs * LINE_H_EM;
    emit_boundary_text(svg, boundary.kind.type_label(), tx, type_y, fs, opts, None);
    emit_boundary_text(svg, &boundary.label, tx, name_y, fs, opts, Some("bold"));
}

/// Width needed for a boundary's two-line label (`«Type»` / bold name).
fn boundary_label_width(boundary: &model::Boundary, fs: f32) -> f32 {
    let type_w = crate::label::measure(boundary.kind.type_label(), fs).0;
    // Name is bold (renders a touch wider than the measured regular weight).
    let name_w = crate::label::measure(&boundary.label, fs).0 * 1.08;
    type_w.max(name_w) + 4.0
}

/// A left-anchored single-line `<text>` for boundary labels.
fn emit_boundary_text(
    svg: &mut String,
    label: &str,
    x: f32,
    y: f32,
    fs: f32,
    opts: &MermaidOptions,
    font_weight: Option<&str>,
) {
    if label.is_empty() {
        return;
    }
    let weight = font_weight
        .map(|w| format!(" font-weight=\"{w}\""))
        .unwrap_or_default();
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\"{weight} fill=\"{fill}\"{fo}>{txt}</text>",
        family = escape(&opts.font_family),
        fill = rgb(BOUNDARY_TEXT),
        fo = opacity_attr("fill-opacity", BOUNDARY_TEXT),
        txt = escape(label),
    );
}

/// One element box: optional person head circle, then the stacked centered
/// lines (bold name, `[Type: tech]`, wrapped description).
#[allow(clippy::too_many_arguments)]
fn emit_element(
    svg: &mut String,
    elem: &model::Element,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    line_h: f32,
    opts: &MermaidOptions,
) {
    let x = cx - w / 2.0;
    let y = cy - h / 2.0;
    let (fill, stroke, text_color) = element_colors(elem, opts);

    // Outer box.
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" rx=\"3\" ry=\"3\" \
         fill=\"{f}\"{fo} stroke=\"{s}\"{so} stroke-width=\"1.5\"/>",
        f = rgb(fill),
        fo = opacity_attr("fill-opacity", fill),
        s = rgb(stroke),
        so = opacity_attr("stroke-opacity", stroke),
    );

    // The stacked text lines, vertically centered within the (head-adjusted)
    // text area.
    let lines = element_lines(elem);
    let head_offset = if elem.kind == model::ElemKind::Person {
        // Little "head" circle on top to suggest a person.
        let head_cx = cx;
        let head_cy = y + PERSON_HEAD_R + 2.0;
        let _ = write!(
            svg,
            "<circle cx=\"{head_cx:.2}\" cy=\"{head_cy:.2}\" r=\"{r:.2}\" \
             fill=\"{f}\"{fo} stroke=\"{s}\"{so} stroke-width=\"1.5\"/>",
            r = PERSON_HEAD_R,
            f = rgb(fill),
            fo = opacity_attr("fill-opacity", fill),
            s = rgb(stroke),
            so = opacity_attr("stroke-opacity", stroke),
        );
        PERSON_HEAD_R * 2.0
    } else {
        0.0
    };

    let text_area_top = y + opts.node_padding_y + head_offset;
    let text_area_h = h - 2.0 * opts.node_padding_y - head_offset;
    let block_h = lines.len() as f32 * line_h;
    let mut ty = text_area_top + (text_area_h - block_h) / 2.0 + line_h * 0.5;
    for (txt, bold) in &lines {
        // The bold name line is a single centered string: route it through the
        // rich-label renderer so it supports markdown/math. Plain names keep the
        // bold `<text>` (label::emit has no default-bold, so it would otherwise
        // drop the weight); rich names render their own emphasis/math.
        if *bold && has_rich_markup(txt) {
            crate::label::emit(
                svg,
                txt,
                cx,
                ty,
                crate::label::Anchor::Middle,
                opts.font_size_px,
                text_color,
                &opts.font_family,
            );
        } else {
            let weight = if *bold { Some("bold") } else { None };
            emit_text(svg, txt, cx, ty, opts, text_color, None, weight);
        }
        ty += line_h;
    }
}

/// Cheap check for any rich-label marker (markdown emphasis, inline math, or an
/// explicit `<br>`); mirrors `crate::label`'s richness test so plain labels stay
/// on the byte-identical `<text>` path.
fn has_rich_markup(s: &str) -> bool {
    s.contains('*') || s.contains('_') || s.contains('$') || s.contains("<br")
}

/// A centered single-line `<text>` with optional `font-style` / `font-weight`.
#[allow(clippy::too_many_arguments)]
fn emit_text(
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

