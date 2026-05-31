//! Block formatting context: vertical stacking of block-level boxes with a
//! simple margin-collapse model, width resolution, and delegation to the inline
//! formatting context for boxes that contain inline content.
//!
//! Margin collapse: we implement adjacent-sibling collapsing only — the margin
//! between two consecutive in-flow block siblings is `max(prev.bottom,
//! next.top)` (no negative-margin handling, no parent/first-child or
//! parent/last-child collapsing through borders/padding). This keeps vertical
//! rhythm close to a browser for typical article markup without the full
//! collapsing-through machinery. Documented deviation, intentional for v1.
//!
//! ## Floats (see ARCHITECTURE.md §5)
//!
//! A box whose computed `float` is left/right is taken OUT of normal vertical
//! flow: its shrink-to-fit width is computed, it is laid out, then placed via
//! the establishing block's [`FloatManager`] (dropping down past existing floats
//! until it fits). In-flow blocks and line boxes next to a float have their
//! available width/left-offset reduced by floats at their `y`. `clear` advances
//! a box top below the cleared floats.
//!
//! ### Float context (BFC) establishment + coordinate convention
//!
//! The [`FloatManager`] is threaded down by reference through `cb` (the
//! [`BlockCtx`]). A *fresh* manager is established by: the document root
//! (`layout_block_box` called with `cb = None`), any box with `overflow` set
//! (not yet modelled, so currently floats + inline-block via
//! [`establishes_bfc`]), floats themselves, inline-blocks, and table cells (the
//! table path calls `layout_block_box` with `cb = None`). Float rects are stored
//! in the manager in **document coordinates** — the same coordinate space as
//! every box's `rect`/`content_rect` — because a fresh manager is created per
//! establishing block and only queried with document-space y/x while that block
//! is laid out. This keeps the conversion trivial: no per-block origin subtract.

use crate::css::computed::ComputedStyle;
use crate::css::values::{
    BoxSizing, Clear, Display, Float, Length, LengthOrPercent, LengthPercentOrAuto, Position,
};
use crate::dom::{Document, NodeId};
use crate::geom::{Rect, Vec2};

use super::construct::{collect_inline_items, length_px, style_for};
use super::float::{FloatManager, Side};
use super::fonts::FontCtx;
use super::inline::layout_inline;
use super::{BoxKind, ContentSizes, LayoutTree};

/// Cap on the float "drop down" search — belt-and-braces; the FloatManager has
/// its own internal cap too. Guarantees termination on pathological pages.
const MAX_FLOAT_ITERS: usize = 1000;

/// Per-block float context threaded into child layout. Carries the establishing
/// block's [`FloatManager`] (document coords).
pub struct BlockCtx<'a> {
    pub floats: &'a mut FloatManager,
}

/// Lay out a block box at content-box origin `(x, y)` in document coords, given
/// the containing block's content width. Returns the box's used **border-box**
/// size (width, height). Sets `rect`/`content_rect` on the box and recurses.
///
/// This is the public entry (root, table cells): it establishes a *fresh* float
/// context. Children are laid out within it via [`layout_block_inner`].
pub fn layout_block_box(
    doc: &Document,
    fonts: &FontCtx,
    tree: &mut LayoutTree,
    idx: usize,
    cb_width: f32,
    x: f32,
    y: f32,
) -> Vec2 {
    let mut floats = FloatManager::new();
    let mut cb = BlockCtx { floats: &mut floats };
    // This box established the fresh manager, so it owns float-containment.
    layout_block_inner(doc, fonts, tree, idx, cb_width, x, y, &mut cb, true)
}

/// Lay out a block box within an existing float context `cb`.
///
/// `owns_floats` is true only when this box *established* the [`FloatManager`] in
/// `cb` (i.e. it was entered via [`layout_block_box`] — the root, a BFC, a float,
/// an inline-block, a table cell). Such a box grows to contain the floats in its
/// manager. Boxes that merely *share* an ancestor's manager (anonymous inline-run
/// wrappers, ordinary nested blocks) must NOT — otherwise they'd absorb the full
/// height of a sibling float and push following content below it.
#[allow(clippy::too_many_arguments)]
pub fn layout_block_inner(
    doc: &Document,
    fonts: &FontCtx,
    tree: &mut LayoutTree,
    idx: usize,
    cb_width: f32,
    x: f32,
    y: f32,
    cb: &mut BlockCtx,
    owns_floats: bool,
) -> Vec2 {
    // Tables run their own formatting context (grid + auto column widths). Cells
    // inside establish their own float context (handled in table.rs).
    if tree.boxes[idx].kind == BoxKind::Table {
        return super::table::layout_table_box(doc, fonts, tree, idx, cb_width, x, y);
    }

    let zoom = fonts.zoom();
    let style = box_style(doc, tree, idx);

    // Resolve horizontal box edges (percentages against cb_width).
    resolve_horizontal_edges(&mut tree.boxes[idx], &style, cb_width, zoom);

    let m = tree.boxes[idx].margin;
    let bp_h = tree.boxes[idx].inline_extra();

    // --- width resolution (border-box width) ---
    let avail_inner = (cb_width - m.horizontal()).max(0.0);
    let content_width = resolve_width(&style, cb_width, avail_inner, bp_h, zoom);
    let border_box_width = content_width + bp_h;

    // Content-box origin (document coords).
    let b = &tree.boxes[idx];
    let content_x = x + m.left + b.border.left + b.padding.left;
    let content_y_origin = y + m.top + b.border.top + b.padding.top;

    // --- replaced (img): intrinsic-ish size, no children layout ---
    if tree.boxes[idx].fc == super::FormattingContext::Replaced {
        let (w, h) = replaced_size(&style, content_width, zoom);
        let border_w = w + bp_h;
        let border_h = h + tree.boxes[idx].block_extra();
        set_rects(tree, idx, x + m.left, y + m.top, border_w, border_h, content_x, content_y_origin, w, h);
        return Vec2::new(border_w, border_h);
    }

    // Decide: does this block establish an IFC (all inline children) or hold
    // block children?
    let has_inline = !tree.boxes[idx].children.is_empty()
        && tree.boxes[idx]
            .children
            .iter()
            .all(|&c| is_inline_level(tree, c));
    let has_block = tree.boxes[idx]
        .children
        .iter()
        .any(|&c| !is_inline_level(tree, c));

    let content_height;

    if has_inline && !has_block {
        // --- inline formatting context ---
        // First, size atomic boxes (replaced / inline-block) so the IFC can
        // measure them — recursively, so atomics nested inside inline spans get
        // sized too (e.g. Wikipedia math `<img>` inside `<span
        // class="mwe-math-element">`). Direct-children-only sizing left those at
        // their 0×0 default, so the math rendered to a zero-size box.
        let kids: Vec<usize> = tree.boxes[idx].children.clone();
        size_inline_atomics(doc, fonts, tree, idx, content_width);
        let items = collect_inline_items(tree, doc, &kids);
        // IFC lays out relative to (0,0); float bands are queried in document
        // coords, so pass the content origin to shift the query frame.
        let layout = layout_inline(
            doc,
            fonts,
            tree,
            &items,
            content_width,
            &style,
            Some((cb.floats, content_x, content_y_origin)),
        );

        // Offset fragments + atomic boxes by the content origin (doc coords).
        let mut frags = layout.fragments;
        for f in &mut frags {
            offset_fragment(f, content_x, content_y_origin);
        }
        for (cidx, pos) in &layout.atomic_positions {
            offset_box_tree(tree, *cidx, content_x + pos.x, content_y_origin + pos.y);
        }
        tree.boxes[idx].inline_fragments = frags;
        content_height = layout.size.y;
    } else {
        // --- block formatting context: stack children vertically ---
        let kids: Vec<usize> = tree.boxes[idx].children.clone();
        let mut cursor = content_y_origin;
        let mut prev_margin_bottom = 0.0_f32;
        let mut first = true;
        for &c in &kids {
            let child_style = box_style(doc, tree, c);

            // Absolutely/fixed positioned: out of normal flow. Positioned by its
            // offsets against this block's content box, which we treat as the
            // containing block. That is exact when this block is itself
            // positioned (the common case — e.g. Wikipedia's NFPA fire diamond
            // puts `position:relative` directly on the parent of the absolute
            // cells); for a static parent it is an approximation, but crucially
            // keeps the box out of flow so it can't push siblings down. Does not
            // advance the cursor or contribute to content height.
            if matches!(child_style.position, Position::Absolute | Position::Fixed) {
                layout_abs_child(
                    doc, fonts, tree, c, &child_style, content_x, content_y_origin, content_width, zoom,
                );
                continue;
            }

            // Floats are pulled out of normal flow; they do not advance cursor.
            if child_style.float != Float::None {
                layout_float(
                    doc, fonts, tree, c, &child_style, content_x, content_width, cursor, cb, zoom,
                );
                continue;
            }

            let child_top_margin = top_margin_px(&child_style, cb_width, zoom);
            // Collapse adjacent sibling margins (max), skip before first child.
            if first {
                first = false;
            } else {
                cursor += child_top_margin.max(prev_margin_bottom) - child_top_margin;
            }

            // `clear`: advance below the cleared floats before placing.
            if child_style.clear != Clear::None {
                cursor = cb.floats.clearance(child_style.clear, cursor);
            }

            // Narrow the child to the float band available at its top.
            let band_left = cb.floats.left_edge(cursor).max(content_x);
            let band_right = cb.floats.right_edge(cursor, content_x + content_width);
            let child_x = band_left;
            let child_cb_width = (band_right - band_left).max(0.0);

            let used = if establishes_bfc(&child_style) {
                // Fresh float context: floats inside don't escape.
                layout_block_box(doc, fonts, tree, c, child_cb_width, child_x, cursor)
            } else {
                layout_block_inner(doc, fonts, tree, c, child_cb_width, child_x, cursor, cb, false)
            };
            // `used` is the border-box height; advance past it plus this child's
            // bottom margin (collapsing handled at next iteration's top).
            let child_bottom_margin = bottom_margin_px(&child_style, cb_width, zoom);
            cursor += used.y + child_bottom_margin;
            prev_margin_bottom = child_bottom_margin;
        }
        // The last bottom margin was added; subtract it so it sits outside the
        // content box.
        content_height = (cursor - prev_margin_bottom - content_y_origin).max(0.0);
    }

    // --- height resolution ---
    let explicit_h = resolve_height(&style, zoom);
    // A block that established this float manager grows to contain its floats
    // (ARCHITECTURE §5). Boxes that merely share an ancestor's manager must not,
    // or a sibling float's full height would be absorbed and push later content
    // below it. `owns_floats` is true exactly for manager-establishing boxes
    // (root, BFC, float, inline-block, table cell — all entered via
    // `layout_block_box`).
    let floats_height = if owns_floats {
        (cb.floats.lowest_float_bottom() - content_y_origin).max(0.0)
    } else {
        0.0
    };
    let auto_h = content_height.max(floats_height);
    let final_content_h = explicit_h.unwrap_or(auto_h);
    let bp_v = tree.boxes[idx].block_extra();
    let border_box_h = final_content_h + bp_v;

    set_rects(
        tree,
        idx,
        x + m.left,
        y + m.top,
        border_box_width,
        border_box_h,
        content_x,
        content_y_origin,
        content_width,
        final_content_h,
    );

    Vec2::new(border_box_width, border_box_h)
}

/// Place a float out of normal flow and register its band in `cb.floats`. Does
/// NOT advance the caller's vertical cursor.
#[allow(clippy::too_many_arguments)]
fn layout_float(
    doc: &Document,
    fonts: &FontCtx,
    tree: &mut LayoutTree,
    idx: usize,
    style: &ComputedStyle,
    content_x: f32,
    content_width: f32,
    flow_y: f32,
    cb: &mut BlockCtx,
    zoom: f32,
) {
    let side = match style.float {
        Float::Left => Side::Left,
        Float::Right => Side::Right,
        Float::None => return,
    };

    let content_left = content_x;
    let content_right = content_x + content_width;
    let avail = (content_right - content_left).max(0.0);

    // Shrink-to-fit width: explicit width wins (clamped to avail), else clamp
    // max-content to avail, floored at min-content.
    let width = match style.width {
        LengthPercentOrAuto::Length(l) => (length_px(l) * zoom).clamp(0.0, avail),
        LengthPercentOrAuto::Percent(p) => (content_width * p).clamp(0.0, avail),
        LengthPercentOrAuto::Auto => {
            let cs = intrinsic_block(doc, fonts, tree, idx);
            cs.max_content.min(avail).max(cs.min_content.min(avail)).max(0.0)
        }
    };

    // Lay the float subtree out at a provisional origin to learn its height. The
    // float establishes its OWN float context (fresh manager via layout_block_box).
    let provisional = layout_block_box(doc, fonts, tree, idx, width, content_left, flow_y);
    let outer_w = provisional.x.max(width);
    let outer_h = provisional.y.max(0.0);

    // Find the real placement, dropping down past existing floats as needed.
    let _ = MAX_FLOAT_ITERS; // documented cap; the search loop lives in fm.place.
    let place = cb.floats.place(
        egui::vec2(outer_w, outer_h),
        side,
        flow_y,
        content_left,
        content_right,
    );

    // Move the laid-out subtree to its final position (border-box top-left).
    let final_pos = if place.x.is_finite() && place.y.is_finite() {
        place
    } else {
        egui::pos2(content_left, flow_y)
    };
    offset_box_tree(tree, idx, final_pos.x, final_pos.y);

    let outer = Rect::from_min_size(final_pos, egui::vec2(outer_w.max(0.0), outer_h.max(0.0)));
    match side {
        Side::Left => cb.floats.add_left(outer),
        Side::Right => cb.floats.add_right(outer),
    }
}

/// Lay out an absolutely/fixed-positioned child out of normal flow and place it
/// by its `top`/`left`/`right`/`bottom` offsets relative to the containing
/// block's content box `(cb_x, cb_y)` of width `cb_width`. Width resolution is
/// the same shrink-to-fit as floats when `width:auto`. The box stays in its
/// parent's `children` list (so it still paints) but does not affect flow.
#[allow(clippy::too_many_arguments)]
fn layout_abs_child(
    doc: &Document,
    fonts: &FontCtx,
    tree: &mut LayoutTree,
    idx: usize,
    style: &ComputedStyle,
    cb_x: f32,
    cb_y: f32,
    cb_width: f32,
    zoom: f32,
) {
    // Width: explicit wins; auto -> shrink-to-fit clamped to the containing block.
    let width = match style.width {
        LengthPercentOrAuto::Length(l) => (length_px(l) * zoom).max(0.0),
        LengthPercentOrAuto::Percent(p) => (cb_width * p).max(0.0),
        LengthPercentOrAuto::Auto => {
            let cs = intrinsic_block(doc, fonts, tree, idx);
            cs.max_content.min(cb_width).max(cs.min_content.min(cb_width)).max(0.0)
        }
    };

    // Lay the subtree out at a provisional origin to learn its border-box size.
    let used = layout_block_box(doc, fonts, tree, idx, width, 0.0, 0.0);

    // Resolve horizontal placement: left wins; else right (from cb right edge);
    // else stay at the containing block's left.
    let resolve = |v: LengthPercentOrAuto| -> Option<f32> {
        match v {
            LengthPercentOrAuto::Length(l) => Some(length_px(l) * zoom),
            LengthPercentOrAuto::Percent(p) => Some(cb_width * p),
            LengthPercentOrAuto::Auto => None,
        }
    };
    let x = if let Some(l) = resolve(style.left) {
        cb_x + l
    } else if let Some(r) = resolve(style.right) {
        cb_x + cb_width - r - used.x
    } else {
        cb_x
    };
    // Vertical: top wins; else bottom is not resolvable without a definite cb
    // height (treated as auto -> containing-block top).
    let y = cb_y + resolve(style.top).unwrap_or(0.0);

    offset_box_tree(tree, idx, x, y);
}

/// Whether `style` establishes a new block formatting context (a fresh float
/// scope). v1: floats and inline-blocks (table cells handled via the table path
/// which calls `layout_block_box` with a fresh manager). `overflow` is not yet
/// modelled in `ComputedStyle`.
fn establishes_bfc(style: &ComputedStyle) -> bool {
    style.float != Float::None || style.display == Display::InlineBlock
}

/// Set both border-box and content-box rects on a box.
#[allow(clippy::too_many_arguments)]
fn set_rects(
    tree: &mut LayoutTree,
    idx: usize,
    bx: f32,
    by: f32,
    bw: f32,
    bh: f32,
    cx: f32,
    cy: f32,
    cw: f32,
    ch: f32,
) {
    let b = &mut tree.boxes[idx];
    b.rect = Rect::from_min_size(egui::pos2(bx, by), egui::vec2(bw.max(0.0), bh.max(0.0)));
    b.content_rect = Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw.max(0.0), ch.max(0.0)));
}

fn box_style(doc: &Document, tree: &LayoutTree, idx: usize) -> ComputedStyle {
    match tree.boxes[idx].node {
        Some(n) => style_for(doc, n),
        None => ComputedStyle::initial(),
    }
}

fn is_inline_level(tree: &LayoutTree, idx: usize) -> bool {
    matches!(
        tree.boxes[idx].kind,
        BoxKind::Inline | BoxKind::InlineBlock | BoxKind::Replaced
    ) || tree.boxes[idx].is_br
}

fn is_atomic(tree: &LayoutTree, idx: usize) -> bool {
    matches!(tree.boxes[idx].kind, BoxKind::InlineBlock | BoxKind::Replaced)
}

/// Resolve percentage-based horizontal margins/padding now that cb_width known.
fn resolve_horizontal_edges(
    b: &mut super::LayoutBox,
    style: &ComputedStyle,
    cb_width: f32,
    zoom: f32,
) {
    // Padding: percentages resolve against cb_width.
    b.padding.left = lp_px(style.padding.left, cb_width, zoom);
    b.padding.right = lp_px(style.padding.right, cb_width, zoom);
    b.padding.top = lp_px(style.padding.top, cb_width, zoom);
    b.padding.bottom = lp_px(style.padding.bottom, cb_width, zoom);
    // Margins: `auto` -> 0 here.
    b.margin.left = lpa_px(style.margin.left, cb_width, zoom);
    b.margin.right = lpa_px(style.margin.right, cb_width, zoom);
    b.margin.top = lpa_px(style.margin.top, cb_width, zoom);
    b.margin.bottom = lpa_px(style.margin.bottom, cb_width, zoom);
}

fn top_margin_px(style: &ComputedStyle, cb_width: f32, zoom: f32) -> f32 {
    lpa_px(style.margin.top, cb_width, zoom)
}
fn bottom_margin_px(style: &ComputedStyle, cb_width: f32, zoom: f32) -> f32 {
    lpa_px(style.margin.bottom, cb_width, zoom)
}

fn resolve_width(
    style: &ComputedStyle,
    cb_width: f32,
    avail_inner: f32,
    bp_h: f32,
    zoom: f32,
) -> f32 {
    let content = match style.width {
        LengthPercentOrAuto::Auto => avail_inner - bp_h,
        LengthPercentOrAuto::Length(l) => box_sizing_content(length_px(l) * zoom, bp_h, style),
        LengthPercentOrAuto::Percent(p) => box_sizing_content(cb_width * p, bp_h, style),
    }
    .max(0.0);
    // Clamp by min/max-width (these are content-box widths after box-sizing).
    let min = box_sizing_content(lp_px(style.min_width, cb_width, zoom), bp_h, style).max(0.0);
    let max = style
        .max_width
        .map(|m| box_sizing_content(lp_px(m, cb_width, zoom), bp_h, style))
        .unwrap_or(f32::INFINITY)
        .max(0.0);
    content.clamp(min.min(max), max)
}

/// If box-sizing is border-box, the given length is the border-box; convert to
/// content width by subtracting border+padding.
fn box_sizing_content(len: f32, bp_h: f32, style: &ComputedStyle) -> f32 {
    match style.box_sizing {
        BoxSizing::ContentBox => len,
        BoxSizing::BorderBox => (len - bp_h).max(0.0),
    }
}

fn resolve_height(style: &ComputedStyle, zoom: f32) -> Option<f32> {
    match style.height {
        LengthPercentOrAuto::Length(l) => Some((length_px(l) * zoom).max(0.0)),
        // Percentage heights need a definite cb height we don't track; treat
        // as auto for v1.
        LengthPercentOrAuto::Percent(_) | LengthPercentOrAuto::Auto => None,
    }
}

/// Intrinsic-ish replaced size from width/height attrs or a default box.
/// Recursively lay out (size) every atomic inline box — replaced (`<img>`,
/// `<math>`) and inline-block — in `idx`'s inline subtree, descending through
/// inline spans. Atomics are laid out at (0,0); the IFC repositions them. A
/// nested atomic that's never sized here keeps its 0×0 default (the bug this
/// fixes: math `<img>` inside `<span class="mwe-math-element">`).
fn size_inline_atomics(
    doc: &Document,
    fonts: &FontCtx,
    tree: &mut LayoutTree,
    idx: usize,
    content_width: f32,
) {
    let kids: Vec<usize> = tree.boxes[idx].children.clone();
    for c in kids {
        match tree.boxes[c].kind {
            BoxKind::Replaced | BoxKind::InlineBlock => {
                layout_block_box(doc, fonts, tree, c, content_width, 0.0, 0.0);
            }
            BoxKind::Inline => size_inline_atomics(doc, fonts, tree, c, content_width),
            _ => {}
        }
    }
}

fn replaced_size(style: &ComputedStyle, content_width: f32, zoom: f32) -> (f32, f32) {
    let w = match style.width {
        LengthPercentOrAuto::Length(l) => length_px(l) * zoom,
        LengthPercentOrAuto::Percent(p) => content_width.max(0.0) * p,
        LengthPercentOrAuto::Auto => content_width.clamp(0.0, 100.0 * zoom).max(20.0 * zoom),
    };
    let h = match style.height {
        LengthPercentOrAuto::Length(l) => length_px(l) * zoom,
        _ => (w * 0.75).max(20.0 * zoom),
    };
    (w.max(0.0), h.max(0.0))
}

fn lp_px(lp: LengthOrPercent, base: f32, zoom: f32) -> f32 {
    match lp {
        LengthOrPercent::Length(l) => length_px(l) * zoom,
        LengthOrPercent::Percent(p) => base.max(0.0) * p,
    }
}

fn lpa_px(m: LengthPercentOrAuto, base: f32, zoom: f32) -> f32 {
    match m {
        LengthPercentOrAuto::Length(l) => length_px(l) * zoom,
        LengthPercentOrAuto::Percent(p) => base.max(0.0) * p,
        LengthPercentOrAuto::Auto => 0.0,
    }
}

/// Offset an inline fragment's position by (dx, dy).
fn offset_fragment(f: &mut super::InlineFragment, dx: f32, dy: f32) {
    match f {
        super::InlineFragment::Text { pos, .. } => {
            pos.x += dx;
            pos.y += dy;
        }
        super::InlineFragment::Rect { rect, .. } => {
            *rect = rect.translate(egui::vec2(dx, dy));
        }
        super::InlineFragment::Box { .. } => {}
    }
}

/// Recursively translate a laid-out box subtree to a new top-left (border box).
/// Used to place atomic inline children and floats after their position is known.
fn offset_box_tree(tree: &mut LayoutTree, idx: usize, new_x: f32, new_y: f32) {
    let old = tree.boxes[idx].rect.min;
    let dx = new_x - old.x;
    let dy = new_y - old.y;
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    translate_box_tree(tree, idx, dx, dy);
}

fn translate_box_tree(tree: &mut LayoutTree, idx: usize, dx: f32, dy: f32) {
    {
        let b = &mut tree.boxes[idx];
        b.rect = b.rect.translate(egui::vec2(dx, dy));
        b.content_rect = b.content_rect.translate(egui::vec2(dx, dy));
        for f in &mut b.inline_fragments {
            offset_fragment(f, dx, dy);
        }
    }
    let kids = tree.boxes[idx].children.clone();
    for c in kids {
        translate_box_tree(tree, c, dx, dy);
    }
}

/// Intrinsic min/max-content sizes of a block-level subtree (rough).
pub fn intrinsic_block(
    doc: &Document,
    fonts: &FontCtx,
    tree: &LayoutTree,
    idx: usize,
) -> ContentSizes {
    let kids = tree.boxes[idx].children.clone();
    if !kids.is_empty() && kids.iter().all(|&c| is_inline_level(tree, c)) {
        let items = collect_inline_items(tree, doc, &kids);
        return super::inline::intrinsic_inline(doc, fonts, tree, &items);
    }
    let mut acc = ContentSizes::ZERO;
    for c in kids {
        acc = acc.max(intrinsic_block(doc, fonts, tree, c));
    }
    acc
}

/// Keep the `Length`/`NodeId` imports referenced if width paths shrink.
#[allow(dead_code)]
fn _markers(_l: Length, _n: NodeId) {}
