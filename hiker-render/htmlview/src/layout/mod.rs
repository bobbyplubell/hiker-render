//! Layout subsystem.
//!
//! Two conceptual passes (intrinsic sizing -> used layout) over one set of node
//! types. The entry point produces a [`LayoutTree`] of [`LayoutBox`]es with
//! resolved rects, which the paint pass walks.
//!
//! Coordinate convention: every box's `rect` (border box) and `content_rect`
//! are in **document coordinates** (origin = document top-left). Inline
//! fragments stored on a box have their positions in those same document
//! coordinates (the IFC lays content out relative to the block content origin,
//! then the block's content origin offset is baked in).

pub mod block;
pub mod construct;
pub mod float;
pub mod fonts;
pub mod inline;
pub mod table;

use std::sync::Arc;

use egui::{Color32, Galley};

use crate::dom::{Document, NodeId};
use crate::geom::{Edges, Rect, Vec2};

/// Intrinsic content sizes for shrink-to-fit / auto-table width resolution.
///
/// `min_content` is the largest unbreakable piece; `max_content` is the width
/// the content would take with no wrapping.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct ContentSizes {
    pub min_content: f32,
    pub max_content: f32,
}

impl ContentSizes {
    pub const ZERO: ContentSizes = ContentSizes {
        min_content: 0.0,
        max_content: 0.0,
    };

    /// Per-field maximum (used when laying siblings side by side in a row, etc.).
    pub fn max(self, other: ContentSizes) -> ContentSizes {
        ContentSizes {
            min_content: self.min_content.max(other.min_content),
            max_content: self.max_content.max(other.max_content),
        }
    }

    /// Combine sizes that stack along the inline axis (sum), e.g. adjacent
    /// inline runs on the same line.
    pub fn union(self, other: ContentSizes) -> ContentSizes {
        ContentSizes {
            min_content: self.min_content.max(other.min_content),
            max_content: self.max_content + other.max_content,
        }
    }

    /// Add a fixed amount (border/padding/margin) to both sizes.
    pub fn add(self, extra: f32) -> ContentSizes {
        ContentSizes {
            min_content: self.min_content + extra,
            max_content: self.max_content + extra,
        }
    }
}

/// Which layout algorithm a box runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormattingContext {
    Block,
    Inline,
    Table,
    Replaced,
}

/// Coarse box category from `display`, so later table/float agents can dispatch
/// without re-deriving it. Tables are currently laid out as plain blocks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoxKind {
    Block,
    Inline,
    InlineBlock,
    Replaced,
    Table,
    TableRow,
    TableRowGroup,
    TableCell,
    /// Synthesized anonymous box (no DOM node).
    Anonymous,
}

/// The laid-out tree: a flat arena of boxes, paralleling the DOM but only for
/// nodes that generate boxes (anon boxes get synthesized ids beyond the DOM).
#[derive(Debug, Default)]
pub struct LayoutTree {
    pub boxes: Vec<LayoutBox>,
    /// Index of the root box, if any.
    pub root: Option<usize>,
}

/// One painted inline fragment with its position already in document coords.
///
/// CONTRACT ADDITION: `node` traces a fragment back to its source DOM element
/// (or the parent element of a text node) so the paint agent can walk up to find
/// an ancestor `<a href>` for link hit-testing. `None` for purely synthetic
/// fragments.
#[derive(Clone)]
pub enum InlineFragment {
    /// A run of shaped text.
    Text {
        galley: Arc<Galley>,
        /// Top-left of the galley in document coords.
        pos: egui::Pos2,
        color: Color32,
        underline: bool,
        node: Option<NodeId>,
    },
    /// An atomic inline (img / inline-block): refers to a child box laid out
    /// elsewhere in the tree.
    Box {
        box_idx: usize,
        node: Option<NodeId>,
    },
    /// A solid rect (e.g. inline background) — reserved for future use.
    Rect {
        rect: Rect,
        color: Color32,
        node: Option<NodeId>,
    },
}

impl std::fmt::Debug for InlineFragment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InlineFragment::Text { pos, node, .. } => {
                write!(f, "Text{{pos:{pos:?}, node:{node:?}}}")
            }
            InlineFragment::Box { box_idx, node } => {
                write!(f, "Box{{idx:{box_idx}, node:{node:?}}}")
            }
            InlineFragment::Rect { rect, node, .. } => {
                write!(f, "Rect{{{rect:?}, node:{node:?}}}")
            }
        }
    }
}

/// A single laid-out box with its resolved geometry.
#[derive(Debug)]
pub struct LayoutBox {
    /// DOM node this box was generated from (`None` for anonymous boxes).
    pub node: Option<NodeId>,
    pub fc: FormattingContext,
    pub kind: BoxKind,
    /// Border-box rectangle in document coordinates.
    pub rect: Rect,
    /// Content-box rectangle (inside padding+border), document coordinates.
    pub content_rect: Rect,
    pub margin: Edges<f32>,
    pub padding: Edges<f32>,
    pub border: Edges<f32>,
    /// Indices into [`LayoutTree::boxes`].
    pub children: Vec<usize>,
    /// Inline fragments produced when this box establishes an IFC.
    pub inline_fragments: Vec<InlineFragment>,
    /// `<br>` marker (only meaningful inside an IFC).
    pub is_br: bool,
}

impl LayoutBox {
    /// A box for a DOM node with default (zero) geometry.
    pub fn new(node: NodeId, fc: FormattingContext, kind: BoxKind) -> Self {
        LayoutBox {
            node: Some(node),
            fc,
            kind,
            rect: Rect::ZERO,
            content_rect: Rect::ZERO,
            margin: Edges::ZERO,
            padding: Edges::ZERO,
            border: Edges::ZERO,
            children: Vec::new(),
            inline_fragments: Vec::new(),
            is_br: false,
        }
    }

    /// An anonymous box (no DOM node).
    pub fn new_anon(fc: FormattingContext, kind: BoxKind) -> Self {
        LayoutBox {
            node: None,
            fc,
            kind,
            rect: Rect::ZERO,
            content_rect: Rect::ZERO,
            margin: Edges::ZERO,
            padding: Edges::ZERO,
            border: Edges::ZERO,
            children: Vec::new(),
            inline_fragments: Vec::new(),
            is_br: false,
        }
    }

    /// Sum of border + padding on the inline (horizontal) axis.
    pub fn inline_extra(&self) -> f32 {
        self.border.horizontal() + self.padding.horizontal()
    }

    /// Sum of border + padding on the block (vertical) axis.
    pub fn block_extra(&self) -> f32 {
        self.border.vertical() + self.padding.vertical()
    }
}

/// Lay out `doc` at the given content `width` (CSS px), measuring text via
/// `fonts`. Returns the layout tree and the total content size.
///
/// `zoom` multiplies every px dimension. Font sizes/line-heights are zoomed
/// inside [`fonts::FontCtx`]; margins/padding/border/width/height are zoomed
/// during construction/layout. The caller (`HtmlView::layout`) passes its zoom.
pub fn layout_document(
    doc: &Document,
    fonts: &mut fonts::FontCtx,
    width: f32,
    _zoom: f32,
) -> (LayoutTree, Vec2) {
    let mut tree = LayoutTree::default();
    let root = construct::build_tree(doc, fonts, &mut tree);
    tree.root = root;

    let mut content = Vec2::ZERO;
    if let Some(root_idx) = root {
        // The root anonymous block fills the full content width at origin (0,0).
        let used = block::layout_block_box(doc, fonts, &mut tree, root_idx, width, 0.0, 0.0);
        content = used;
    }
    (tree, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::parse_html;
    use std::path::PathBuf;

    struct DirProvider {
        root: PathBuf,
    }

    impl crate::ResourceProvider for DirProvider {
        fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
            let rel = url.trim_start_matches("./").trim_start_matches('/');
            let path = self.root.join(rel);
            let bytes = std::fs::read(&path).ok()?;
            let mime = if rel.ends_with(".css") {
                "text/css".to_string()
            } else {
                "application/octet-stream".to_string()
            };
            Some((bytes, mime))
        }
    }

    /// A headless egui context can do font layout once fonts are installed.
    /// `Context::default()` lazily initializes fonts on first `fonts(...)` call,
    /// but we install the default set explicitly to be safe across versions.
    fn headless_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::default());
        // Run one empty frame so font atlases are ready for measurement.
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        ctx
    }

    #[test]
    fn lays_out_wiki_article_without_panicking() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample/article.html");
        let html = std::fs::read_to_string(path).expect("read article.html");
        let mut doc = parse_html(&html);

        let provider = DirProvider {
            root: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample")),
        };
        let ctx = headless_ctx();
        crate::css::stylo::style_document_stylo(&mut doc, &provider, Some("./"), crate::Theme::Light, 1000.0, Some(&ctx));

        let mut fonts = fonts::FontCtx::new(ctx, 1.0);

        let (tree, content_size) = layout_document(&doc, &mut fonts, 800.0, 1.0);

        eprintln!(
            "article.html content_size = {:?}  ({} boxes)",
            content_size,
            tree.boxes.len()
        );

        assert!(tree.root.is_some(), "expected a root box");
        assert!(
            content_size.y > 1000.0,
            "expected content height > 1000, got {}",
            content_size.y
        );
    }

    fn box_text(doc: &Document, tree: &LayoutTree, idx: usize, depth: usize, out: &mut String) {
        if out.len() > 60 || depth > 8 {
            return;
        }
        if let Some(n) = tree.boxes[idx].node {
            if let crate::dom::NodeData::Text(t) = &doc.node(n).data {
                let t = t.trim();
                if !t.is_empty() {
                    out.push_str(t);
                    out.push(' ');
                }
            }
        }
        for f in &tree.boxes[idx].inline_fragments {
            if let InlineFragment::Text { galley, .. } = f {
                out.push_str(&galley.text().chars().take(40).collect::<String>());
                out.push(' ');
            }
        }
        for &c in &tree.boxes[idx].children {
            box_text(doc, tree, c, depth + 1, out);
        }
    }

    #[test]
    fn lead_text_wraps_beside_right_float_infobox() {
        // The Water infobox is `float:right`; the lead paragraphs must flow to its
        // LEFT (narrowed, sharing the float's vertical band), not be pushed below
        // it. Regression for an anonymous-box wrapper absorbing the float's full
        // height and clearing all following content.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample/article.html");
        let html = std::fs::read_to_string(path).expect("read article.html");
        let mut doc = parse_html(&html);
        let provider = DirProvider {
            root: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample")),
        };
        let ctx = headless_ctx();
        crate::css::stylo::style_document_stylo(&mut doc, &provider, Some("./"), crate::Theme::Light, 1000.0, Some(&ctx));
        let mut fonts = fonts::FontCtx::new(ctx, 1.0);
        let (tree, _content) = layout_document(&doc, &mut fonts, 800.0, 1.0);

        // Locate the floated infobox.
        let infobox = (0..tree.boxes.len())
            .find(|&i| {
                tree.boxes[i].node.map_or(false, |n| {
                    doc.node(n).attr("class").map_or(false, |c| c.contains("ib-chembox"))
                })
            })
            .expect("infobox box");
        let fb = tree.boxes[infobox].rect;
        // It sits on the right (a right float), not the full content width.
        assert!(fb.left() > 200.0, "infobox should be pushed right, left={}", fb.left());

        // Collect <p> boxes with non-trivial text.
        let mut paras: Vec<(usize, Rect)> = Vec::new();
        for i in 0..tree.boxes.len() {
            if tree.boxes[i].node.and_then(|n| doc.node(n).tag()) == Some("p") {
                let mut txt = String::new();
                box_text(&doc, &tree, i, 0, &mut txt);
                if txt.trim().len() > 20 {
                    paras.push((i, tree.boxes[i].rect));
                }
            }
        }

        // The first substantial paragraph (the lead) must sit beside the float:
        // its top is within the float's vertical span and its right edge stops at
        // (or before) the float's left edge — i.e. it is narrowed, not full width.
        let (_, lead) = *paras
            .iter()
            .find(|(_, r)| r.top() < fb.bottom() && r.height() > 1.0)
            .expect("a paragraph beside the float");
        assert!(
            lead.top() >= fb.top() - 1.0 && lead.top() < fb.bottom(),
            "lead top {} should be within float span [{}, {}]",
            lead.top(), fb.top(), fb.bottom()
        );
        assert!(
            lead.right() <= fb.left() + 1.0,
            "lead right {} should not overlap float left {}",
            lead.right(), fb.left()
        );

        // At least one paragraph below the float bottom must use the full content
        // width again (wrap-back below the float).
        let full_below = paras
            .iter()
            .any(|(_, r)| r.top() > fb.bottom() && r.right() > fb.left() + 50.0);
        assert!(full_below, "expected a full-width paragraph below the float");
    }

    #[test]
    fn absolute_children_are_out_of_flow_and_positioned_by_offsets() {
        // A `position:relative` box with `position:absolute` children placed by
        // top/left. The absolutes must NOT stack in flow (the relative box keeps
        // its explicit height) and must land at offset positions inside it. This
        // mirrors Wikipedia's NFPA fire-diamond (absolute digits in a relative
        // box) which previously overflowed into the next table row.
        let html = r#"<div style="position:relative; height:80px; width:80px">
            <div style="position:absolute; top:12px; left:35px; width:12px">A</div>
            <div style="position:absolute; top:31px; left:15px; width:12px">B</div>
            <p>after</p>
        </div>"#;
        let mut doc = parse_html(html);
        struct Null;
        impl crate::ResourceProvider for Null {
            fn fetch(&self, _: &str) -> Option<(Vec<u8>, String)> {
                None
            }
        }
        let ctx = headless_ctx();
        crate::css::stylo::style_document_stylo(&mut doc, &Null, None, crate::Theme::Light, 1000.0, Some(&ctx));
        let mut fonts = fonts::FontCtx::new(ctx, 1.0);
        let (tree, _content) = layout_document(&doc, &mut fonts, 800.0, 1.0);

        // The relative container keeps its explicit 80px height (absolutes do not
        // add to it).
        let rel = (0..tree.boxes.len())
            .find(|&i| {
                tree.boxes[i].node.map_or(false, |n| {
                    doc.node(n)
                        .attr("style")
                        .map_or(false, |s| s.contains("position:relative"))
                })
            })
            .expect("relative box");
        let rel_rect = tree.boxes[rel].rect;
        assert!(
            (rel_rect.height() - 80.0).abs() <= 2.0,
            "relative box should stay ~80px, got {}",
            rel_rect.height()
        );

        // The two absolute children sit at their offsets relative to the
        // container's content box, not stacked at the top-left.
        let mut abs: Vec<Rect> = Vec::new();
        for i in 0..tree.boxes.len() {
            if tree.boxes[i].node.map_or(false, |n| {
                doc.node(n).attr("style").map_or(false, |s| s.contains("position:absolute"))
            }) {
                abs.push(tree.boxes[i].rect);
            }
        }
        assert_eq!(abs.len(), 2, "two absolute boxes");
        // A is at top:12,left:35; B at top:31,left:15 -> A is higher & further
        // right than B (proving offsets applied, not in-flow stacking).
        abs.sort_by(|a, b| a.top().partial_cmp(&b.top()).unwrap());
        assert!(abs[0].top() < abs[1].top(), "A above B");
        assert!(abs[0].left() > abs[1].left(), "A right of B");
        // Both stay within the container vertically (no overflow).
        for r in &abs {
            assert!(
                r.top() >= rel_rect.top() - 1.0 && r.bottom() <= rel_rect.bottom() + 1.0,
                "absolute child {r:?} overflows container {rel_rect:?}"
            );
        }
    }

    #[test]
    fn opacity_zero_inline_content_is_not_laid_out() {
        // The visually-hidden MathML a11y span (`opacity:0`) must not contribute
        // any inline fragments — otherwise its raw LaTeX/letters overprint the
        // surrounding text. Mirrors `<span class=mwe-math-mathml-a11y>`.
        let html = r#"<p>visible <span style="opacity:0">HIDDENTOKEN</span> tail</p>"#;
        let mut doc = parse_html(html);
        struct Null;
        impl crate::ResourceProvider for Null {
            fn fetch(&self, _: &str) -> Option<(Vec<u8>, String)> {
                None
            }
        }
        let ctx = headless_ctx();
        crate::css::stylo::style_document_stylo(&mut doc, &Null, None, crate::Theme::Light, 1000.0, Some(&ctx));
        let mut fonts = fonts::FontCtx::new(ctx, 1.0);
        let (tree, _content) = layout_document(&doc, &mut fonts, 800.0, 1.0);

        let mut all_text = String::new();
        for b in &tree.boxes {
            for f in &b.inline_fragments {
                if let InlineFragment::Text { galley, .. } = f {
                    all_text.push_str(galley.text());
                }
            }
        }
        assert!(
            all_text.contains("visible") && all_text.contains("tail"),
            "visible text should remain, got {all_text:?}"
        );
        assert!(
            !all_text.contains("HIDDENTOKEN"),
            "opacity:0 content must not be laid out, got {all_text:?}"
        );
    }
}
