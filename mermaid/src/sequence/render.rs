//! Sequence-diagram layout + drawing: participants become columns with vertical
//! lifelines, items are walked into placed messages/notes/frames/activations/
//! rects (self-layout, no dagre), then the whole scene is emitted as one SVG
//! document.

use std::fmt::Write as _;

use crate::{MermaidError, MermaidOptions, MermaidRender};

use super::model;
use super::parse::parse;

/// Per-char advance fraction of font size (kept in sync with `measure`).
const CHAR_ADVANCE_EM: f32 = 0.6;
/// Outer canvas margin, px.
const MARGIN: f32 = 16.0;
/// Participant box vertical padding (each side), px.
const BOX_PAD_Y: f32 = 8.0;
/// Participant box horizontal padding (each side), px.
const BOX_PAD_X: f32 = 12.0;
/// Minimum participant box width, px.
const MIN_BOX_W: f32 = 40.0;
/// Horizontal gap between adjacent participant boxes, px.
const COL_GAP: f32 = 40.0;
/// Vertical gap between consecutive message rows, px. A little roomy so a
/// message's lifted text label clears the line above it.
const MESSAGE_GAP: f32 = 46.0;
/// Gap between the participant boxes and the first message row, px.
const TOP_GAP: f32 = 30.0;
/// Width of a self-message loop, px.
const SELF_LOOP_W: f32 = 36.0;
/// Height of a self-message loop, px.
const SELF_LOOP_H: f32 = 28.0;
/// Arrowhead length / half-width, px.
const ARROW_LEN: f32 = 9.0;
const ARROW_HALF: f32 = 4.0;
/// Stroke width, px.
const STROKE_W: f32 = 1.5;
/// Width of an activation bar, px.
const ACT_W: f32 = 10.0;
/// Horizontal offset added per nested activation level, px.
const ACT_NEST_DX: f32 = 5.0;
/// Vertical row consumed by a note, px.
const NOTE_GAP: f32 = 50.0;
/// Note rectangle internal padding (each side), px.
const NOTE_PAD_X: f32 = 10.0;
const NOTE_PAD_Y: f32 = 6.0;
/// Min note rectangle width, px.
const NOTE_MIN_W: f32 = 60.0;
/// How far a left/right note sits from the lifeline, px.
const NOTE_SIDE_GAP: f32 = 14.0;
/// Inset (each side) added per nested block frame, px.
const FRAME_INSET: f32 = 10.0;
/// Base vertical padding inside a frame below the last row, px.
const FRAME_PAD_BOTTOM: f32 = 12.0;
/// Height of the frame's keyword label tab, px.
const FRAME_TAB_H: f32 = 16.0;
/// Radius of the autonumber badge circle, px.
const BADGE_R: f32 = 9.0;

/// Vertical headroom a frame reserves between its top edge and its first
/// contained row, so the centered opening label (a line of text at `font_size`)
/// clears the first message's lifted text label with a small gap.
///
/// The opening label is centered at `y0 + FRAME_TAB_H/2` and a message's text is
/// lifted ~`font_size*0.4 + 3` above its line; we add a full label height plus a
/// gap on top of that so the two never touch.
fn frame_pad_top(fs: f32) -> f32 {
    // Base tab/label band + ~font_size * 1.4 of clearance.
    FRAME_TAB_H + fs * 1.4 + 6.0
}

/// Vertical headroom reserved after a section divider before its first row, so
/// the section label (drawn `font_size*0.7` below the divider, ~`font_size`
/// tall) clears the next message's lifted text label.
fn section_pad_top(fs: f32) -> f32 {
    fs * 1.6 + 12.0
}

/// Heuristic label width (font-free), matching the flowchart `measure` rule.
/// Used for non-label decorations (the frame keyword tab) where rich markup
/// never applies.
fn label_width(label: &str, font_size: f32) -> f32 {
    label.chars().count() as f32 * font_size * CHAR_ADVANCE_EM
}

/// Rich-aware label width: equals [`label_width`] for plain labels but accounts
/// for markdown/math in message, note, and participant labels.
fn rich_label_width(label: &str, font_size: f32) -> f32 {
    crate::label::measure(label, font_size).0
}

/// Participant box width for a label (with padding + minimum).
fn box_width(label: &str, font_size: f32) -> f32 {
    (rich_label_width(label, font_size) + 2.0 * BOX_PAD_X).max(MIN_BOX_W)
}

// ----------------------------------------------------------------------------
// Render
// ----------------------------------------------------------------------------

/// Render mermaid sequence-diagram source to an SVG document.
pub(super) fn render(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diagram = parse(src).map_err(MermaidError::Parse)?;
    if diagram.participants.is_empty() {
        return Err(MermaidError::Empty);
    }
    Ok(draw_sequence(&diagram, opts))
}

// ----------------------------------------------------------------------------
// Layout records
// ----------------------------------------------------------------------------

/// A message ready to draw at a row y, with resolved column endpoints and the
/// activation-edge x positions (so arrows can land on the activation bar rather
/// than the bare lifeline when one is active).
struct PlacedMessage<'a> {
    msg: &'a model::Message,
    y: f32,
    from_col: usize,
    to_col: usize,
    /// x where the line should start (the active edge of `from`'s bar, or its
    /// lifeline center).
    from_x: f32,
    /// x where the line should end (the active edge of `to`'s bar, or its
    /// lifeline center).
    to_x: f32,
}

/// A note ready to draw.
struct PlacedNote<'a> {
    note: &'a model::Note,
    y: f32,
    /// Rectangle left/right x and the center for the text.
    x0: f32,
    x1: f32,
}

/// A block frame ready to draw.
struct PlacedFrame {
    kind: model::BlockKind,
    label: String,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    /// (divider y, section label) for each else/and/option section.
    dividers: Vec<(f32, String)>,
}

/// A `rect` background highlight ready to draw: a color + the rectangle that
/// spans the contained rows and involved participants. Drawn behind everything.
struct PlacedRect {
    color: model::Rgba,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
}

/// An activation bar ready to draw: a column, nesting level (for x offset), and
/// vertical span.
struct PlacedAct {
    col: usize,
    level: usize,
    y0: f32,
    y1: f32,
}

/// Mutable state threaded through the layout walk.
struct Layout<'a> {
    centers: &'a [f32],
    /// Per-participant stack of open activations: each entry is the y where the
    /// bar started. Stack depth = nesting level.
    act_stacks: Vec<Vec<f32>>,
    messages: Vec<PlacedMessage<'a>>,
    notes: Vec<PlacedNote<'a>>,
    frames: Vec<PlacedFrame>,
    rects: Vec<PlacedRect>,
    acts: Vec<PlacedAct>,
    /// Tracks the widest x reached (for canvas sizing).
    max_x: f32,
}

impl<'a> Layout<'a> {
    /// Center x of the currently-active edge of `col` on the right side: the
    /// outer x of its top-of-stack activation bar, used so an arrow arriving at
    /// an active participant lands on the bar.
    fn active_edge(&self, col: usize, from_left: bool) -> f32 {
        let cx = self.centers[col];
        let depth = self.act_stacks[col].len();
        if depth == 0 {
            return cx;
        }
        // Outer edge of the top bar.
        let level = depth - 1;
        let half = ACT_W / 2.0 + level as f32 * ACT_NEST_DX;
        if from_left { cx - half } else { cx + half }
    }
}

/// Lay out + draw the parsed diagram into an SVG document.
fn draw_sequence(diagram: &model::SequenceDiagram, opts: &MermaidOptions) -> MermaidRender {
    let fs = opts.font_size_px;
    let box_h = fs * 1.2 + 2.0 * BOX_PAD_Y;

    // --- Columns: each participant gets a center x. Uniform column width =
    // widest box, so lifelines are evenly spaced. ---
    let widths: Vec<f32> = diagram.participants.iter().map(|p| box_width(&p.label, fs)).collect();
    let col_w = widths.iter().cloned().fold(MIN_BOX_W, f32::max);

    let n = diagram.participants.len();
    let mut centers: Vec<f32> = Vec::with_capacity(n);
    for i in 0..n {
        let cx = MARGIN + col_w / 2.0 + i as f32 * (col_w + COL_GAP);
        centers.push(cx);
    }

    // Index id → column.
    let col_of = |id: &str| -> Option<usize> {
        diagram.participants.iter().position(|p| p.id == id)
    };

    // --- Vertical extents ---
    let box_top = MARGIN;
    let box_bottom = box_top + box_h;
    let first_row_y = box_bottom + TOP_GAP;

    // --- Walk the item tree assigning y to leaves, computing frame extents and
    // activation spans. ---
    let mut lay = Layout {
        centers: &centers,
        act_stacks: vec![Vec::new(); n],
        messages: Vec::new(),
        notes: Vec::new(),
        frames: Vec::new(),
        rects: Vec::new(),
        acts: Vec::new(),
        max_x: centers.last().copied().unwrap_or(MARGIN + col_w / 2.0) + col_w / 2.0,
    };

    let mut y = first_row_y;
    layout_items(&mut lay, &diagram.items, &col_of, fs, &mut y, 0);

    // Close any activations left open at end of script: run them to the bottom.
    let content_bottom = (y - MESSAGE_GAP * 0.5).max(first_row_y);
    for col in 0..n {
        let stack = std::mem::take(&mut lay.act_stacks[col]);
        for (level, y0) in stack.into_iter().enumerate() {
            lay.acts.push(PlacedAct { col, level, y0, y1: content_bottom });
        }
    }

    let messages_bottom = content_bottom + MESSAGE_GAP * 0.5;

    // --- Canvas size ---
    let last_cx = centers.last().copied().unwrap_or(MARGIN + col_w / 2.0);
    let right_extent = (last_cx + col_w / 2.0 + SELF_LOOP_W + MARGIN).max(lay.max_x + MARGIN);
    let width = right_extent.max(MARGIN * 2.0 + col_w);
    let height = messages_bottom + MARGIN;

    let mut svg = String::new();
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Shared filled-arrowhead marker.
    emit_defs(&mut svg, opts);

    // --- Rect background highlights (drawn first, behind everything) ---
    for rb in &lay.rects {
        emit_rect_bg(&mut svg, rb);
    }

    // --- Frames (behind lifelines/messages, above rect backgrounds) ---
    for f in &lay.frames {
        emit_frame(&mut svg, f, opts);
    }

    // --- Lifelines (dashed verticals) ---
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    for &cx in &centers {
        let _ = write!(
            svg,
            "<line x1=\"{cx:.2}\" y1=\"{y1:.2}\" x2=\"{cx:.2}\" y2=\"{y2:.2}\" \
             stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\" stroke-dasharray=\"3 3\"/>",
            y1 = box_bottom,
            y2 = messages_bottom,
        );
    }

    // --- Activation bars (over lifelines, under messages) ---
    for a in &lay.acts {
        emit_activation(&mut svg, centers[a.col], a, opts);
    }

    // --- Participant boxes (rect + centered label) ---
    let (fill, fo) = fill_attrs(opts.node_fill);
    let (nstroke, nso) = stroke_attrs(opts.node_stroke);
    for (i, p) in diagram.participants.iter().enumerate() {
        let cx = centers[i];
        let bw = widths[i].max(MIN_BOX_W);
        let x = cx - bw / 2.0;
        let _ = write!(
            svg,
            "<rect x=\"{x:.2}\" y=\"{box_top:.2}\" width=\"{bw:.2}\" height=\"{box_h:.2}\" \
             rx=\"3\" ry=\"3\" fill=\"{fill}\"{fo} stroke=\"{nstroke}\"{nso} stroke-width=\"{STROKE_W}\"/>",
        );
        emit_text(&mut svg, &p.label, cx, box_top + box_h / 2.0, opts);
    }

    // --- Notes ---
    for pn in &lay.notes {
        emit_note(&mut svg, pn, opts);
    }

    // --- Messages ---
    for pm in &lay.messages {
        if pm.from_col == pm.to_col {
            emit_self_message(&mut svg, lay.centers[pm.from_col], pm.y, pm.msg, opts);
        } else {
            emit_message(&mut svg, pm.from_x, pm.to_x, pm.y, pm.msg, opts);
        }
    }

    svg.push_str("</svg>");

    MermaidRender { svg, width_px: w, height_px: h }
}

/// Recursively place items, advancing `*y` for each leaf row and recording
/// frames/activations. `depth` is the current block-nesting depth (for insets).
fn layout_items<'a>(
    lay: &mut Layout<'a>,
    items: &'a [model::Item],
    col_of: &impl Fn(&str) -> Option<usize>,
    fs: f32,
    y: &mut f32,
    depth: usize,
) {
    for item in items {
        match item {
            model::Item::Message(m) => {
                let from_col = col_of(&m.from);
                let to_col = col_of(&m.to);
                let (Some(fc), Some(tc)) = (from_col, to_col) else { continue };
                let my = *y;

                // Activation: `+` activates the target on arrival.
                if m.activate_to {
                    lay.act_stacks[tc].push(my);
                }
                // Compute landing edges using the *current* activation depth
                // (after a possible +activate, so the arrow lands on the new
                // bar's outer edge facing the sender).
                let going_right = lay.centers[tc] >= lay.centers[fc];
                let from_x = lay.active_edge(fc, !going_right);
                let to_x = lay.active_edge(tc, going_right);

                lay.messages.push(PlacedMessage {
                    msg: m,
                    y: my,
                    from_col: fc,
                    to_col: tc,
                    from_x,
                    to_x,
                });

                // A self-message draws its label to the right of the loop
                // (at `cx + SELF_LOOP_W + 4.0`, see `emit_self_message`); track
                // its right edge so the canvas reserves room for the full label.
                if fc == tc && !m.text.is_empty() {
                    let label_right = lay.centers[fc]
                        + SELF_LOOP_W
                        + 4.0
                        + rich_label_width(&m.text, fs);
                    lay.max_x = lay.max_x.max(label_right);
                }

                // `-` deactivates the sender's current activation on send.
                if m.deactivate_from {
                    if let Some(y0) = lay.act_stacks[fc].pop() {
                        let level = lay.act_stacks[fc].len();
                        lay.acts.push(PlacedAct { col: fc, level, y0, y1: my });
                    }
                }

                if m.from == m.to {
                    *y += SELF_LOOP_H + MESSAGE_GAP * 0.5;
                } else {
                    *y += MESSAGE_GAP;
                }
            }
            model::Item::Note(note) => {
                let (x0, x1) = note_extents(lay, note, col_of, fs);
                lay.max_x = lay.max_x.max(x1);
                lay.notes.push(PlacedNote { note, y: *y, x0, x1 });
                *y += NOTE_GAP;
            }
            model::Item::Activate(id) => {
                if let Some(c) = col_of(id) {
                    lay.act_stacks[c].push(*y - MESSAGE_GAP * 0.3);
                }
            }
            model::Item::Deactivate(id) => {
                if let Some(c) = col_of(id) {
                    if let Some(y0) = lay.act_stacks[c].pop() {
                        let level = lay.act_stacks[c].len();
                        lay.acts.push(PlacedAct { col: c, level, y0, y1: *y - MESSAGE_GAP * 0.3 });
                    }
                }
            }
            model::Item::Block(b) => {
                layout_block(lay, b, col_of, fs, y, depth);
            }
            model::Item::Rect(rb) => {
                layout_rect(lay, rb, col_of, fs, y, depth);
            }
        }
    }
}

/// Place a `rect` background highlight: lay out its children (advancing `*y`),
/// then record a background rectangle spanning those rows and the involved
/// participants. No label tab. Nesting depth widens the span slightly so a rect
/// inside another frame still reads as a band.
fn layout_rect<'a>(
    lay: &mut Layout<'a>,
    rb: &'a model::RectBlock,
    col_of: &impl Fn(&str) -> Option<usize>,
    fs: f32,
    y: &mut f32,
    depth: usize,
) {
    // A little top padding so the band doesn't clip the first row's lifted text.
    let pad = MESSAGE_GAP * 0.35;
    let y_top = *y;
    *y += pad;
    layout_items(lay, &rb.items, col_of, fs, y, depth);
    let y_bottom = *y - MESSAGE_GAP * 0.25 + pad;
    *y = y_bottom + MESSAGE_GAP * 0.2;

    // Horizontal span across the involved participants (fall back to all).
    let (lo, hi) =
        rect_col_span(&rb.items, col_of).unwrap_or((0, lay.centers.len().saturating_sub(1)));
    let inset = depth as f32 * FRAME_INSET;
    let left = lay.centers[lo] - col_half(lay, fs) - FRAME_INSET - inset;
    let right = lay.centers[hi] + col_half(lay, fs) + FRAME_INSET + inset;
    lay.max_x = lay.max_x.max(right);

    lay.rects.push(PlacedRect {
        color: rb.color,
        x0: left,
        x1: right,
        y0: y_top,
        y1: y_bottom,
    });
}

/// Place a block: reserve top padding, lay out children (tracking divider ys),
/// reserve bottom padding, and record the frame rectangle spanning the involved
/// participants.
fn layout_block<'a>(
    lay: &mut Layout<'a>,
    block: &'a model::Block,
    col_of: &impl Fn(&str) -> Option<usize>,
    fs: f32,
    y: &mut f32,
    depth: usize,
) {
    let pad_top = frame_pad_top(fs);
    let y_top = *y;
    *y += pad_top;
    let first_child_y = *y;

    // Section divider ys, keyed by child index where the section begins.
    let mut div_by_idx: Vec<(usize, String)> = block.sections.clone();
    div_by_idx.sort_by_key(|(i, _)| *i);
    let mut next_div = 0;
    let mut dividers: Vec<(f32, String)> = Vec::new();

    for (idx, child) in block.items.iter().enumerate() {
        // Emit any divider that begins at this child index (before placing it).
        while next_div < div_by_idx.len() && div_by_idx[next_div].0 == idx {
            // Place the divider a little above the upcoming row, then reserve
            // headroom below it so the section label clears that row's text.
            let dy = (*y - MESSAGE_GAP * 0.2).max(first_child_y - pad_top * 0.4);
            dividers.push((dy, div_by_idx[next_div].1.clone()));
            *y += section_pad_top(fs);
            next_div += 1;
        }
        layout_items(lay, std::slice::from_ref(child), col_of, fs, y, depth + 1);
    }
    // Dividers that begin after the last child (empty trailing section).
    while next_div < div_by_idx.len() {
        dividers.push((*y - MESSAGE_GAP * 0.2, div_by_idx[next_div].1.clone()));
        next_div += 1;
    }

    let y_bottom = *y + FRAME_PAD_BOTTOM;
    *y = y_bottom + MESSAGE_GAP * 0.2;

    // Horizontal span: the min/max column involved by any descendant. Fall back
    // to all columns if none resolved.
    let (lo, hi) = block_col_span(block, col_of).unwrap_or((0, lay.centers.len().saturating_sub(1)));
    let inset = depth as f32 * FRAME_INSET;
    let left = lay.centers[lo] - col_half(lay, fs) - FRAME_INSET - inset;
    let right = lay.centers[hi] + col_half(lay, fs) + FRAME_INSET + inset;
    lay.max_x = lay.max_x.max(right);

    lay.frames.push(PlacedFrame {
        kind: block.kind,
        label: block.label.clone(),
        x0: left,
        x1: right,
        y0: y_top,
        y1: y_bottom,
        dividers,
    });

    let _ = first_child_y;
}

/// Half the inter-lifeline padding to extend a frame past edge lifelines.
fn col_half(_lay: &Layout, _fs: f32) -> f32 {
    COL_GAP / 2.0
}

/// Compute the min/max participant column referenced anywhere inside a block
/// (messages' endpoints, note targets, nested blocks). `None` if nothing maps.
fn block_col_span(block: &model::Block, col_of: &impl Fn(&str) -> Option<usize>) -> Option<(usize, usize)> {
    rect_col_span(&block.items, col_of)
}

/// Compute the min/max participant column referenced anywhere within `items`
/// (messages' endpoints, note targets, nested blocks/rects). `None` if nothing
/// maps. Shared by labeled-block frames and `rect` background blocks.
fn rect_col_span(items: &[model::Item], col_of: &impl Fn(&str) -> Option<usize>) -> Option<(usize, usize)> {
    let mut lo = usize::MAX;
    let mut hi = 0usize;
    let mut any = false;
    fn visit(
        items: &[model::Item],
        col_of: &impl Fn(&str) -> Option<usize>,
        lo: &mut usize,
        hi: &mut usize,
        any: &mut bool,
    ) {
        for it in items {
            let note = |c: usize, lo: &mut usize, hi: &mut usize, any: &mut bool| {
                *lo = (*lo).min(c);
                *hi = (*hi).max(c);
                *any = true;
            };
            match it {
                model::Item::Message(m) => {
                    if let Some(c) = col_of(&m.from) { note(c, lo, hi, any); }
                    if let Some(c) = col_of(&m.to) { note(c, lo, hi, any); }
                }
                model::Item::Note(n) => {
                    for t in &n.targets {
                        if let Some(c) = col_of(t) { note(c, lo, hi, any); }
                    }
                }
                model::Item::Activate(id) | model::Item::Deactivate(id) => {
                    if let Some(c) = col_of(id) { note(c, lo, hi, any); }
                }
                model::Item::Block(b) => visit(&b.items, col_of, lo, hi, any),
                model::Item::Rect(rb) => visit(&rb.items, col_of, lo, hi, any),
            }
        }
    }
    visit(items, col_of, &mut lo, &mut hi, &mut any);
    if any { Some((lo, hi)) } else { None }
}

/// Compute a note's rectangle x extents for the given placement/targets.
fn note_extents(
    lay: &Layout,
    note: &model::Note,
    col_of: &impl Fn(&str) -> Option<usize>,
    fs: f32,
) -> (f32, f32) {
    let text_w = rich_label_width(&note.text, fs) + 2.0 * NOTE_PAD_X;
    let w = text_w.max(NOTE_MIN_W);
    match note.placement {
        model::NotePlacement::LeftOf => {
            let c = note.targets.first().and_then(|t| col_of(t)).unwrap_or(0);
            let cx = lay.centers[c];
            let x1 = cx - NOTE_SIDE_GAP;
            (x1 - w, x1)
        }
        model::NotePlacement::RightOf => {
            let c = note.targets.first().and_then(|t| col_of(t)).unwrap_or(0);
            let cx = lay.centers[c];
            let x0 = cx + NOTE_SIDE_GAP;
            (x0, x0 + w)
        }
        model::NotePlacement::Over => {
            let cols: Vec<usize> =
                note.targets.iter().filter_map(|t| col_of(t)).collect();
            if cols.is_empty() {
                let cx = lay.centers.first().copied().unwrap_or(MARGIN);
                return (cx - w / 2.0, cx + w / 2.0);
            }
            let lo = *cols.iter().min().unwrap();
            let hi = *cols.iter().max().unwrap();
            let cl = lay.centers[lo];
            let cr = lay.centers[hi];
            let mid = (cl + cr) / 2.0;
            // Span at least the lifelines plus padding, or the text width.
            let span = (cr - cl + 2.0 * NOTE_PAD_X * 2.0).max(w);
            (mid - span / 2.0, mid + span / 2.0)
        }
    }
}

/// `<defs>` with the filled-triangle end marker (oriented along the path).
fn emit_defs(svg: &mut String, opts: &MermaidOptions) {
    let (fill, fo) = fill_attrs(opts.edge_stroke);
    let _ = write!(
        svg,
        "<defs><marker id=\"seq-arrow\" markerWidth=\"{len}\" markerHeight=\"{w}\" \
         refX=\"{len}\" refY=\"{half}\" orient=\"auto\" markerUnits=\"userSpaceOnUse\">\
         <path d=\"M0,0 L{len},{half} L0,{w} Z\" fill=\"{fill}\"{fo}/></marker></defs>",
        len = ARROW_LEN,
        w = ARROW_HALF * 2.0,
        half = ARROW_HALF,
    );
}

/// A normal (cross-lifeline) message: horizontal line + head + centered text.
fn emit_message(svg: &mut String, x_from: f32, x_to: f32, y: f32, m: &model::Message, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let dash = if m.dashed { " stroke-dasharray=\"4 3\"" } else { "" };

    // Pull the line back from the target so the head's tip lands on the lifeline.
    let dir = if x_to >= x_from { 1.0 } else { -1.0 };
    let line_end = match m.style {
        model::ArrowStyle::Filled => x_to - dir * ARROW_LEN,
        _ => x_to,
    };
    let marker = if m.style == model::ArrowStyle::Filled {
        " marker-end=\"url(#seq-arrow)\""
    } else {
        ""
    };

    let _ = write!(
        svg,
        "<line x1=\"{x_from:.2}\" y1=\"{y:.2}\" x2=\"{line_end:.2}\" y2=\"{y:.2}\" \
         stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"{dash}{marker}/>",
    );

    // Open / async heads: a small V at the target. Cross: an ✗.
    match m.style {
        model::ArrowStyle::Open | model::ArrowStyle::Async => emit_open_head(svg, x_to, y, dir, opts),
        model::ArrowStyle::Cross => emit_cross(svg, x_to, y, opts),
        model::ArrowStyle::Filled => {}
    }

    // Autonumber badge at the sending end, just inside the line start.
    if let Some(num) = m.number {
        let bx = x_from + dir * (BADGE_R + 2.0);
        emit_number_badge(svg, bx, y, num, opts);
    }

    // Text centered clearly above the line (lift it so descenders clear the
    // line, not sitting on top of it).
    if !m.text.is_empty() {
        let cx = (x_from + x_to) / 2.0;
        let ty = y - (opts.font_size_px * 0.4 + 3.0);
        emit_text(svg, &m.text, cx, ty, opts);
    }
}

/// A self-message: a small rectangular loop to the right of the lifeline, with
/// a head returning to the lifeline and the text to the loop's right.
fn emit_self_message(svg: &mut String, cx: f32, y: f32, m: &model::Message, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let dash = if m.dashed { " stroke-dasharray=\"4 3\"" } else { "" };

    let right = cx + SELF_LOOP_W;
    let y0 = y;
    let y1 = y + SELF_LOOP_H;

    // Out from lifeline, down, back toward lifeline (stop short for the head).
    let back_end = match m.style {
        model::ArrowStyle::Filled => cx + ARROW_LEN,
        _ => cx,
    };
    let marker = if m.style == model::ArrowStyle::Filled {
        " marker-end=\"url(#seq-arrow)\""
    } else {
        ""
    };

    let _ = write!(
        svg,
        "<path d=\"M{cx:.2},{y0:.2} L{right:.2},{y0:.2} L{right:.2},{y1:.2} L{back_end:.2},{y1:.2}\" \
         fill=\"none\" stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"{dash}{marker}/>",
    );

    // Heads pointing left (dir = -1) back at the lifeline on the lower segment.
    match m.style {
        model::ArrowStyle::Open | model::ArrowStyle::Async => emit_open_head(svg, cx, y1, -1.0, opts),
        model::ArrowStyle::Cross => emit_cross(svg, cx, y1, opts),
        model::ArrowStyle::Filled => {}
    }

    // Autonumber badge at the top-out point of the loop.
    if let Some(num) = m.number {
        emit_number_badge(svg, cx + BADGE_R + 2.0, y0, num, opts);
    }

    if !m.text.is_empty() {
        let tx = right + 4.0;
        let ty = (y0 + y1) / 2.0;
        emit_text_left(svg, &m.text, tx, ty, opts);
    }
}

/// Draw a small themed circular badge containing the autonumber `num`, centered
/// at `(cx, cy)`. Filled with the node fill / stroke so it matches the theme.
fn emit_number_badge(svg: &mut String, cx: f32, cy: f32, num: u32, opts: &MermaidOptions) {
    let (fill, fo) = fill_attrs(opts.node_fill);
    let (stroke, so) = stroke_attrs(opts.node_stroke);
    let _ = write!(
        svg,
        "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" \
         fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        r = BADGE_R,
    );
    // Number text: slightly smaller than the body font so it fits the badge.
    let small = opts.font_size_px * 0.8;
    crate::label::emit(
        svg,
        &num.to_string(),
        cx,
        cy,
        crate::label::Anchor::Middle,
        small,
        opts.text_color,
        &opts.font_family,
    );
}

/// Draw a `rect` background highlight: a translucent filled rectangle behind the
/// messages in its span. No stroke, no label.
fn emit_rect_bg(svg: &mut String, rb: &PlacedRect) {
    let c = rb.color;
    let w = (rb.x1 - rb.x0).max(0.0);
    let h = (rb.y1 - rb.y0).max(0.0);
    let opacity = c.a as f32 / 255.0;
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         fill=\"rgb({r},{g},{b})\" fill-opacity=\"{op:.4}\"/>",
        x = rb.x0,
        y = rb.y0,
        r = c.r,
        g = c.g,
        b = c.b,
        op = opacity,
    );
}

/// Draw a labeled block frame: outer rectangle, a keyword tab in the top-left,
/// the opening label centered along the top, and dashed dividers per section.
fn emit_frame(svg: &mut String, f: &PlacedFrame, opts: &MermaidOptions) {
    let (stroke, so) = stroke_attrs(opts.node_stroke);
    let w = f.x1 - f.x0;
    let hgt = f.y1 - f.y0;

    // Outer rectangle (transparent fill so content shows through).
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{hgt:.2}\" \
         fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        x = f.x0,
        y = f.y0,
    );

    // Keyword tab: a small filled rectangle with a notched bottom-right corner.
    let kw = f.kind.keyword();
    let tab_w = (label_width(kw, opts.font_size_px) + 14.0).max(34.0);
    let notch = 8.0;
    let (tfill, tfo) = fill_attrs(opts.node_fill);
    let tx = f.x0;
    let ty = f.y0;
    let _ = write!(
        svg,
        "<path d=\"M{x:.2},{y:.2} L{xr:.2},{y:.2} L{xr:.2},{yb0:.2} L{xn:.2},{yb1:.2} L{x:.2},{yb1:.2} Z\" \
         fill=\"{tfill}\"{tfo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        x = tx,
        y = ty,
        xr = tx + tab_w,
        xn = tx + tab_w - notch,
        yb0 = ty + FRAME_TAB_H - notch,
        yb1 = ty + FRAME_TAB_H,
    );
    emit_text(svg, kw, tx + tab_w / 2.0 - notch / 2.0, ty + FRAME_TAB_H / 2.0, opts);

    // Opening label centered in the space to the right of the keyword tab, but
    // never overlapping it: if centering would push the label's left edge over
    // the tab, shift the whole label right so it starts past the tab + a gap.
    if !f.label.is_empty() {
        let tab_right = tx + tab_w;
        let label_w = label_width(&f.label, opts.font_size_px);
        let gap = 6.0;
        let mut cx = (tab_right + f.x1) / 2.0;
        let min_cx = tab_right + gap + label_w / 2.0;
        if cx < min_cx {
            cx = min_cx;
        }
        emit_text(svg, &f.label, cx, ty + FRAME_TAB_H / 2.0, opts);
    }

    // Section dividers (dashed horizontal line + that section's label).
    for (dy, slabel) in &f.dividers {
        let _ = write!(
            svg,
            "<line x1=\"{x0:.2}\" y1=\"{dy:.2}\" x2=\"{x1:.2}\" y2=\"{dy:.2}\" \
             stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\" stroke-dasharray=\"3 3\"/>",
            x0 = f.x0,
            x1 = f.x1,
        );
        if !slabel.is_empty() {
            let cx = (f.x0 + f.x1) / 2.0;
            emit_text(svg, slabel, cx, dy + opts.font_size_px * 0.7, opts);
        }
    }
}

/// Draw an activation bar: a narrow themed rectangle on a lifeline, offset
/// horizontally by its nesting level.
fn emit_activation(svg: &mut String, cx: f32, a: &PlacedAct, opts: &MermaidOptions) {
    let (fill, fo) = fill_attrs(opts.node_fill);
    let (stroke, so) = stroke_attrs(opts.node_stroke);
    let dx = a.level as f32 * ACT_NEST_DX;
    let x = cx - ACT_W / 2.0 + dx;
    let y = a.y0;
    let h = (a.y1 - a.y0).max(1.0);
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{ACT_W:.2}\" height=\"{h:.2}\" \
         fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
    );
}

/// Draw a note: a themed rectangle (pale fill) with wrapped/centered text.
fn emit_note(svg: &mut String, pn: &PlacedNote, opts: &MermaidOptions) {
    let (stroke, so) = stroke_attrs(opts.node_stroke);
    // Pale-yellow note fill (mermaid-like), opaque.
    let fill = "rgb(255,255,221)";
    let h = NOTE_GAP - 2.0 * NOTE_PAD_Y;
    let y = pn.y - h / 2.0;
    let w = (pn.x1 - pn.x0).max(NOTE_MIN_W);
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         rx=\"2\" ry=\"2\" fill=\"{fill}\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        x = pn.x0,
    );
    let cx = (pn.x0 + pn.x1) / 2.0;
    emit_text(svg, &pn.note.text, cx, pn.y, opts);
}

/// An open ("V") arrowhead at `(x, y)` pointing in `dir` (+1 right / -1 left).
fn emit_open_head(svg: &mut String, x: f32, y: f32, dir: f32, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let bx = x - dir * ARROW_LEN;
    let _ = write!(
        svg,
        "<path d=\"M{bx:.2},{ty:.2} L{x:.2},{y:.2} L{bx:.2},{by:.2}\" \
         fill=\"none\" stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"/>",
        ty = y - ARROW_HALF,
        by = y + ARROW_HALF,
    );
}

/// A small cross (`✗`) end-marker centered at `(x, y)`.
fn emit_cross(svg: &mut String, x: f32, y: f32, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let r = ARROW_HALF;
    let _ = write!(
        svg,
        "<path d=\"M{x0:.2},{y0:.2} L{x1:.2},{y1:.2} M{x0:.2},{y1:.2} L{x1:.2},{y0:.2}\" \
         fill=\"none\" stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"/>",
        x0 = x - r,
        x1 = x + r,
        y0 = y - r,
        y1 = y + r,
    );
}

// ----------------------------------------------------------------------------
// Text + color helpers (mirrors draw.rs conventions)
// ----------------------------------------------------------------------------

/// A centered single-line label at `(cx, cy)`, routed through the rich-label
/// renderer so message/note/participant labels support markdown (`**bold**`,
/// `*italic*`, `<br>`) and inline math (`$…$`). Plain labels emit a single
/// centered `<text>` identical to the previous output.
fn emit_text(svg: &mut String, label: &str, cx: f32, cy: f32, opts: &MermaidOptions) {
    crate::label::emit(
        svg,
        label,
        cx,
        cy,
        crate::label::Anchor::Middle,
        opts.font_size_px,
        opts.text_color,
        &opts.font_family,
    );
}

/// A left-anchored single-line label at `(x, cy)` (for self-message labels),
/// routed through the rich-label renderer (see [`emit_text`]).
fn emit_text_left(svg: &mut String, label: &str, x: f32, cy: f32, opts: &MermaidOptions) {
    crate::label::emit(
        svg,
        label,
        x,
        cy,
        crate::label::Anchor::Start,
        opts.font_size_px,
        opts.text_color,
        &opts.font_family,
    );
}

/// `fill="rgb(r,g,b)"` plus optional ` fill-opacity`.
fn fill_attrs(color: [u8; 4]) -> (String, String) {
    let [r, g, b, a] = color;
    let fill = format!("rgb({r},{g},{b})");
    let opacity = if a < 255 {
        format!(" fill-opacity=\"{:.4}\"", a as f32 / 255.0)
    } else {
        String::new()
    };
    (fill, opacity)
}

/// Same as [`fill_attrs`] but the opacity attribute is `stroke-opacity`.
fn stroke_attrs(color: [u8; 4]) -> (String, String) {
    let [r, g, b, a] = color;
    let stroke = format!("rgb({r},{g},{b})");
    let opacity = if a < 255 {
        format!(" stroke-opacity=\"{:.4}\"", a as f32 / 255.0)
    } else {
        String::new()
    };
    (stroke, opacity)
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------
