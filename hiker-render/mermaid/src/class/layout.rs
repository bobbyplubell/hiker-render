//! Class-diagram layout: size each class into a 3-compartment box, then place
//! boxes and route relationships with the [`hiker_graph`] layered (dagre) engine.
//! Consumes [`super::model::ClassDiagram`]; produces positioned boxes + routed
//! relationships for [`super::render`].

use crate::svgutil::{text_size, LINE_HEIGHT_EM};
use crate::MermaidOptions;
use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

use super::model;

pub(super) struct BoxGeom {
    pub(super) w: f32,
    pub(super) h: f32,
    /// Height of the name band.
    pub(super) name_h: f32,
    /// Height of the attribute band.
    pub(super) attr_h: f32,
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
fn box_geom(c: &model::Class, opts: &MermaidOptions) -> BoxGeom {
    let fs = opts.font_size_px;
    let line_h = fs * LINE_HEIGHT_EM;
    let pad_x = opts.node_padding_x;
    let pad_y = opts.node_padding_y;

    // Width: widest of the (display) name, any annotation, and every member.
    let mut max_w = text_size(&c.display_name, fs).0;
    if let Some(ann) = &c.annotation {
        max_w = max_w.max(text_size(&format!("«{ann}»"), fs).0);
    }
    for m in c.attributes.iter().chain(c.methods.iter()) {
        max_w = max_w.max(text_size(&m.text, fs).0);
    }
    let w = max_w + 2.0 * pad_x;

    // The name band gets a second line when a stereotype is present.
    let name_lines = if c.annotation.is_some() { 2.0 } else { 1.0 };
    let name_h = name_lines * line_h + pad_y;
    let attr_h = band_height(c.attributes.len(), line_h, pad_y);
    let method_h = band_height(c.methods.len(), line_h, pad_y);
    let h = name_h + attr_h + method_h;

    BoxGeom { w, h, name_h, attr_h }
}

pub(super) struct Positioned {
    pub(super) cx: f32,
    pub(super) cy: f32,
    pub(super) geom: BoxGeom,
    pub(super) class_idx: usize,
}

/// A routed relationship.
pub(super) struct RoutedRel {
    pub(super) points: Vec<(f32, f32)>,
    pub(super) rel_idx: usize,
    /// Position within its parallel group (unordered endpoint pair) and the
    /// group size, used to spread overlapping edge labels.
    pub(super) label_index: usize,
    pub(super) label_count: usize,
    /// Dagre's reserved label center, when it positioned one for this edge.
    pub(super) dagre_label: Option<Vec2>,
}

pub(super) struct Layout {
    pub(super) boxes: Vec<Positioned>,
    pub(super) rels: Vec<RoutedRel>,
    pub(super) width: f32,
    pub(super) height: f32,
}

pub(super) fn layout(diagram: &model::ClassDiagram, opts: &MermaidOptions) -> Layout {
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
        node_parents: None,
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
