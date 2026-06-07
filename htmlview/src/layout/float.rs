//! Float layout — real implementation (was a placeholder).
//!
//! See ARCHITECTURE.md §5. This module provides [`FloatManager`], a band model
//! that tracks how far left- and right-floats intrude into a block's content
//! area, plus placement / clearance helpers used by `block.rs` and `inline.rs`.
//!
//! ## Coordinate convention
//!
//! All float math is in **document coordinates** — the same space as every
//! [`crate::layout::LayoutBox`]'s `rect`/`content_rect`. A fresh `FloatManager`
//! is established per block formatting context (root, table cell, float,
//! inline-block) and is only ever queried with document-space `y`/`x` while that
//! block is being laid out, so storing document coords directly avoids any
//! per-block origin translation in `block.rs`.
//!
//! - `left_edge(y)` → max right edge of left floats spanning `y` (else 0/the
//!   container-left the caller clamps to).
//! - `right_edge(y, container_right)` → min left edge of right floats spanning
//!   `y` (else `container_right`).
//! - `add_left/add_right(outer_rect)` → register a placed float's margin box.
//! - `clearance(clear, y)` → lowest `y` clearing the requested side(s).
//! - `lowest_float_bottom()` → so a BFC can grow to contain its floats.
//! - `place(size, side, y, left, right)` → drop-down placement à la litehtml
//!   `place_to_left/right`.

use egui::{Pos2, Rect, Vec2};

use crate::css::values::Clear;

/// Which side a float sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// A single placed float's outer (margin) rectangle plus the side it floats to.
#[derive(Debug, Clone, Copy)]
struct PlacedFloat {
    rect: Rect,
    side: Side,
}

/// Maximum "drop down" iterations when searching for a placement / band.
/// Guarantees termination even with pathological float stacks.
const MAX_DROP_ITERS: usize = 1000;

const EPS: f32 = 0.01;

/// Tracks floats within a single block formatting context and answers band
/// queries against them. See module docs for the coordinate convention.
#[derive(Debug, Clone, Default)]
pub struct FloatManager {
    /// All placed floats, kept sorted by their top edge.
    floats: Vec<PlacedFloat>,
}

impl FloatManager {
    pub fn new() -> Self {
        Self { floats: Vec::new() }
    }

    /// Right edge of the furthest-intruding *left* float spanning `y`.
    /// Returns 0.0 when no left float covers `y` (caller clamps to container).
    pub fn left_edge(&self, y: f32) -> f32 {
        let mut edge = 0.0_f32;
        for f in &self.floats {
            if f.side == Side::Left && spans(f.rect, y) {
                edge = edge.max(f.rect.right());
            }
        }
        sanitize(edge, 0.0)
    }

    /// Left edge of the furthest-intruding *right* float spanning `y`, clamped
    /// to `container_right`. Returns `container_right` when none covers `y`.
    pub fn right_edge(&self, y: f32, container_right: f32) -> f32 {
        let mut edge = container_right;
        for f in &self.floats {
            if f.side == Side::Right && spans(f.rect, y) {
                edge = edge.min(f.rect.left());
            }
        }
        sanitize(edge, container_right)
    }

    /// Register an already-placed left float (outer rect, document coords).
    pub fn add_left(&mut self, outer: Rect) {
        self.insert(PlacedFloat { rect: outer, side: Side::Left });
    }

    /// Register an already-placed right float (outer rect, document coords).
    pub fn add_right(&mut self, outer: Rect) {
        self.insert(PlacedFloat { rect: outer, side: Side::Right });
    }

    fn insert(&mut self, pf: PlacedFloat) {
        let idx = self
            .floats
            .partition_point(|other| other.rect.top() <= pf.rect.top());
        self.floats.insert(idx, pf);
    }

    /// Lowest `y` the requested clear side(s) must drop below. Returns
    /// `max(y, bottom of the relevant floats)`.
    pub fn clearance(&self, clear: Clear, y: f32) -> f32 {
        let want_left = matches!(clear, Clear::Left | Clear::Both);
        let want_right = matches!(clear, Clear::Right | Clear::Both);
        let mut result = y;
        for f in &self.floats {
            let relevant = match f.side {
                Side::Left => want_left,
                Side::Right => want_right,
            };
            if relevant {
                result = result.max(f.rect.bottom());
            }
        }
        sanitize(result, y)
    }

    /// Lowest bottom edge among all floats, so an establishing block can grow to
    /// contain them. 0.0 when there are no floats.
    pub fn lowest_float_bottom(&self) -> f32 {
        let mut b = 0.0_f32;
        for f in &self.floats {
            b = b.max(f.rect.bottom());
        }
        sanitize(b, 0.0)
    }

    /// Smallest band boundary (float top or bottom) strictly greater than `y`,
    /// used to advance a line/float that doesn't currently fit.
    pub fn next_line_top(&self, y: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for f in &self.floats {
            for edge in [f.rect.top(), f.rect.bottom()] {
                if edge > y + EPS {
                    best = Some(match best {
                        Some(b) => b.min(edge),
                        None => edge,
                    });
                }
            }
        }
        best
    }

    /// Find the top-left placement for a new float of `size` (outer/margin size)
    /// on `side`, starting no higher than `y`, fitting within
    /// `[container_left, container_right]`. Drops down past existing floats until
    /// the band is wide enough, à la litehtml `place_to_left/right`.
    ///
    /// Returns the float's outer top-left in document coords. If the float is
    /// wider than the container (or the cap is hit), it is placed at the side
    /// edge below all existing floats so it never overlaps.
    pub fn place(
        &self,
        size: Vec2,
        side: Side,
        y: f32,
        container_left: f32,
        container_right: f32,
    ) -> Pos2 {
        let width = size.x.max(0.0);
        let mut top = y;

        for _ in 0..MAX_DROP_ITERS {
            let left = self.left_edge(top).max(container_left);
            let right = self.right_edge(top, container_right);
            if (right - left) + EPS >= width {
                let x = match side {
                    Side::Left => left,
                    Side::Right => right - width,
                };
                return Pos2::new(sanitize(x, container_left), sanitize(top, y));
            }
            match self.next_line_top(top) {
                Some(next) if next > top => top = next,
                _ => break,
            }
        }

        // Couldn't fit: place at the side edge, below everything.
        let drop_y = self.lowest_float_bottom().max(y);
        let left = self.left_edge(drop_y).max(container_left);
        let right = self.right_edge(drop_y, container_right);
        let x = match side {
            Side::Left => left,
            Side::Right => (right - width).max(container_left),
        };
        Pos2::new(sanitize(x, container_left), sanitize(drop_y, y))
    }
}

/// True if `rect` vertically spans `y` (half-open: top ≤ y < bottom). Zero-height
/// floats never span anything.
fn spans(rect: Rect, y: f32) -> bool {
    rect.height() > 0.0 && y + EPS >= rect.top() && y + EPS < rect.bottom()
}

/// Replace NaN/inf with a finite fallback. Float math must stay finite.
fn sanitize(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::parse_html;
    use crate::layout::{fonts::FontCtx, layout_document, LayoutTree};
    use egui::{Pos2, Rect, Vec2};
    use std::path::PathBuf;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    // ---- band-model unit tests --------------------------------------------

    #[test]
    fn empty_manager_no_intrusion() {
        let fm = FloatManager::new();
        assert_eq!(fm.left_edge(0.0), 0.0);
        assert_eq!(fm.right_edge(0.0, 800.0), 800.0);
        assert_eq!(fm.lowest_float_bottom(), 0.0);
        assert_eq!(fm.clearance(Clear::Both, 50.0), 50.0);
    }

    #[test]
    fn right_float_narrows_band_then_full_width_below() {
        // float:right, width 200, height 100, in an 800-wide container.
        let mut fm = FloatManager::new();
        let pos = fm.place(Vec2::new(200.0, 100.0), Side::Right, 0.0, 0.0, 800.0);
        assert!((pos.x - 600.0).abs() < 0.5, "x = {}", pos.x);
        assert_eq!(pos.y, 0.0);
        fm.add_right(rect(pos.x, pos.y, 200.0, 100.0));

        // A line at y=10 (within the float) is narrowed: right edge ≤ 600.
        assert_eq!(fm.left_edge(10.0), 0.0);
        assert!(fm.right_edge(10.0, 800.0) <= 600.5);

        // A line below the float (y=150) gets the full width.
        assert_eq!(fm.left_edge(150.0), 0.0);
        assert_eq!(fm.right_edge(150.0, 800.0), 800.0);

        // clear:both / clear:right must start at/after the float bottom (100).
        assert!(fm.clearance(Clear::Both, 0.0) >= 100.0);
        assert!(fm.clearance(Clear::Right, 0.0) >= 100.0);
        // clear:left is unaffected by a right float.
        assert_eq!(fm.clearance(Clear::Left, 0.0), 0.0);

        assert!((fm.lowest_float_bottom() - 100.0).abs() < 0.5);
    }

    #[test]
    fn two_wide_left_floats_drop_down() {
        let mut fm = FloatManager::new();
        let p1 = fm.place(Vec2::new(500.0, 50.0), Side::Left, 0.0, 0.0, 800.0);
        assert_eq!(p1, Pos2::new(0.0, 0.0));
        fm.add_left(rect(p1.x, p1.y, 500.0, 50.0));

        let p2 = fm.place(Vec2::new(500.0, 50.0), Side::Left, 0.0, 0.0, 800.0);
        assert!(p2.y >= 50.0, "p2.y = {}", p2.y);
        assert_eq!(p2.x, 0.0);
    }

    #[test]
    fn float_wider_than_container_terminates() {
        let mut fm = FloatManager::new();
        for _ in 0..5 {
            let p = fm.place(Vec2::new(800.0, 30.0), Side::Left, 0.0, 0.0, 800.0);
            fm.add_left(rect(p.x, p.y, 800.0, 30.0));
        }
        let p = fm.place(Vec2::new(1000.0, 30.0), Side::Left, 0.0, 0.0, 800.0);
        assert!(p.x.is_finite() && p.y.is_finite());
        assert!(p.y >= 0.0);
    }

    // ---- inline-integration test (deterministic, drives layout_inline) -----

    fn headless_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::default());
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        ctx
    }

    #[test]
    fn synthetic_right_float_narrows_then_full_width() {
        // A float:right box of width 200 in an 800-wide container, followed by a
        // long inline run. We register the float in the manager (as block.rs
        // would) then lay out the run through the real IFC with that manager.
        // Assert: lines beside the float are narrower (right ≤ 600), lines below
        // the float use full width, and clear:both lands below the float bottom.
        use crate::css::computed::ComputedStyle;
        use crate::dom::NodeData;
        use crate::layout::inline::{layout_inline, InlineItem};
        use crate::layout::InlineFragment;

        // Build a tiny DOM with one text node so style_for has a node to read.
        // No Stylo pass runs here, so `style_for` falls back to the CSS initial
        // style for this node (exactly what this test wants).
        let mut doc = crate::dom::Document::new();
        let n = doc.push(NodeData::Text("x".to_string()));
        doc.node_mut(n).parent = Some(doc.root);
        doc.node_mut(doc.root).children.push(n);

        let ctx = headless_ctx();
        let fonts = FontCtx::new(ctx, 1.0);
        let tree = LayoutTree::default();

        // Register a 200-wide, 120-tall right float at the right edge of [0,800].
        let mut fm = FloatManager::new();
        let place = fm.place(Vec2::new(200.0, 120.0), Side::Right, 0.0, 0.0, 800.0);
        assert!((place.x - 600.0).abs() < 0.5, "float x = {}", place.x);
        fm.add_right(rect(place.x, place.y, 200.0, 120.0));
        let float_bottom = fm.lowest_float_bottom();
        assert!((float_bottom - 120.0).abs() < 0.5);

        // A long single text run that will wrap across many lines (well past the
        // float bottom), exercising both narrowed and full-width bands.
        let text = "word ".repeat(600);
        let items = vec![InlineItem::Text { node: n, text }];
        let style = ComputedStyle::initial();
        let layout = layout_inline(
            &doc,
            &fonts,
            &tree,
            &items,
            800.0,
            &style,
            Some((&fm, 0.0, 0.0)),
        );

        let mut narrowed_beside = false;
        let mut full_below = false;
        for f in &layout.fragments {
            if let InlineFragment::Text { galley, pos, .. } = f {
                let right = pos.x + galley.size().x;
                if pos.y < float_bottom - 20.0 && right <= 601.0 {
                    narrowed_beside = true;
                }
                if pos.y > float_bottom + 5.0 && right > 601.0 {
                    full_below = true;
                }
            }
        }
        assert!(narrowed_beside, "expected narrowed lines beside the float");
        assert!(full_below, "expected full-width lines below the float");

        // clear:both must drop below the float bottom.
        assert!(fm.clearance(Clear::Both, 0.0) >= float_bottom - 0.5);
    }

    // ---- article-pipeline smoke test --------------------------------------

    struct DirProvider {
        root: PathBuf,
    }
    impl crate::ResourceProvider for DirProvider {
        fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
            let rel = url.trim_start_matches("./").trim_start_matches('/');
            let bytes = std::fs::read(self.root.join(rel)).ok()?;
            let mime = if rel.ends_with(".css") {
                "text/css".to_string()
            } else {
                "application/octet-stream".to_string()
            };
            Some((bytes, mime))
        }
    }

    fn collect_floats(tree: &LayoutTree, doc: &crate::dom::Document, idx: usize, out: &mut Vec<usize>) {
        let is_float = tree.boxes[idx]
            .node
            .map(|nid| {
                crate::layout::construct::style_for(doc, nid).float != crate::css::values::Float::None
            })
            .unwrap_or(false);
        if is_float {
            out.push(idx);
        }
        for &c in &tree.boxes[idx].children {
            collect_floats(tree, doc, c, out);
        }
    }

    #[test]
    fn article_layout_floats_finite() {
        // Re-run real article layout: must not panic, must terminate, produce a
        // sane content size. Print float stats.
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample"));
        let html = std::fs::read_to_string(dir.join("article.html")).expect("read article.html");
        let mut doc = parse_html(&html);
        let provider = DirProvider { root: dir };
        let ctx = headless_ctx();
        crate::css::stylo::style_document_stylo(&mut doc, &provider, Some("./"), crate::Theme::Light, 1000.0, Some(&ctx));

        let mut fonts = FontCtx::new(ctx, 1.0);
        let (tree, size) = layout_document(&doc, &mut fonts, 800.0, 1.0);

        assert!(size.x.is_finite() && size.y.is_finite(), "non-finite size {size:?}");
        assert!(size.x > 0.0 && size.y > 0.0, "degenerate size {size:?}");

        let root = tree.root.expect("root");
        let mut floats = Vec::new();
        collect_floats(&tree, &doc, root, &mut floats);
        let lowest = floats
            .iter()
            .map(|&i| tree.boxes[i].rect.bottom())
            .fold(0.0_f32, f32::max);
        for &i in &floats {
            assert!(tree.boxes[i].rect.is_finite(), "float rect not finite");
        }
        eprintln!(
            "[float-stats] floats placed = {}, lowest float bottom = {:.1}, content_size = {:.0}x{:.0}",
            floats.len(),
            lowest,
            size.x,
            size.y
        );
    }
}
