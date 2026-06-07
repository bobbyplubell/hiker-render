//! The laid-out box-tree data model: the box/fragment types the layout
//! algorithms (block / inline / table) fill in and the paint pass reads.
//!
//! These are the shared vocabulary of the layout subsystem — the [`LayoutTree`]
//! arena of [`LayoutBox`]es plus the [`InlineFragment`], [`BoxKind`],
//! [`FormattingContext`], and [`ContentSizes`] helper types. They carry no
//! algorithm; the formatting-context modules import them as a unit and write the
//! resolved geometry back into them.

use std::sync::Arc;

use egui::{Color32, Galley};

use crate::dom::NodeId;
use crate::geom::{Edges, Rect};

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
    pub fn border_padding_inline(&self) -> f32 {
        self.border.horizontal() + self.padding.horizontal()
    }

    /// Sum of border + padding on the block (vertical) axis.
    pub fn border_padding_block(&self) -> f32 {
        self.border.vertical() + self.padding.vertical()
    }
}
