//! State-diagram drawing: size and lay out states with the [`hiker_graph`]
//! layered (dagre) engine (composites via the cluster API), then emit the SVG
//! (pseudo-states, fork/join bars, choice diamonds, composite boundary boxes,
//! transitions with arrowheads and labels, notes) plus per-state hit regions.

use std::collections::HashMap;
use std::fmt::Write as _;

use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb};
use crate::{HitRegion, MermaidError, MermaidOptions, MermaidRender};

use super::model;
use super::parse::parse;

/// Diameter of a pseudo-state circle, px.
const PSEUDO_SIZE: f32 = 18.0;
/// Length (long axis) of a fork/join bar, px.
pub(super) const FORK_LEN: f32 = 70.0;
/// Thickness (short axis) of a fork/join bar, px.
const FORK_THICK: f32 = 10.0;
/// Bounding size of a choice diamond, px.
const CHOICE_SIZE: f32 = 40.0;

/// A representative descendant node index for composite `i`, used to redirect
/// edges that target the composite (the layout engine can't rank a container as
/// an edge endpoint). Prefers `i`'s own start pseudo-state, then its first
/// non-composite child, then its first child; falls back to `i` itself.
fn composite_rep(diag: &model::StateDiagram, i: u32) -> u32 {
    if !diag.states[i as usize].composite {
        return i;
    }
    let children: Vec<u32> = diag
        .states
        .iter()
        .enumerate()
        .filter(|(_, s)| s.parent == Some(i as usize))
        .map(|(k, _)| k as u32)
        .collect();
    // Own start pseudo-state first.
    if let Some(&c) = children
        .iter()
        .find(|&&c| diag.states[c as usize].pseudo == Some(model::Pseudo::Start))
    {
        return resolve_rep(diag, c);
    }
    if let Some(&c) = children
        .iter()
        .find(|&&c| !diag.states[c as usize].composite)
    {
        return resolve_rep(diag, c);
    }
    if let Some(&c) = children.first() {
        return resolve_rep(diag, c);
    }
    i
}

/// Resolve a child to a concrete (non-composite) node, recursing into nested
/// composites so the redirected endpoint is always rankable.
fn resolve_rep(diag: &model::StateDiagram, c: u32) -> u32 {
    if diag.states[c as usize].composite {
        composite_rep(diag, c)
    } else {
        c
    }
}

/// Shared pipeline for [`render_state`] / [`render_state_with_regions`].
pub(super) fn render(
    src: &str,
    opts: &MermaidOptions,
) -> Result<(MermaidRender, Vec<HitRegion>), MermaidError> {
    let diag = parse(src).map_err(MermaidError::Parse)?;
    if diag.states.is_empty() {
        return Err(MermaidError::Empty);
    }

    // id → node index, first-seen order (matches dagre node indices).
    let index_of: HashMap<&str, u32> = diag
        .states
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i as u32))
        .collect();

    // Node sizes: real states from text + padding, pseudo-states fixed, special
    // markers fixed (bar / diamond), composites are containers sized (0,0) by
    // the engine from their members.
    let sizes: Vec<(f32, f32)> = diag
        .states
        .iter()
        .map(|s| {
            if s.composite {
                return (0.0, 0.0);
            }
            match s.kind {
                model::StateKind::Fork | model::StateKind::Join => (FORK_LEN, FORK_THICK),
                model::StateKind::Choice => (CHOICE_SIZE, CHOICE_SIZE),
                model::StateKind::Normal => match s.pseudo {
                    Some(_) => (PSEUDO_SIZE, PSEUDO_SIZE),
                    None => {
                        // Rich-aware so a state name with markdown/math is sized
                        // to its rendered width (== text_size for plain labels).
                        let (tw, th) = crate::label::measure(&s.label, opts.font_size_px);
                        (tw + 2.0 * opts.node_padding_x, th + 2.0 * opts.node_padding_y)
                    }
                },
            }
        })
        .collect();

    // The layout engine cannot rank a container (composite) node as an edge
    // endpoint, so an edge touching a composite is redirected to a representative
    // descendant: its own start pseudo-state if present, else its first child.
    // (`rep[i]` is the node index to use when an edge references composite `i`.)
    let rep: Vec<u32> = (0..diag.states.len() as u32)
        .map(|i| composite_rep(&diag, i))
        .collect();
    let remap = |i: u32| -> u32 {
        if diag.states[i as usize].composite {
            rep[i as usize]
        } else {
            i
        }
    };

    // Build the dagre edge list; keep the mapping back to original transitions.
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(diag.transitions.len());
    let mut kept: Vec<usize> = Vec::with_capacity(diag.transitions.len());
    // Per-edge label box size (aligned to `edges`) so dagre reserves a gap and
    // positions the label there; None for unlabeled transitions.
    let mut label_sizes: Vec<Option<Vec2>> = Vec::with_capacity(diag.transitions.len());
    for (j, t) in diag.transitions.iter().enumerate() {
        if let (Some(&a), Some(&b)) =
            (index_of.get(t.from.as_str()), index_of.get(t.to.as_str()))
        {
            let (a, b) = (remap(a), remap(b));
            if a == b {
                // Self-edge after redirection (e.g. a composite with one child):
                // skip to avoid a degenerate route.
                continue;
            }
            edges.push((a, b));
            kept.push(j);
            label_sizes.push(
                t.label
                    .as_deref()
                    .filter(|l| !l.is_empty())
                    .map(|l| {
                        let (w, h) = crate::label::measure(l, opts.font_size_px);
                        Vec2::new(w + 10.0, h + 6.0)
                    }),
            );
        }
    }

    let node_sizes: Vec<Vec2> = sizes.iter().map(|&(w, h)| Vec2::new(w, h)).collect();

    // Cluster wiring for composite states: a child node's `node_parents[i]` is
    // the dagre index of the composite that directly contains it. Built only
    // when there is at least one composite, so simple diagrams pass `None` and
    // keep the byte-for-byte-unchanged no-composite path.
    let has_composite = diag.states.iter().any(|s| s.composite);
    let node_parents: Option<Vec<Option<usize>>> = if has_composite {
        Some(diag.states.iter().map(|s| s.parent).collect())
    } else {
        None
    };

    let engine = LayeredEngine {
        rankdir: RankDir::Tb,
        ranksep: opts.rank_sep,
        nodesep: opts.node_sep,
        edgesep: 20.0,
        default_node_size: Vec2::new(50.0, 50.0),
    };
    let out = engine.layout(&GraphInput {
        node_count: diag.states.len(),
        edges: &edges,
        node_sizes: Some(&node_sizes),
        edge_label_sizes: Some(&label_sizes),
        node_parents: node_parents.as_deref(),
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
    emit_defs(&mut svg, opts);

    // One hit region per drawn state node (and composite), accumulated alongside
    // drawing so each rect matches its on-canvas box exactly.
    let mut regions: Vec<HitRegion> = Vec::with_capacity(diag.states.len());
    let region_for = |s: &model::State, cx: f32, cy: f32, w: f32, h: f32| HitRegion {
        id: s.id.clone(),
        x: cx - w / 2.0,
        y: cy - h / 2.0,
        w,
        h,
        link: s.link.clone(),
        callback: s.callback.clone(),
        tooltip: s.tooltip.clone(),
    };

    // Composite boundary boxes first (behind everything), largest-first so a
    // nested composite layers on top of its enclosing one. The container rect is
    // read back from the engine: center = positions[i], size = node_sizes[i].
    let mut composites: Vec<usize> = diag
        .states
        .iter()
        .enumerate()
        .filter(|(_, s)| s.composite)
        .map(|(i, _)| i)
        .collect();
    composites.sort_by(|&a, &b| {
        let area = |i: usize| {
            out.node_sizes
                .get(i)
                .map(|v| v.x * v.y)
                .unwrap_or(0.0)
        };
        area(b).total_cmp(&area(a))
    });
    for &i in &composites {
        let center = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
        let size = out.node_sizes.get(i).copied().unwrap_or(Vec2::ZERO);
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        emit_composite(&mut svg, &diag.states[i], center, size, opts);
        regions.push(region_for(&diag.states[i], center.x, center.y, size.x, size.y));
    }

    // Group edges by unordered endpoint pair so bidirectional / parallel
    // transitions spread their labels instead of stacking at one midpoint.
    let mut pair_members: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
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

    // Edges first so node fills paint over the line ends.
    for (dagre_idx, &orig) in kept.iter().enumerate() {
        let route = out.edge_routes.get(dagre_idx);
        let pts: Vec<(f32, f32)> = route
            .map(|r| r.iter().map(|p| (p.x, p.y)).collect())
            .unwrap_or_default();
        let (idx, cnt) = group[dagre_idx];
        // Dagre's reserved label center, if it placed one for this edge.
        let dagre_label = out.edge_label_positions.get(dagre_idx).copied().flatten();
        emit_edge(
            &mut svg,
            &pts,
            diag.transitions[orig].label.as_deref(),
            idx,
            cnt,
            dagre_label,
            opts,
        );
    }

    // Nodes (composites already drawn as boundary boxes above).
    for (i, s) in diag.states.iter().enumerate() {
        if s.composite {
            continue;
        }
        let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
        let (w, h) = sizes[i];
        emit_node(&mut svg, s, pos.x, pos.y, w, h, opts);
        regions.push(region_for(s, pos.x, pos.y, w, h));
    }

    // Notes: not part of the dagre graph — placed beside their target's final
    // position. Drawn last so they overlay.
    let pos_of = |id: &str| -> Option<(Vec2, (f32, f32))> {
        let &n = index_of.get(id)?;
        let i = n as usize;
        let center = out.positions.get(i).copied()?;
        Some((center, sizes[i]))
    };
    for note in &diag.notes {
        if let Some((center, (w, h))) = pos_of(&note.target) {
            emit_note(&mut svg, note, center, w, h, opts);
        }
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

/// Arrowhead marker shared by every transition.
fn emit_defs(svg: &mut String, opts: &MermaidOptions) {
    let len = 9.0_f32;
    let half = 4.0_f32;
    let _ = write!(
        svg,
        "<defs><marker id=\"state-arrow\" markerWidth=\"{len}\" markerHeight=\"{w}\" \
         refX=\"{len}\" refY=\"{half}\" orient=\"auto\" markerUnits=\"userSpaceOnUse\">\
         <path d=\"M0,0 L{len},{half} L0,{w} Z\" fill=\"{fill}\"{fo}/></marker></defs>",
        w = half * 2.0,
        fill = rgb(opts.edge_stroke),
        fo = opacity_attr("fill-opacity", opts.edge_stroke),
    );
}

/// One transition polyline (with an arrowhead and optional centered label).
/// `index`/`count` give the edge's position within its parallel group so the
/// label is nudged perpendicular to the route for bidirectional/parallel pairs.
#[allow(clippy::too_many_arguments)]
fn emit_edge(
    svg: &mut String,
    points: &[(f32, f32)],
    label: Option<&str>,
    index: usize,
    count: usize,
    dagre_label: Option<Vec2>,
    opts: &MermaidOptions,
) {
    if points.len() < 2 {
        return;
    }
    let mut pts = points.to_vec();
    pullback_end(&mut pts, 9.0);

    // Smooth curve through the (already arrowhead-shortened) points.
    let d = crate::svgutil::smooth_path_d(&pts);
    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\"{so} \
         marker-end=\"url(#state-arrow)\"/>",
        d.trim_end(),
        stroke = rgb(opts.edge_stroke),
        so = opacity_attr("stroke-opacity", opts.edge_stroke),
    );

    if let Some(label) = label.filter(|l| !l.is_empty()) {
        // Prefer dagre's reserved label center; fall back to the
        // perpendicular-nudged midpoint when dagre didn't place it.
        let anchor = match dagre_label {
            Some(p) => Some((p.x, p.y)),
            None => edge_label_anchor(points, index, count, opts.font_size_px),
        };
        if let Some((cx, cy)) = anchor {
            emit_label(svg, label, cx, cy, opts);
        }
    }
}

/// A state node: pseudo-states are circles, real states are rounded rects.
fn emit_node(
    svg: &mut String,
    s: &model::State,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    opts: &MermaidOptions,
) {
    // Special-marker shapes take precedence over the rounded-rect path.
    match s.kind {
        model::StateKind::Fork | model::StateKind::Join => {
            // A thin filled bar.
            let x = cx - w / 2.0;
            let y = cy - h / 2.0;
            let fill_c = s.style.fill.unwrap_or(opts.node_stroke);
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
                 rx=\"2\" ry=\"2\" fill=\"{fill}\"{fo}/>",
                fill = rgb(fill_c),
                fo = opacity_attr("fill-opacity", fill_c),
            );
            return;
        }
        model::StateKind::Choice => {
            // A small diamond (rotated square) centered on the node.
            let r = w / 2.0;
            let fill_c = s.style.fill.unwrap_or(opts.node_fill);
            let stroke_c = s.style.stroke.unwrap_or(opts.node_stroke);
            let sw = s.style.stroke_width.unwrap_or(1.5);
            let _ = write!(
                svg,
                "<polygon points=\"{x0:.2},{cy:.2} {cx:.2},{y0:.2} {x1:.2},{cy:.2} {cx:.2},{y1:.2}\" \
                 fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
                x0 = cx - r,
                x1 = cx + r,
                y0 = cy - r,
                y1 = cy + r,
                fill = rgb(fill_c),
                fo = opacity_attr("fill-opacity", fill_c),
                stroke = rgb(stroke_c),
                so = opacity_attr("stroke-opacity", stroke_c),
            );
            return;
        }
        model::StateKind::Normal => {}
    }
    match s.pseudo {
        Some(model::Pseudo::Start) => {
            // Small solid filled circle.
            let _ = write!(
                svg,
                "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" fill=\"{fill}\"{fo}/>",
                r = w / 2.0,
                fill = rgb(opts.node_stroke),
                fo = opacity_attr("fill-opacity", opts.node_stroke),
            );
        }
        Some(model::Pseudo::End) => {
            // Outer ring + inner solid circle.
            let _ = write!(
                svg,
                "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" fill=\"none\" \
                 stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>\
                 <circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{ri:.2}\" fill=\"{fill}\"{fo}/>",
                r = w / 2.0,
                ri = (w / 2.0 - 4.0).max(1.0),
                stroke = rgb(opts.node_stroke),
                so = opacity_attr("stroke-opacity", opts.node_stroke),
                fill = rgb(opts.node_stroke),
                fo = opacity_attr("fill-opacity", opts.node_stroke),
            );
        }
        None => {
            let x = cx - w / 2.0;
            let y = cy - h / 2.0;
            // Per-state style overrides, falling back to theme defaults.
            let fill_c = s.style.fill.unwrap_or(opts.node_fill);
            let stroke_c = s.style.stroke.unwrap_or(opts.node_stroke);
            let sw = s.style.stroke_width.unwrap_or(1.5);
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
                 rx=\"6\" ry=\"6\" fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
                fill = rgb(fill_c),
                fo = opacity_attr("fill-opacity", fill_c),
                stroke = rgb(stroke_c),
                so = opacity_attr("stroke-opacity", stroke_c),
            );
            let text_c = s.style.text_color.unwrap_or(opts.text_color);
            emit_label_colored(svg, &s.label, cx, cy, opts, text_c);
        }
    }
}

/// A composite (nested) state: a labeled rounded boundary box with a title at
/// the top-left and a faint themed fill, enclosing its laid-out children.
/// `center`/`size` are the container rect read back from the layout engine.
fn emit_composite(
    svg: &mut String,
    s: &model::State,
    center: Vec2,
    size: Vec2,
    opts: &MermaidOptions,
) {
    // Reserve a strip at the top for the title.
    let fs = opts.font_size_px;
    let title_h = fs + 6.0;
    let x = center.x - size.x / 2.0;
    let y = center.y - size.y / 2.0 - title_h;
    let w = size.x;
    let h = size.y + title_h;

    // Faint tint of the node fill so the box reads as a grouping.
    let mut fill = s.style.fill.unwrap_or(opts.node_fill);
    fill[3] = 51; // ~0.2 opacity
    let stroke_c = s.style.stroke.unwrap_or(opts.node_stroke);
    let sw = s.style.stroke_width.unwrap_or(1.5);
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         rx=\"6\" ry=\"6\" fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
        fill = rgb(fill),
        fo = opacity_attr("fill-opacity", fill),
        stroke = rgb(stroke_c),
        so = opacity_attr("stroke-opacity", stroke_c),
    );
    // Separator line under the title.
    let sep_y = y + title_h;
    let _ = write!(
        svg,
        "<line x1=\"{x:.2}\" y1=\"{sep_y:.2}\" x2=\"{x2:.2}\" y2=\"{sep_y:.2}\" \
         stroke=\"{stroke}\"{so} stroke-width=\"1\"/>",
        x2 = x + w,
        stroke = rgb(stroke_c),
        so = opacity_attr("stroke-opacity", stroke_c),
    );

    let title = s.label.trim();
    if !title.is_empty() {
        let text_c = s.style.text_color.unwrap_or(opts.text_color);
        let tx = x + 8.0;
        let ty = y + title_h / 2.0;
        let _ = write!(
            svg,
            "<text x=\"{tx:.2}\" y=\"{ty:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}>{txt}</text>",
            family = escape(&opts.font_family),
            fill = rgb(text_c),
            fo = opacity_attr("fill-opacity", text_c),
            txt = escape(title),
        );
    }
}

/// A note: a pale-filled rectangle placed beside (`pos`) its target state, with
/// the (possibly multi-line) note text. `(cx, cy)`/`(tw, th)` describe the
/// target's final box.
fn emit_note(
    svg: &mut String,
    note: &model::Note,
    target_center: Vec2,
    tw: f32,
    th: f32,
    opts: &MermaidOptions,
) {
    let fs = opts.font_size_px;
    let lines: Vec<&str> = note.text.lines().collect();
    let line_count = lines.len().max(1) as f32;
    let max_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0).max(1);
    let nw = (max_chars as f32 * fs * 0.6 + 12.0).max(40.0);
    let nh = line_count * (fs + 4.0) + 8.0;

    let gap = 12.0;
    let (nx, ny) = match note.pos {
        model::NotePos::Left => (target_center.x - tw / 2.0 - gap - nw, target_center.y - nh / 2.0),
        model::NotePos::Right => (target_center.x + tw / 2.0 + gap, target_center.y - nh / 2.0),
        model::NotePos::Over => (target_center.x - nw / 2.0, target_center.y - th / 2.0 - gap - nh),
    };

    // Pale yellow note fill (mermaid's note color), themed stroke.
    let fill = [255u8, 245, 181, 255];
    let stroke = [170u8, 170, 51, 255];
    let _ = write!(
        svg,
        "<rect x=\"{nx:.2}\" y=\"{ny:.2}\" width=\"{nw:.2}\" height=\"{nh:.2}\" \
         rx=\"2\" ry=\"2\" fill=\"{f}\" stroke=\"{s}\" stroke-width=\"1\"/>",
        f = rgb(fill),
        s = rgb(stroke),
    );
    let cx = nx + nw / 2.0;
    let line_h = fs + 4.0;
    let mut ty = ny + 4.0 + line_h / 2.0;
    for line in &lines {
        emit_label_colored(svg, line, cx, ty, opts, opts.text_color);
        ty += line_h;
    }
}

/// Centered `<text>` (single line) at `(cx, cy)` in the theme text color.
fn emit_label(svg: &mut String, label: &str, cx: f32, cy: f32, opts: &MermaidOptions) {
    emit_label_colored(svg, label, cx, cy, opts, opts.text_color);
}

/// Centered label (single string) at `(cx, cy)` in the given color, routed
/// through the rich-label renderer so state names, transition labels, and note
/// lines support markdown (`**bold**`/`*italic*`/`<br>`) and inline math
/// (`$…$`). Plain labels emit a single centered `<text>` identical to before.
fn emit_label_colored(
    svg: &mut String,
    label: &str,
    cx: f32,
    cy: f32,
    opts: &MermaidOptions,
    color: [u8; 4],
) {
    crate::label::emit(
        svg,
        label,
        cx,
        cy,
        crate::label::Anchor::Middle,
        opts.font_size_px,
        color,
        &opts.font_family,
    );
}

/// Shorten the polyline's last segment by `amount` px so an arrowhead tip lands
/// on the target border.
fn pullback_end(pts: &mut [(f32, f32)], amount: f32) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let (tx, ty) = pts[n - 1];
    let (px, py) = pts[n - 2];
    let (dx, dy) = (tx - px, ty - py);
    let len = dx.hypot(dy);
    if len <= amount || len == 0.0 {
        return;
    }
    let t = (len - amount) / len;
    pts[n - 1] = (px + dx * t, py + dy * t);
}

