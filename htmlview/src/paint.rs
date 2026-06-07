//! Paint emission: walk the laid-out tree and produce a flat display list of
//! egui shapes in paint order, plus link rectangles for hit-testing.
//!
//! All coordinates in the display list are **document coordinates** (origin at
//! the document top-left, 0,0). The host translates them by the scroll-adjusted
//! `origin` at paint time (see `HtmlView::paint`).
//!
//! Paint order per box (matches CSS painting order for the static subset we
//! support): background fill -> borders -> child boxes (recursively) -> the
//! box's own inline fragments (text / inline atomics). This keeps a box's text
//! above its own background while still letting nested block children paint
//! over the parent background.

use std::collections::HashMap;

use egui::epaint::StrokeKind;
use egui::{Color32, Pos2, Rect, Shape, Stroke, TextureId};

use crate::layout::construct::style_for;
use crate::dom::{Document, NodeData, NodeId};
use crate::geom::Edges;
use crate::layout::{BoxKind, InlineFragment, LayoutTree};

/// Resolved textures for `<img>` nodes, keyed by the image element's `NodeId`.
/// Missing entries (SVG / decode failure / no `src`) fall back to a placeholder
/// box so the image area stays visible.
pub type TextureMap = HashMap<NodeId, TextureId>;

/// A flattened display list. `shapes` are in paint order; `links` maps a
/// document-space rectangle to its `href`.
#[derive(Clone, Default)]
pub struct DisplayList {
    pub shapes: Vec<egui::Shape>,
    pub links: Vec<(egui::Rect, String)>,
    /// Per-shape visual bounding rect (document space), computed once at build
    /// so `paint()` never recomputes `visual_bounding_rect()` (expensive on text
    /// shapes) on every scroll frame.
    bboxes: Vec<egui::Rect>,
    /// Y-banded spatial index: `bands[b]` lists (in paint order) the shape
    /// indices whose bbox overlaps band `b`. Lets `paint()` touch only the
    /// shapes near the viewport instead of all N every frame.
    bands: Vec<Vec<u32>>,
    /// Shapes whose bbox is non-finite (can't be culled) — always emitted.
    always: Vec<u32>,
    band_h: f32,
    origin_y: f32,
}

impl DisplayList {
    pub fn new() -> Self {
        DisplayList::default()
    }

    /// The href of the topmost link whose rect contains `doc_point`, if any.
    pub fn link_at(&self, doc_point: egui::Pos2) -> Option<&str> {
        self.links
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(doc_point))
            .map(|(_, href)| href.as_str())
    }

    /// Walk the laid-out tree in paint order, emitting shapes (document coords)
    /// and collecting link rectangles.
    ///
    /// `textures` provides decoded raster images for `<img>` nodes; nodes
    /// without a texture get a placeholder box (so SVG / missing images stay
    /// visible). `doc` is needed to trace fragments back up to an ancestor
    /// `<a href>` for link hit-testing.
    pub fn build(
        tree: &LayoutTree,
        doc: &Document,
        textures: &TextureMap,
        page_bg: Color32,
        content_size: egui::Vec2,
    ) -> DisplayList {
        let mut dl = DisplayList::default();
        // Opaque themed page background: the very first shape, covering the whole
        // content area at the document origin. Everything else paints on top.
        if page_bg.a() != 0 && content_size.x > 0.0 && content_size.y > 0.0 {
            let rect = Rect::from_min_size(Pos2::ZERO, content_size);
            dl.shapes.push(Shape::rect_filled(rect, 0.0, page_bg));
        }
        if let Some(root) = tree.root {
            let mut b = Builder {
                tree,
                doc,
                textures,
                dl: &mut dl,
            };
            b.paint_box(root);
        }
        dl.build_index();
        dl
    }

    /// Build the per-shape bbox cache + Y-band spatial index from `shapes`.
    fn build_index(&mut self) {
        self.bboxes = self
            .shapes
            .iter()
            .map(|s| s.visual_bounding_rect())
            .collect();
        self.bands.clear();
        self.always.clear();
        let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
        for r in &self.bboxes {
            if r.min.y.is_finite() && r.max.y.is_finite() {
                min_y = min_y.min(r.min.y);
                max_y = max_y.max(r.max.y);
            }
        }
        if !min_y.is_finite() || !max_y.is_finite() {
            return; // no finite shapes → paint falls back to a linear scan
        }
        self.origin_y = min_y;
        self.band_h = 1024.0;
        let nbands = (((max_y - min_y) / self.band_h).ceil() as usize).max(1);
        self.bands = vec![Vec::new(); nbands];
        let band_of = |y: f32| {
            (((y - min_y) / self.band_h) as usize).min(nbands - 1)
        };
        for (i, r) in self.bboxes.iter().enumerate() {
            if !(r.min.y.is_finite() && r.max.y.is_finite()) {
                self.always.push(i as u32);
                continue;
            }
            let (b0, b1) = (band_of(r.min.y), band_of(r.max.y));
            for band in &mut self.bands[b0..=b1] {
                band.push(i as u32);
            }
        }
    }

    /// Paint the shapes intersecting `clip_rect` into `painter`, where `offset`
    /// maps document space to screen space (document (0,0) → `offset`). Uses the
    /// Y-band index so cost is O(visible) per frame, not O(total shapes).
    pub fn paint_into(&self, painter: &egui::Painter, offset: egui::Vec2, clip_rect: egui::Rect) {
        let emit = |i: usize| {
            let mut s = self.shapes[i].clone();
            s.translate(offset);
            painter.add(s);
        };

        // Fallback: no usable index (e.g. all shapes non-finite) → linear scan.
        if self.bands.is_empty() || self.band_h <= 0.0 {
            for i in 0..self.shapes.len() {
                let bb = self.bboxes.get(i).copied().unwrap_or(clip_rect);
                if bb.translate(offset).intersects(clip_rect) {
                    emit(i);
                }
            }
            return;
        }

        // Candidate set = shapes in the bands the viewport covers (+ uncullable).
        let nbands = self.bands.len();
        let cy0 = clip_rect.min.y - offset.y;
        let cy1 = clip_rect.max.y - offset.y;
        let band_of = |y: f32| {
            (((y - self.origin_y) / self.band_h).floor()).clamp(0.0, (nbands - 1) as f32) as usize
        };
        let (b0, b1) = (band_of(cy0), band_of(cy1));

        let mut cand: Vec<u32> = Vec::with_capacity(256);
        for band in &self.bands[b0..=b1] {
            cand.extend_from_slice(band);
        }
        cand.extend_from_slice(&self.always);
        cand.sort_unstable();
        cand.dedup();

        for &i in &cand {
            let i = i as usize;
            if self.bboxes[i].translate(offset).intersects(clip_rect) {
                emit(i);
            }
        }
    }
}

/// Light-gray placeholder fill for images we can't decode (SVG / missing).
const PLACEHOLDER_FILL: Color32 = Color32::from_rgb(0xE8, 0xE8, 0xE8);
const PLACEHOLDER_BORDER: Color32 = Color32::from_rgb(0xB0, 0xB0, 0xB0);

struct Builder<'a> {
    tree: &'a LayoutTree,
    doc: &'a Document,
    textures: &'a TextureMap,
    dl: &'a mut DisplayList,
}

impl Builder<'_> {
    fn paint_box(&mut self, idx: usize) {
        // Snapshot the small geometry fields we need so we don't hold a borrow
        // of `self.tree` across the `&mut self` paint helpers below.
        let (node, kind, rect, border) = {
            let b = &self.tree.boxes[idx];
            (b.node, b.kind, b.rect, b.border)
        };

        // 0. `opacity: 0` hides the box and its whole subtree (e.g. the
        //    visually-hidden MathML a11y span that mirrors the SVG fallback). We
        //    don't composite partial alpha — only fully-transparent is skipped.
        if let Some(n) = node {
            if style_for(self.doc, n).opacity <= 0.0 {
                return;
            }
        }

        // 1. Background fill (skip fully transparent).
        if let Some(node) = node {
            if let Some(bg) = style_for(self.doc, node).background_color {
                if bg.a() != 0 {
                    self.dl.shapes.push(Shape::rect_filled(rect, 0.0, bg));
                }
            }
        }

        // 2. Borders.
        self.paint_borders(node, rect, border);

        // 3. Replaced boxes (block/atomic <img>) paint their image into the
        //    border rect; everything else recurses + paints inlines.
        if kind == BoxKind::Replaced {
            self.paint_image(node, rect);
            self.record_link(node, rect);
            return;
        }

        // 4. Recurse into child boxes.
        let children = self.tree.boxes[idx].children.clone();
        for child in children {
            self.paint_box(child);
        }

        // 5. This box's inline fragments (text / inline atomics).
        let frags = self.tree.boxes[idx].inline_fragments.clone();
        for frag in &frags {
            self.paint_fragment(frag);
        }
    }

    fn paint_borders(&mut self, node: Option<NodeId>, rect: Rect, bw: Edges<f32>) {
        if bw.top == 0.0 && bw.right == 0.0 && bw.bottom == 0.0 && bw.left == 0.0 {
            return;
        }
        let Some(node) = node else { return };
        let colors = style_for(self.doc, node).border_color;

        // Uniform border (same width + same color all sides) -> one stroke.
        let uniform_w = bw.top == bw.right && bw.right == bw.bottom && bw.bottom == bw.left;
        let uniform_c = colors.top == colors.right
            && colors.right == colors.bottom
            && colors.bottom == colors.left;
        if uniform_w && uniform_c && bw.top > 0.0 {
            self.dl.shapes.push(Shape::rect_stroke(
                rect,
                0.0,
                Stroke::new(bw.top, colors.top),
                StrokeKind::Inside,
            ));
            return;
        }

        // Asymmetric: draw each non-zero side as a filled strip (handles
        // differing widths/colors, including butted corners).
        if bw.top > 0.0 {
            let r = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + bw.top));
            self.dl.shapes.push(Shape::rect_filled(r, 0.0, colors.top));
        }
        if bw.bottom > 0.0 {
            let r = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - bw.bottom), rect.max);
            self.dl.shapes.push(Shape::rect_filled(r, 0.0, colors.bottom));
        }
        if bw.left > 0.0 {
            let r = Rect::from_min_max(rect.min, Pos2::new(rect.min.x + bw.left, rect.max.y));
            self.dl.shapes.push(Shape::rect_filled(r, 0.0, colors.left));
        }
        if bw.right > 0.0 {
            let r = Rect::from_min_max(Pos2::new(rect.max.x - bw.right, rect.min.y), rect.max);
            self.dl.shapes.push(Shape::rect_filled(r, 0.0, colors.right));
        }
    }

    fn paint_fragment(&mut self, frag: &InlineFragment) {
        match frag {
            InlineFragment::Text {
                galley,
                pos,
                color,
                underline,
                node,
            } => {
                let rect = Rect::from_min_size(*pos, galley.size());
                self.dl
                    .shapes
                    .push(Shape::galley(*pos, galley.clone(), *color));
                if *underline {
                    let y = rect.max.y - 1.0;
                    self.dl.shapes.push(Shape::line_segment(
                        [Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)],
                        Stroke::new(1.0, *color),
                    ));
                }
                self.record_link(*node, rect);
            }
            InlineFragment::Rect { rect, color, node } => {
                self.dl.shapes.push(Shape::rect_filled(*rect, 0.0, *color));
                self.record_link(*node, *rect);
            }
            InlineFragment::Box { box_idx, .. } => {
                // Atomic inline (inline-block / inline <img>): recurse. The
                // child box paints its own background/border/image and records
                // its own link rect.
                self.paint_box(*box_idx);
            }
        }
    }

    /// Paint a raster texture for `node` into `rect`, or a placeholder box if no
    /// texture exists (SVG / decode failure / missing src).
    fn paint_image(&mut self, node: Option<NodeId>, rect: Rect) {
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        let tex = node.and_then(|n| self.textures.get(&n)).copied();
        if let Some(tex_id) = tex {
            let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            self.dl
                .shapes
                .push(Shape::image(tex_id, rect, uv, Color32::WHITE));
        } else {
            self.dl
                .shapes
                .push(Shape::rect_filled(rect, 0.0, PLACEHOLDER_FILL));
            self.dl.shapes.push(Shape::rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, PLACEHOLDER_BORDER),
                StrokeKind::Inside,
            ));
        }
    }

    /// If `node` (or an ancestor) is an `<a href>`, record `(rect, href)`.
    fn record_link(&mut self, node: Option<NodeId>, rect: Rect) {
        let Some(start) = node else { return };
        let mut cur = Some(start);
        while let Some(id) = cur {
            let Some(n) = self.doc.nodes.get(id) else { break };
            if let NodeData::Element { name, .. } = &n.data {
                if name == "a" {
                    if let Some(href) = n.attr("href") {
                        if !href.is_empty() {
                            self.dl.links.push((rect, href.to_string()));
                            return;
                        }
                    }
                }
            }
            cur = n.parent;
        }
    }
}

/// Resolve a (possibly relative) subresource `href` against `base_url`.
/// Mirrors the directory-backed provider convention used by the tests/example:
/// strip a leading `./`, and if the href is relative, prefix it with the base.
/// Absolute (`http(s)://`, `//`, `data:`, root `/`) URLs pass through unchanged.
pub fn resolve_url(base_url: Option<&str>, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://")
        || href.starts_with("https://")
        || href.starts_with("//")
        || href.starts_with("data:")
        || href.starts_with('/')
    {
        return href.to_string();
    }
    match base_url {
        Some(base) => {
            let base = base.trim_end_matches('/');
            let rel = href.trim_start_matches("./");
            if base.is_empty() || base == "." {
                rel.to_string()
            } else {
                format!("{base}/{rel}")
            }
        }
        None => href.to_string(),
    }
}
