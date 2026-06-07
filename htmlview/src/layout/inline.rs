//! Inline formatting context: tokenize inline content into word/space/atomic
//! boxes, greedily fill line boxes, apply text-align and vertical alignment.
//! We own line breaking; egui only measures.
//!
//! Positions: the IFC lays content out relative to (0,0) = the content origin of
//! the establishing block, then the caller offsets every fragment by the block's
//! content-box document origin (`offset_fragments`). The returned size is the
//! used content size (widest line, total height).
//!
//! ## Floats (see ARCHITECTURE.md §4/§5)
//!
//! Each line, when it starts at vertical position `y` (relative to the block
//! content origin), queries the establishing block's [`FloatManager`] for the
//! left/right edges available at that `y` (translated into document coords by
//! the caller-supplied origin) so text wraps alongside floats. When floats make
//! a line too narrow to hold anything, the line drops to the next band boundary.

use crate::css::computed::ComputedStyle;
use crate::css::values::{TextAlign, WhiteSpace};
use crate::dom::{Document, NodeId};
use crate::geom::Vec2;

use super::construct::style_for;
use super::float::FloatManager;
use super::fonts::{allows_wrap, collapses_whitespace, FontCtx};
use super::boxtree::{InlineFragment, LayoutTree};

/// Cap on dropping a line below floats to find a wide-enough band.
const MAX_LINE_DROP_ITERS: usize = 1000;
/// A line band narrower than this (px) can't hold content; drop below the float.
const MIN_LINE_WIDTH: f32 = 4.0;

/// Float context for the IFC: the establishing block's [`FloatManager`] plus the
/// content origin (document coords) used to translate the IFC's local `y`/`x`
/// (relative to the content origin) into the document coords the manager stores.
pub type FloatBands<'a> = (&'a FloatManager, f32, f32);

/// A flat inline item (Servo idiom): runs are bracketed by Start/End markers
/// rather than nested, which keeps line breaking simple.
#[derive(Clone, Debug)]
pub enum InlineItem {
    /// A run of text (split further on whitespace by the breaker).
    Text { node: NodeId, text: String },
    /// An atomic box (image / inline-block); refers to a child box index.
    Atomic { box_idx: usize },
    /// Open an inline box (for padding/border/background + hit-testing scope).
    StartInlineBox(NodeId),
    /// Close the most recent inline box.
    EndInlineBox,
    /// A forced line break (`<br>`).
    ForcedBreak,
}

/// A single token positioned on a line during breaking.
struct Token {
    kind: TokKind,
    width: f32,
    /// Ascent/descent above/below the baseline for this token.
    ascent: f32,
    descent: f32,
    node: Option<NodeId>,
}

enum TokKind {
    Word { galley: std::sync::Arc<egui::Galley>, color: egui::Color32, underline: bool },
    Space { galley: std::sync::Arc<egui::Galley>, color: egui::Color32, underline: bool },
    Atomic { box_idx: usize },
    Break,
}

/// Result of laying out an IFC: produced fragments (relative to content origin)
/// and the used size.
pub struct InlineLayout {
    pub fragments: Vec<InlineFragment>,
    pub size: Vec2,
    /// Atomic boxes that need positioning: (box_idx, top-left relative to origin).
    pub atomic_positions: Vec<(usize, egui::Pos2)>,
}

/// Lay out an inline formatting context within `available_width`.
///
/// `style_node` is the block establishing the IFC (used for text-align). The
/// child box styles come from each item's node. `bands`, if present, lets each
/// line query floats at its y so text wraps alongside them.
#[allow(clippy::too_many_arguments)]
pub fn layout_inline(
    doc: &Document,
    fonts: &FontCtx,
    tree: &LayoutTree,
    items: &[InlineItem],
    available_width: f32,
    block_style: &ComputedStyle,
    bands: Option<FloatBands>,
) -> InlineLayout {
    let tokens = tokenize(doc, fonts, tree, items);

    let avail = available_width.max(0.0);
    let align = block_style.text_align;

    let mut fragments: Vec<InlineFragment> = Vec::new();
    let mut atomic_positions: Vec<(usize, egui::Pos2)> = Vec::new();
    let mut max_width: f32 = 0.0;
    let mut y: f32 = 0.0;

    // The IFC's local x-range is [0, avail]; floats are stored in document
    // coords, so we query [origin_x, origin_x+avail] and shift results back.
    let resolve_band = |line_y: f32| -> (f32, f32, f32) {
        match bands {
            Some((fm, ox, oy)) => {
                let mut cur = oy + line_y;
                let doc_right = ox + avail;
                for _ in 0..MAX_LINE_DROP_ITERS {
                    let left = (fm.left_edge(cur).max(ox)) - ox;
                    let right = fm.right_edge(cur, doc_right) - ox;
                    if (right - left).max(0.0) >= MIN_LINE_WIDTH {
                        return (left.max(0.0), right.min(avail), (cur - oy).max(line_y));
                    }
                    match fm.next_line_top(cur) {
                        Some(next) if next > cur => cur = next,
                        _ => break,
                    }
                }
                let left = (fm.left_edge(oy + line_y).max(ox)) - ox;
                let right = fm.right_edge(oy + line_y, doc_right) - ox;
                (left.max(0.0), right.min(avail), line_y)
            }
            None => (0.0, avail, line_y),
        }
    };

    // Greedy line fill. Each line resolves its band (and possibly drops) before
    // filling; `line_left`/`line_avail` constrain placement for this line.
    let mut line: Vec<&Token> = Vec::new();
    let mut line_width: f32 = 0.0;
    let (mut line_left, mut line_right, _) = resolve_band(y);
    let mut line_avail = (line_right - line_left).max(0.0);
    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        if let TokKind::Break = &tok.kind {
            flush_line(&line, line_width, line_left, line_avail, align, false, &mut y, &mut fragments, &mut atomic_positions, &mut max_width);
            line.clear();
            line_width = 0.0;
            let (l, r, ny) = resolve_band(y);
            line_left = l;
            line_right = r;
            line_avail = (line_right - line_left).max(0.0);
            y = ny;
            i += 1;
            continue;
        }

        // Would this token overflow the line? Spaces never force a break by
        // themselves; we allow a leading word that is wider than the line.
        let is_space = matches!(tok.kind, TokKind::Space { .. });
        if !line.is_empty() && !is_space && line_width + tok.width > line_avail {
            flush_line(&line, line_width, line_left, line_avail, align, true, &mut y, &mut fragments, &mut atomic_positions, &mut max_width);
            line.clear();
            line_width = 0.0;
            // Re-resolve the band at the new y (it may differ next to a float).
            let (l, r, ny) = resolve_band(y);
            line_left = l;
            line_right = r;
            line_avail = (line_right - line_left).max(0.0);
            y = ny;
        }

        // Drop leading spaces at the very start of a line.
        if line.is_empty() && is_space {
            i += 1;
            continue;
        }

        line.push(tok);
        line_width += tok.width;
        i += 1;
    }
    if !line.is_empty() {
        flush_line(&line, line_width, line_left, line_avail, align, false, &mut y, &mut fragments, &mut atomic_positions, &mut max_width);
    }

    InlineLayout {
        fragments,
        size: Vec2::new(max_width.min(avail.max(max_width)), y),
        atomic_positions,
    }
}

/// Place one finished line: trim trailing space, apply alignment, emit fragments
/// with positions relative to the content origin, advance `y`. `line_left` is
/// the x where the line begins (≥ 0; nonzero when a left float intrudes);
/// `avail` is the usable width within the line's band.
#[allow(clippy::too_many_arguments)]
fn flush_line(
    line: &[&Token],
    raw_width: f32,
    line_left: f32,
    avail: f32,
    align: TextAlign,
    line_wrapped: bool,
    y: &mut f32,
    fragments: &mut Vec<InlineFragment>,
    atomic_positions: &mut Vec<(usize, egui::Pos2)>,
    max_width: &mut f32,
) {
    // Trim a single trailing space from the line width for alignment purposes.
    let mut content: Vec<&Token> = line.to_vec();
    let mut width = raw_width;
    while let Some(last) = content.last() {
        if matches!(last.kind, TokKind::Space { .. }) {
            width -= last.width;
            content.pop();
        } else {
            break;
        }
    }
    if content.is_empty() {
        // Empty line (e.g. a <br> on its own): advance by a default line height.
        *y += default_empty_line_height(line);
        return;
    }

    // Line metrics from the tallest token.
    let mut ascent: f32 = 0.0;
    let mut descent: f32 = 0.0;
    for t in &content {
        ascent = ascent.max(t.ascent);
        descent = descent.max(t.descent);
    }
    let line_height = ascent + descent;
    let baseline = *y + ascent;

    // Alignment slack along the inline axis. Justify only applies to wrapped
    // (non-final) lines; the final line of a justified block is left-aligned.
    let slack = (avail - width).max(0.0);
    let (mut x, gap_extra, justify) = match align {
        TextAlign::Right => (line_left + slack, 0.0, false),
        TextAlign::Center => (line_left + slack / 2.0, 0.0, false),
        TextAlign::Justify if line_wrapped => {
            let gaps = content
                .iter()
                .filter(|t| matches!(t.kind, TokKind::Space { .. }))
                .count();
            let extra = if gaps > 0 { slack / gaps as f32 } else { 0.0 };
            (line_left, extra, true)
        }
        // Left, Justify-final-line, and any fallthrough: flush to band left.
        _ => (line_left, 0.0, false),
    };

    for t in &content {
        match &t.kind {
            TokKind::Word { galley, color, underline } => {
                let top = baseline - t.ascent;
                fragments.push(InlineFragment::Text {
                    galley: galley.clone(),
                    pos: egui::pos2(x, top),
                    color: *color,
                    underline: *underline,
                    node: t.node,
                });
                x += t.width;
            }
            TokKind::Space { galley, color, underline } => {
                // Spaces are not painted, but we keep them traceable; emit a
                // zero-content text fragment only if underlined (link space).
                if *underline {
                    let top = baseline - t.ascent;
                    fragments.push(InlineFragment::Text {
                        galley: galley.clone(),
                        pos: egui::pos2(x, top),
                        color: *color,
                        underline: true,
                        node: t.node,
                    });
                }
                x += t.width + if justify { gap_extra } else { 0.0 };
            }
            TokKind::Atomic { box_idx } => {
                let top = baseline - t.ascent;
                atomic_positions.push((*box_idx, egui::pos2(x, top)));
                fragments.push(InlineFragment::Box {
                    box_idx: *box_idx,
                    node: t.node,
                });
                x += t.width;
            }
            TokKind::Break => {}
        }
    }

    *max_width = max_width.max(x);
    *y += line_height;
}

/// Height to advance for an empty (text-less) line such as a lone `<br>`.
fn default_empty_line_height(line: &[&Token]) -> f32 {
    line.iter()
        .map(|t| t.ascent + t.descent)
        .fold(0.0_f32, f32::max)
        .max(16.0)
}

/// Tokenize the flat inline item list into positioned-size tokens. Whitespace is
/// collapsed per the run's `white-space`. A stack of inline-box nodes tracks the
/// current source node for fragments.
fn tokenize(
    doc: &Document,
    fonts: &FontCtx,
    tree: &LayoutTree,
    items: &[InlineItem],
) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let mut stack: Vec<NodeId> = Vec::new();

    for item in items {
        match item {
            InlineItem::StartInlineBox(n) => stack.push(*n),
            InlineItem::EndInlineBox => {
                stack.pop();
            }
            InlineItem::ForcedBreak => out.push(Token {
                kind: TokKind::Break,
                width: 0.0,
                ascent: 0.0,
                descent: 0.0,
                node: stack.last().copied(),
            }),
            InlineItem::Atomic { box_idx } => {
                let b = &tree.boxes[*box_idx];
                let size = b.rect.size();
                out.push(Token {
                    kind: TokKind::Atomic { box_idx: *box_idx },
                    width: size.x.max(0.0),
                    ascent: size.y.max(0.0),
                    descent: 0.0,
                    node: b.node,
                });
            }
            InlineItem::Text { node, text } => {
                let style = style_for(doc, *node);
                tokenize_text(fonts, &style, *node, text, &mut out);
            }
        }
    }
    out
}

/// Split a text run into word/space tokens honoring `white-space`.
fn tokenize_text(
    fonts: &FontCtx,
    style: &ComputedStyle,
    node: NodeId,
    text: &str,
    out: &mut Vec<Token>,
) {
    let metrics = fonts.metrics(style);
    // Line metrics: spread line-height symmetrically around the font ascent/desc.
    let leading = (metrics.line_height - (metrics.ascent + metrics.descent)).max(0.0);
    let ascent = metrics.ascent + leading / 2.0;
    let descent = metrics.descent + leading / 2.0;
    let color = style.color;
    let underline = style.text_decoration_underline;
    let ws = style.white_space;

    let collapse = collapses_whitespace(ws);
    let _wrap = allows_wrap(ws);

    if collapse {
        // Split on ASCII whitespace; each gap becomes one space token.
        let mut chars = text.chars().peekable();
        let mut leading_space = false;
        // Detect a leading whitespace so inter-element spaces survive.
        if let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                leading_space = true;
            }
        }
        if leading_space {
            push_space(fonts, style, node, ascent, descent, color, underline, out);
        }
        let mut word = String::new();
        for c in text.chars() {
            if c.is_whitespace() {
                if !word.is_empty() {
                    push_word(fonts, style, node, &word, ascent, descent, color, underline, out);
                    word.clear();
                    push_space(fonts, style, node, ascent, descent, color, underline, out);
                }
            } else {
                word.push(c);
            }
        }
        if !word.is_empty() {
            push_word(fonts, style, node, &word, ascent, descent, color, underline, out);
        } else if text.ends_with(|c: char| c.is_whitespace()) && !text.trim().is_empty() {
            push_space(fonts, style, node, ascent, descent, color, underline, out);
        }
    } else {
        // Preserve whitespace (pre / pre-wrap): split on '\n' into forced breaks,
        // keep runs verbatim as word tokens (egui measures the whole run).
        let mut first = true;
        for segment in text.split('\n') {
            if !first {
                out.push(Token {
                    kind: TokKind::Break,
                    width: 0.0,
                    ascent,
                    descent,
                    node: Some(node),
                });
            }
            first = false;
            if !segment.is_empty() {
                push_word(fonts, style, node, segment, ascent, descent, color, underline, out);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_word(
    fonts: &FontCtx,
    style: &ComputedStyle,
    node: NodeId,
    word: &str,
    ascent: f32,
    descent: f32,
    color: egui::Color32,
    underline: bool,
    out: &mut Vec<Token>,
) {
    let galley = fonts.layout_run(word, style);
    let width = galley.size().x;
    out.push(Token {
        kind: TokKind::Word { galley, color, underline },
        width,
        ascent,
        descent,
        node: Some(node),
    });
}

#[allow(clippy::too_many_arguments)]
fn push_space(
    fonts: &FontCtx,
    style: &ComputedStyle,
    node: NodeId,
    ascent: f32,
    descent: f32,
    color: egui::Color32,
    underline: bool,
    out: &mut Vec<Token>,
) {
    let galley = fonts.layout_run(" ", style);
    let width = galley.size().x;
    out.push(Token {
        kind: TokKind::Space { galley, color, underline },
        width,
        ascent,
        descent,
        node: Some(node),
    });
}

/// Intrinsic min/max-content sizes of an inline run (rough; sums word widths for
/// max, takes widest single word for min). Used by later shrink-to-fit code.
pub fn intrinsic_inline(
    doc: &Document,
    fonts: &FontCtx,
    tree: &LayoutTree,
    items: &[InlineItem],
) -> super::ContentSizes {
    let tokens = tokenize(doc, fonts, tree, items);
    let mut min: f32 = 0.0;
    let mut max: f32 = 0.0;
    for t in &tokens {
        max += t.width;
        if !matches!(t.kind, TokKind::Space { .. } | TokKind::Break) {
            min = min.max(t.width);
        }
    }
    super::ContentSizes {
        min_content: min,
        max_content: max,
    }
}

/// Mark `WhiteSpace` use so the import is not flagged when unused in a build.
#[allow(dead_code)]
fn _ws_marker(_w: WhiteSpace) {}
