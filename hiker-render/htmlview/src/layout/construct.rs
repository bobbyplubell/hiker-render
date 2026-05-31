//! Box-tree construction: walk the styled DOM and build a tree of
//! [`crate::layout::LayoutBox`]es, applying anonymous-box fixup so every block
//! container holds EITHER block-level children OR a single inline formatting
//! context (never mixed).
//!
//! Margins/padding/border are resolved to px here (×zoom). Width/height and
//! percentage edges stay symbolic on the style and are resolved during layout
//! against the containing block.

use crate::css::computed::ComputedStyle;
use crate::css::values::{Display, Length, LengthOrPercent, LengthPercentOrAuto};
use crate::dom::{Document, NodeData, NodeId};
use crate::geom::{Edges, Rect};

use super::fonts::FontCtx;
use super::inline::InlineItem;
use super::{BoxKind, FormattingContext, LayoutBox, LayoutTree};

/// Build a layout-box tree rooted at the rendered subtree of `doc`.
///
/// Returns the index of the root box in `tree.boxes`, or `None` if nothing is
/// rendered. The root box is a block container.
pub fn build_tree(doc: &Document, fonts: &FontCtx, tree: &mut LayoutTree) -> Option<usize> {
    // Prefer <body>; otherwise fall back to the document root's subtree.
    let start = find_body(doc).unwrap_or(doc.root);
    let zoom = fonts.zoom();

    // The root box is an anonymous block wrapping the body's block content so
    // the entry point always has a single block container to lay out.
    let root_idx = tree.boxes.len();
    tree.boxes.push(LayoutBox::new_anon(FormattingContext::Block, BoxKind::Block));

    // The body element itself becomes the single child block of the root.
    if let Some(child) = build_box(doc, start, zoom, tree) {
        tree.boxes[root_idx].children.push(child);
    }
    Some(root_idx)
}

/// Locate the `<body>` element, if any.
fn find_body(doc: &Document) -> Option<NodeId> {
    doc.nodes
        .iter()
        .find(|n| n.tag() == Some("body"))
        .map(|n| n.id)
}

/// Effective display for a node: text nodes are inline; elements use their
/// computed `display`. `None` means the node generates no box.
fn node_display(doc: &Document, node: NodeId) -> Option<Display> {
    let n = doc.node(node);
    match &n.data {
        NodeData::Text(text) => {
            // Whitespace-only text between block siblings is dropped later by
            // the inline tokenizer; keep the box here unless truly empty.
            if text.is_empty() {
                None
            } else {
                Some(Display::Inline)
            }
        }
        NodeData::Element { .. } => {
            let d = style_for(doc, node).display;
            if d == Display::None {
                None
            } else {
                Some(d)
            }
        }
        // Comments / doctype / document never generate boxes here.
        _ => None,
    }
}

/// Whether a display value is block-level (participates in a BFC as a block).
fn is_block_level(d: Display) -> bool {
    matches!(
        d,
        Display::Block
            | Display::ListItem
            | Display::Table
            | Display::TableRowGroup
            | Display::TableHeaderGroup
            | Display::TableFooterGroup
            | Display::TableRow
            | Display::TableCell
            | Display::TableColumn
            | Display::TableColumnGroup
            | Display::TableCaption
            | Display::Flex
    )
}

/// Build a box for `node` and its subtree. Returns the new box index, or `None`
/// if the node generates no box (display:none, empty text, etc.).
pub(super) fn build_box(doc: &Document, node: NodeId, zoom: f32, tree: &mut LayoutTree) -> Option<usize> {
    let display = node_display(doc, node)?;
    let style_owner = style_for(doc, node);

    // --- replaced elements (img, math) ---
    // `<math>` is sized as a replaced element via the SVG pre-render pass (its
    // computed width/height are stamped onto the style before layout). We do NOT
    // recurse into its MathML children, so stray child text never renders.
    if matches!(doc.node(node).tag(), Some("img") | Some("math")) {
        let kind = if is_block_level(display) {
            BoxKind::Block
        } else {
            BoxKind::Replaced
        };
        let fc = FormattingContext::Replaced;
        let mut b = LayoutBox::new(node, fc, kind);
        resolve_edges(&mut b, doc, node, zoom);
        let idx = tree.boxes.len();
        tree.boxes.push(b);
        return Some(idx);
    }

    // --- <br>: forced line break, only meaningful inside an IFC ---
    if doc.node(node).tag() == Some("br") {
        let mut b = LayoutBox::new(node, FormattingContext::Inline, BoxKind::Inline);
        b.is_br = true;
        let idx = tree.boxes.len();
        tree.boxes.push(b);
        return Some(idx);
    }

    let kind = box_kind_for(display);
    // Tables/floats are stubbed: lay table boxes out as plain blocks for now,
    // but tag the kind so a later agent can dispatch on it.
    let fc = match display {
        Display::Inline => FormattingContext::Inline,
        Display::InlineBlock => FormattingContext::Block,
        _ if is_block_level(display) => FormattingContext::Block,
        _ => FormattingContext::Inline,
    };

    let mut b = LayoutBox::new(node, fc, kind);
    let _ = &style_owner;
    resolve_edges(&mut b, doc, node, zoom);
    let idx = tree.boxes.len();
    tree.boxes.push(b);

    // Build children, then apply anonymous-box fixup on the resulting list.
    let children = build_children(doc, node, zoom, tree);
    let fixed = anon_fixup(children, tree, doc, zoom);
    tree.boxes[idx].children = fixed;

    Some(idx)
}

/// Build child boxes (no fixup yet) for `node`.
fn build_children(doc: &Document, node: NodeId, zoom: f32, tree: &mut LayoutTree) -> Vec<usize> {
    let kids = doc.node(node).children.clone();
    let mut out = Vec::new();
    for c in kids {
        if let Some(b) = build_box(doc, c, zoom, tree) {
            out.push(b);
        }
    }
    out
}

/// Map a `display` to a coarse [`BoxKind`].
fn box_kind_for(d: Display) -> BoxKind {
    match d {
        Display::Inline => BoxKind::Inline,
        Display::InlineBlock => BoxKind::InlineBlock,
        Display::Table => BoxKind::Table,
        Display::TableRow => BoxKind::TableRow,
        Display::TableCell => BoxKind::TableCell,
        Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup => {
            BoxKind::TableRowGroup
        }
        _ => BoxKind::Block,
    }
}

/// Whether a child box is inline-level (participates in an IFC).
fn is_inline_box(tree: &LayoutTree, idx: usize) -> bool {
    matches!(
        tree.boxes[idx].kind,
        BoxKind::Inline | BoxKind::InlineBlock | BoxKind::Replaced
    )
}

/// Anonymous-box fixup. If a block container's children mix block-level and
/// inline-level boxes, wrap each maximal run of inline-level children in an
/// anonymous block box, so the container becomes all-block. If the children are
/// *all* inline, leave them as-is (the container is a single IFC). If all block,
/// leave them as-is.
fn anon_fixup(
    children: Vec<usize>,
    tree: &mut LayoutTree,
    doc: &Document,
    _zoom: f32,
) -> Vec<usize> {
    // An out-of-flow child (position:absolute/fixed) participates in NEITHER the
    // inline nor the block flow of its container, so it must not trigger anon
    // block wrapping. Wikipedia's hidden math a11y span is `position:absolute`,
    // which Stylo blockifies to display:block; without this guard it would force
    // its sibling math `<img>` into an anon block and break the img's sizing.
    let inline_level = |tree: &LayoutTree, c: usize| {
        is_out_of_flow(tree, doc, c) || is_inline_box(tree, c)
    };
    let any_block = children.iter().any(|&c| !inline_level(tree, c));
    let any_inline = children.iter().any(|&c| inline_level(tree, c));
    if !(any_block && any_inline) {
        return children;
    }

    let mut out: Vec<usize> = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    for c in children {
        if inline_level(tree, c) {
            run.push(c);
        } else {
            flush_anon_run(&mut run, &mut out, tree);
            out.push(c);
        }
    }
    flush_anon_run(&mut run, &mut out, tree);
    out
}

/// Whether `idx`'s box is out of normal flow (`position: absolute`/`fixed`) — it
/// is positioned independently and so participates in neither inline nor block
/// flow of its container.
fn is_out_of_flow(tree: &LayoutTree, doc: &Document, idx: usize) -> bool {
    use crate::css::values::Position;
    tree.boxes[idx]
        .node
        .map_or(false, |n| matches!(style_for(doc, n).position, Position::Absolute | Position::Fixed))
}

/// Wrap a pending run of inline-level boxes in an anonymous block box.
fn flush_anon_run(run: &mut Vec<usize>, out: &mut Vec<usize>, tree: &mut LayoutTree) {
    if run.is_empty() {
        return;
    }
    let anon = LayoutBox::new_anon(FormattingContext::Inline, BoxKind::Block);
    let idx = tree.boxes.len();
    tree.boxes.push(anon);
    tree.boxes[idx].children = std::mem::take(run);
    out.push(idx);
}

/// Resolve margin/padding/border to px (×zoom). Percentages and `auto` are kept
/// in the style and resolved during layout; here we only fill the absolute
/// border widths and the absolute parts of margin/padding (percentages become
/// 0 as a placeholder, then layout overrides them via `style`).
fn resolve_edges(b: &mut LayoutBox, doc: &Document, node: NodeId, zoom: f32) {
    let style = style_for(doc, node);
    b.border = style.border_width.map(|w| w * zoom);
    // Absolute px parts only; percentages resolved against cb width at layout.
    b.padding = style.padding.map(|lp| resolve_lp_abs(lp, zoom));
    b.margin = style.margin.map(|m| resolve_lpa_abs(m, zoom));
}

/// Resolve the absolute (px/em/rem already in px from cascade) part of a
/// length-or-percent to px; percentages resolve later, return 0 here.
fn resolve_lp_abs(lp: LengthOrPercent, zoom: f32) -> f32 {
    match lp {
        LengthOrPercent::Length(l) => length_px(l) * zoom,
        LengthOrPercent::Percent(_) => 0.0,
    }
}

fn resolve_lpa_abs(m: LengthPercentOrAuto, zoom: f32) -> f32 {
    match m {
        LengthPercentOrAuto::Length(l) => length_px(l) * zoom,
        LengthPercentOrAuto::Auto | LengthPercentOrAuto::Percent(_) => 0.0,
    }
}

/// Best-effort px value of a `Length`. The cascade resolves em/rem against font
/// size; anything still relative here is treated as its raw number (px).
pub fn length_px(l: Length) -> f32 {
    match l {
        Length::Px(v) => v,
        Length::Em(v) | Length::Ex(v) | Length::Rem(v) => v * 16.0,
        Length::Vw(v) | Length::Vh(v) => v,
    }
}

/// The style to use for a node's box, materialized into our [`ComputedStyle`]
/// vocabulary. This is a thin re-export of the single Stylo→`ComputedStyle`
/// projection boundary, [`crate::css::stylo::computed_style_for`]; layout never
/// touches Stylo's `ComputedValues` directly.
pub fn style_for(doc: &Document, node: NodeId) -> ComputedStyle {
    crate::css::stylo::computed_style_for(doc, node)
}

// Re-exports used by the inline tokenizer to walk inline subtrees.
pub(super) use self::collect::collect_inline_items;

mod collect {
    use super::*;

    /// Walk the inline-level children of a block container (given as built box
    /// indices) and flatten them into a `Vec<InlineItem>` for the IFC.
    pub fn collect_inline_items(
        tree: &LayoutTree,
        doc: &Document,
        boxes: &[usize],
    ) -> Vec<InlineItem> {
        let mut items = Vec::new();
        for &b in boxes {
            emit_box(tree, doc, b, &mut items);
        }
        items
    }

    fn emit_box(tree: &LayoutTree, doc: &Document, idx: usize, out: &mut Vec<InlineItem>) {
        let b = &tree.boxes[idx];
        // Fully-transparent inline subtrees (e.g. the visually-hidden MathML
        // `mwe-math-mathml-a11y` span that mirrors the SVG fallback) neither
        // occupy inline space nor paint — drop them from the IFC entirely.
        if let Some(node) = b.node {
            if style_for(doc, node).opacity <= 0.0 {
                return;
            }
        }
        if b.is_br {
            out.push(InlineItem::ForcedBreak);
            return;
        }
        match b.kind {
            BoxKind::Replaced | BoxKind::InlineBlock => {
                out.push(InlineItem::Atomic { box_idx: idx });
            }
            BoxKind::Inline => {
                if let Some(node) = b.node {
                    match &doc.node(node).data {
                        NodeData::Text(text) => {
                            out.push(InlineItem::Text {
                                node,
                                text: text.clone(),
                            });
                        }
                        NodeData::Element { .. } => {
                            // Inline element: bracket its children with markers
                            // so padding/border/hit-testing scope is known.
                            out.push(InlineItem::StartInlineBox(node));
                            for &c in &b.children {
                                emit_box(tree, doc, c, out);
                            }
                            out.push(InlineItem::EndInlineBox);
                        }
                        _ => {}
                    }
                }
            }
            // A block child should not appear inside an IFC after fixup, but be
            // defensive: treat as atomic so we never panic.
            _ => out.push(InlineItem::Atomic { box_idx: idx }),
        }
    }
}

/// A small helper so other modules can build a content-box `Rect` from an origin
/// and size in document coords.
pub fn rect_from(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(egui::pos2(x, y), egui::vec2(w.max(0.0), h.max(0.0)))
}

/// Zero edges helper.
pub fn zero_edges() -> Edges<f32> {
    Edges::ZERO
}
