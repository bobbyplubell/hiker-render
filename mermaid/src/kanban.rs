//! `kanban` diagram (self-contained: parse + layout + draw). No dagre.
//!
//! Mermaid kanban syntax (the subset we support):
//! ```text
//! kanban
//!   Todo
//!     Create JISON
//!     Update DB function
//!   id7[In progress]
//!     id8[Design grammar]
//! ```
//! The header line is `kanban`. **Indentation defines structure**: the first
//! (shallowest) indentation level holds **column headers**; any line indented
//! deeper than its column is a **card** in that column. Both columns and cards may
//! use the `id[Text]` bracket form (the leading id is dropped, the bracket content
//! is the displayed text) or be bare text (the whole trimmed line is the text).
//!
//! Layout: columns are laid out left→right, each a fixed-width vertical lane with a
//! colored **header box** at the top and its **cards** stacked as rounded rectangles
//! below. Card text is word-wrapped to the column width; card height grows with the
//! wrapped line count. Board height is the tallest column.
//!
//! Skipped (noted, not rendered): a card's `@{ ... }` metadata block (assigned /
//! priority / icon / ticket) is parsed-and-discarded — no badges are drawn.
//!
//! Styling: `classDef <name> <props>`, `class <id…> <name>`, the `id:::name`
//! shorthand on a card/column line, and inline `style <id> <props>` are parsed
//! and resolved onto a per-node [`ElemStyle`] (reusing the shared flowchart
//! machinery in [`crate::parse::directives`]). A card/column box picks up
//! fill/stroke/stroke-width/dashed and its label picks up
//! color/font-weight/font-style/text-decoration/font-size/opacity, each falling
//! back to the theme default when unset (same pattern as the flowchart renderer).
//! Targeting requires the leading `id` form (`id[Text]`); a bare-text card has no
//! id to address from a `class`/`style` line.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/kanban/` for the upstream
//! parser/renderer this mirrors.

use std::fmt::Write as _;

use crate::model::ElemStyle;
use crate::parse::directives::{merge_style, parse_style_props};
use crate::svgutil::{
    element_opacity_attr, escape, opacity_attr, rgb, text_size, text_style_attrs,
};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One card within a column: its display text (metadata is discarded), the
/// optional source id used to target it from `class`/`style`, and resolved style.
#[derive(Clone, Debug, Default, PartialEq)]
struct Card {
    text: String,
    /// Leading id from an `id[Text]` form (empty for bare-text cards).
    id: String,
    style: ElemStyle,
}

/// One board column: its header title, optional source id, style, and its cards.
#[derive(Clone, Debug, Default, PartialEq)]
struct Column {
    title: String,
    /// Leading id from an `id[Text]` form (empty for bare-text columns).
    id: String,
    style: ElemStyle,
    cards: Vec<Card>,
}

/// A parsed kanban board: the columns left→right.
#[derive(Clone, Debug, PartialEq)]
struct Board {
    columns: Vec<Column>,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// A tab counts as this many columns of indentation.
const TAB_WIDTH: usize = 2;

/// Count leading-whitespace columns of a raw line (spaces = 1, tab = `TAB_WIDTH`).
fn indent_of(raw: &str) -> usize {
    let mut col = 0usize;
    for c in raw.chars() {
        match c {
            ' ' => col += 1,
            '\t' => col += TAB_WIDTH,
            _ => break,
        }
    }
    col
}

/// A parsed card/column item: its display text, the leading `id` (empty when the
/// line is bare text with no `id[..]` wrapper), and the optional `:::class` name.
struct Item {
    text: String,
    id: String,
    class: Option<String>,
}

/// Parse a card/column content line into an [`Item`]. Forms:
/// - `id[Text]` / `id(Text)` → id + bracket-inner text.
/// - bare `Text` → text is the trimmed line, id is empty.
/// A trailing `:::class` (on either form) is captured as the class name and
/// stripped from the text.
fn parse_item(content: &str) -> Item {
    let mut content = content.trim();
    let mut class = None;
    if let Some(i) = content.find(":::") {
        let name = content[i + 3..].trim();
        if !name.is_empty() {
            class = Some(name.to_string());
        }
        content = content[..i].trim_end();
    }
    for (open, close) in [("[", "]"), ("(", ")")] {
        if let Some(start) = content.find(open) {
            let id = content[..start].trim().to_string();
            let after = &content[start + open.len()..];
            if let Some(end) = after.rfind(close) {
                let inner = after[..end].trim();
                if !inner.is_empty() {
                    return Item { text: inner.to_string(), id, class };
                }
            }
        }
    }
    Item { text: content.to_string(), id: String::new(), class }
}

/// Parse kanban source into a [`Board`]. Returns `Err(message)` when the `kanban`
/// header is missing.
///
/// Indentation model: the first content line after the header sets the "column"
/// indent level. Any subsequent line at that same (or shallower) indent starts a
/// new column; any line indented deeper is a card of the current column.
fn parse_kanban(src: &str) -> Result<Board, String> {
    use std::collections::HashMap;

    let mut saw_header = false;
    let mut columns: Vec<Column> = Vec::new();
    let mut column_indent: Option<usize> = None;
    // Set when we're inside a card's `@{ ... }` metadata block: skip lines until
    // the closing `}`.
    let mut in_meta = false;

    // Styling directives (resolved after parsing so a `classDef` may follow the
    // `class`/`:::` that references it). Mirrors the flowchart two-pass.
    let mut class_defs: HashMap<String, ElemStyle> = HashMap::new();
    // `(node id, class name)` from `class A,B name`, `id:::name`.
    let mut class_assignments: Vec<(String, String)> = Vec::new();
    // Inline `style <id> <props>` overrides.
    let mut inline: Vec<(String, ElemStyle)> = Vec::new();

    for raw in src.lines() {
        let no_comment = raw.split("%%").next().unwrap_or("");
        let line = no_comment.trim();
        if line.is_empty() {
            continue;
        }

        if !saw_header {
            let first = line.split_whitespace().next().unwrap_or("");
            if first != "kanban" {
                return Err(format!("expected 'kanban' header, got: {line:?}"));
            }
            saw_header = true;
            continue;
        }

        // Inside a `@{ ... }` metadata block → swallow until the closing brace.
        if in_meta {
            if line.contains('}') {
                in_meta = false;
            }
            continue;
        }
        // A card metadata block `id@{ ... }` (possibly single-line) → skip it.
        if line.contains("@{") {
            if !line.contains('}') {
                in_meta = true;
            }
            continue;
        }
        // Styling directives, collected for the post-parse resolve.
        let first = line.split_whitespace().next().unwrap_or("");
        match first {
            "classDef" => {
                // classDef <name> <prop:val,...>
                let rest = line["classDef".len()..].trim_start();
                let mut parts = rest.splitn(2, char::is_whitespace);
                if let Some(name) = parts.next().filter(|n| !n.is_empty()) {
                    let props = parts.next().unwrap_or("");
                    class_defs.insert(name.to_string(), parse_style_props(props));
                }
                continue;
            }
            "class" => {
                // class <id1>,<id2>,... <className>
                let rest = line["class".len()..].trim_start();
                if let Some(sp) = rest.rfind(char::is_whitespace) {
                    let ids = rest[..sp].trim();
                    let class_name = rest[sp..].trim();
                    if !class_name.is_empty() {
                        for id in ids.split(',') {
                            let id = id.trim();
                            if !id.is_empty() {
                                class_assignments.push((id.to_string(), class_name.to_string()));
                            }
                        }
                    }
                }
                continue;
            }
            "style" => {
                // style <id> <prop:val,...>
                let rest = line["style".len()..].trim_start();
                let mut parts = rest.splitn(2, char::is_whitespace);
                if let Some(id) = parts.next().filter(|n| !n.is_empty()) {
                    let props = parts.next().unwrap_or("");
                    inline.push((id.to_string(), parse_style_props(props)));
                }
                continue;
            }
            _ => {}
        }

        let item = parse_item(line);
        // An `id:::class` shorthand on the line is recorded as a class assignment.
        if let (Some(class), false) = (&item.class, item.id.is_empty()) {
            class_assignments.push((item.id.clone(), class.clone()));
        }

        let indent = indent_of(no_comment);
        match column_indent {
            None => {
                // First content line defines the column indent level.
                column_indent = Some(indent);
                columns.push(Column { title: item.text, id: item.id, ..Default::default() });
            }
            Some(ci) => {
                if indent <= ci {
                    // Same/shallower → a new column.
                    columns.push(Column { title: item.text, id: item.id, ..Default::default() });
                } else if let Some(col) = columns.last_mut() {
                    // Deeper → a card of the current column.
                    col.cards.push(Card { text: item.text, id: item.id, ..Default::default() });
                }
            }
        }
    }

    if !saw_header {
        return Err("empty input / no 'kanban' header".to_string());
    }

    // Resolve styles: classDef-via-class first, then inline `style` on top
    // (field-by-field), matching the flowchart apply order.
    let apply = |id: &str, style: &ElemStyle, columns: &mut Vec<Column>| {
        for col in columns.iter_mut() {
            if col.id == id {
                merge_style(&mut col.style, style);
            }
            for card in col.cards.iter_mut() {
                if card.id == id {
                    merge_style(&mut card.style, style);
                }
            }
        }
    };
    for (id, class_name) in &class_assignments {
        if let Some(style) = class_defs.get(class_name) {
            apply(id, style, &mut columns);
        }
    }
    for (id, style) in &inline {
        apply(id, style, &mut columns);
    }

    Ok(Board { columns })
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

/// Margin around the whole drawing, px.
const MARGIN: f32 = 24.0;
/// Fixed column width, px.
const COL_W: f32 = 180.0;
/// Horizontal gap between columns, px.
const COL_GAP: f32 = 16.0;
/// Vertical gap between the header and the first card, and between cards, px.
const CARD_GAP: f32 = 10.0;
/// Border stroke width, px.
const STROKE_W: f32 = 1.5;
/// Corner radius for rounded boxes, px.
const CORNER_R: f32 = 6.0;

/// A small fixed palette (straight RGBA) cycled per column header.
const PALETTE: [[u8; 4]; 8] = [
    [236, 236, 255, 255],
    [255, 236, 236, 255],
    [236, 255, 236, 255],
    [255, 248, 220, 255],
    [220, 248, 255, 255],
    [248, 236, 255, 255],
    [255, 236, 248, 255],
    [236, 255, 248, 255],
];

/// Pick a palette color (RGBA) for a column index. Prefers the active theme's
/// `series_palette` when set, falling back to the local [`PALETTE`].
fn palette_color(opts: &MermaidOptions, i: usize) -> [u8; 4] {
    if !opts.series_palette.is_empty() {
        opts.series_palette[i % opts.series_palette.len()]
    } else {
        PALETTE[i % PALETTE.len()]
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render mermaid kanban source to an SVG document.
pub fn render_kanban(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let board = parse_kanban(src).map_err(MermaidError::Parse)?;
    if board.columns.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let px = opts.node_padding_x;
    let py = opts.node_padding_y;
    let line_h = fs * 1.2;
    let inner_w = COL_W - 2.0 * px;

    let header_h = line_h + 2.0 * py;
    let card_h = |text: &str| -> f32 {
        let lines = wrap_lines(text, inner_w, fs);
        lines.len() as f32 * line_h + 2.0 * py
    };

    let n = board.columns.len();
    let col_x = |i: usize| MARGIN + i as f32 * (COL_W + COL_GAP);
    let header_top = MARGIN;
    let cards_top = header_top + header_h + CARD_GAP;

    // Tallest column → board height.
    let mut max_col_h = 0.0f32;
    for col in &board.columns {
        let mut y = 0.0f32;
        for c in &col.cards {
            y += card_h(&c.text) + CARD_GAP;
        }
        max_col_h = max_col_h.max(y);
    }

    let width = MARGIN + n as f32 * (COL_W + COL_GAP) - COL_GAP + MARGIN;
    let height = cards_top + max_col_h + MARGIN;
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    for (i, col) in board.columns.iter().enumerate() {
        let x = col_x(i);
        // Header fill defaults to the cycled palette; an explicit classDef/style
        // `fill:` overrides it. Stroke/width fall back to the theme.
        let color = col.style.fill.unwrap_or_else(|| palette_color(opts, i));
        draw_box(&mut svg, x, header_top, COL_W, header_h, color, &col.style, opts);
        draw_text(
            &mut svg,
            x + COL_W / 2.0,
            header_top + header_h / 2.0,
            &col.title,
            fs,
            opts,
            true,
            &col.style,
        );

        // Cards stacked below.
        let mut cy = cards_top;
        for card in &col.cards {
            let lines = wrap_lines(&card.text, inner_w, fs);
            let bh = lines.len() as f32 * line_h + 2.0 * py;
            let cfill = card.style.fill.unwrap_or(opts.node_fill);
            draw_box(&mut svg, x, cy, COL_W, bh, cfill, &card.style, opts);
            let total = line_h * lines.len() as f32;
            let mut ty = cy + bh / 2.0 - total / 2.0 + line_h / 2.0;
            for ln in &lines {
                draw_text(&mut svg, x + COL_W / 2.0, ty, ln, fs, opts, false, &card.style);
                ty += line_h;
            }
            cy += bh + CARD_GAP;
        }
    }

    svg.push_str("</svg>");

    Ok(MermaidRender { svg, width_px: w, height_px: h })
}

/// Greedy word-wrap of `text` to fit `max_w` px at `font_size`, using the
/// font-free advance heuristic from [`text_size`]. Always returns >= 1 line; a
/// single word wider than `max_w` is kept whole on its own line.
fn wrap_lines(text: &str, max_w: f32, font_size: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if text_size(&candidate, font_size).0 <= max_w || cur.is_empty() {
            cur = candidate;
        } else {
            lines.push(cur);
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Draw a rounded box (header or card) with `fill` and per-node `style` applied:
/// stroke/stroke-width fall back to the theme, `stroke-dasharray` when `dashed`,
/// and the element `opacity` fades the whole box.
fn draw_box(
    svg: &mut String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fill: [u8; 4],
    style: &ElemStyle,
    opts: &MermaidOptions,
) {
    let stroke = style.stroke.unwrap_or(opts.node_stroke);
    let sw = style.stroke_width.unwrap_or(STROKE_W);
    let dash = if style.dashed { " stroke-dasharray=\"4 3\"" } else { "" };
    let op = element_opacity_attr(style.opacity);
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         rx=\"{CORNER_R}\" ry=\"{CORNER_R}\" \
         fill=\"{fillc}\"{fo} stroke=\"{strokec}\"{so} stroke-width=\"{sw}\"{dash}{op}/>",
        fillc = rgb(fill),
        fo = opacity_attr("fill-opacity", fill),
        strokec = rgb(stroke),
        so = opacity_attr("stroke-opacity", stroke),
    );
}

/// Draw a single centered text line. `bold` emphasises column headers; a per-node
/// `style` overrides the text color, font size, weight/style/decoration when set.
fn draw_text(
    svg: &mut String,
    cx: f32,
    cy: f32,
    text: &str,
    font_size: f32,
    opts: &MermaidOptions,
    bold: bool,
    style: &ElemStyle,
) {
    let [tr, tg, tb, _] = style.text_color.unwrap_or(opts.text_color);
    let fs = style.font_size.unwrap_or(font_size);
    // A classDef/style `font-weight` overrides the default header bolding; bold
    // headers fall back to `bold` when no explicit weight is set.
    let extra = text_style_attrs(style);
    // A header is bold by default; an explicit `font-weight` (carried in `extra`)
    // takes precedence, so only add the default bold when none was set.
    let weight = if bold && style.font_weight.is_none() {
        " font-weight=\"bold\""
    } else {
        ""
    };
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" \
         dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{fs}\"{weight}{extra} \
         fill=\"rgb({tr},{tg},{tb})\">{txt}</text>",
        family = escape(&opts.font_family),
        txt = escape(text),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "kanban
  Todo
    Create JISON
    Update DB function
  id7[In progress]
    id8[Design grammar]
";

    #[test]
    fn parses_columns_and_cards_by_indentation() {
        let board = parse_kanban(SAMPLE).expect("parse");
        assert_eq!(board.columns.len(), 2);
        assert_eq!(board.columns[0].title, "Todo");
        assert_eq!(board.columns[0].cards.len(), 2);
        assert_eq!(board.columns[0].cards[0].text, "Create JISON");
        assert_eq!(board.columns[0].cards[1].text, "Update DB function");
        // Bracket form: id dropped, inner text used for both header and card.
        assert_eq!(board.columns[1].title, "In progress");
        assert_eq!(board.columns[1].cards.len(), 1);
        assert_eq!(board.columns[1].cards[0].text, "Design grammar");
    }

    #[test]
    fn tabs_count_as_indentation() {
        let src = "kanban\nTodo\n\tCard A\n";
        let board = parse_kanban(src).expect("parse");
        assert_eq!(board.columns.len(), 1);
        assert_eq!(board.columns[0].cards.len(), 1);
        assert_eq!(board.columns[0].cards[0].text, "Card A");
    }

    #[test]
    fn metadata_block_is_skipped() {
        let src = "kanban\n  Todo\n    id2[A card]\n    id2@{\n      assigned: knsv\n      priority: high\n    }\n  Done\n";
        let board = parse_kanban(src).expect("parse");
        assert_eq!(board.columns.len(), 2);
        assert_eq!(board.columns[0].title, "Todo");
        assert_eq!(board.columns[0].cards.len(), 1);
        assert_eq!(board.columns[0].cards[0].text, "A card");
        assert_eq!(board.columns[1].title, "Done");
    }

    #[test]
    fn style_directive_does_not_create_a_column() {
        // A `style` line targets a node, it isn't itself a column/card.
        let src = "kanban\n  Todo\n    n2[A card]\n  style n2 stroke:#AA00FF,fill:#E1BEE7\n";
        let board = parse_kanban(src).expect("parse");
        assert_eq!(board.columns.len(), 1);
        assert_eq!(board.columns[0].cards.len(), 1);
        // The inline style resolved onto the card by id.
        assert_eq!(board.columns[0].cards[0].style.fill, Some([0xE1, 0xBE, 0xE7, 255]));
        assert_eq!(board.columns[0].cards[0].style.stroke, Some([0xAA, 0x00, 0xFF, 255]));
    }

    #[test]
    fn classdef_and_class_apply_to_card() {
        // `class <id> <name>` targets a card by its leading id.
        let src = "kanban\n  Todo\n    t1[Task one]\n    t2[Task two]\n  \
                   classDef hot fill:#ffcdd2,stroke:#c62828,stroke-width:3px\n  class t1 hot\n";
        let board = parse_kanban(src).expect("parse");
        let cards = &board.columns[0].cards;
        assert_eq!(cards[0].style.fill, Some([0xff, 0xcd, 0xd2, 255]));
        assert_eq!(cards[0].style.stroke, Some([0xc6, 0x28, 0x28, 255]));
        assert_eq!(cards[0].style.stroke_width, Some(3.0));
        // The unstyled card keeps the default style.
        assert_eq!(cards[1].style, ElemStyle::default());
    }

    #[test]
    fn triple_colon_shorthand_on_card() {
        let src = "kanban\n  Todo\n    t1[Task]:::hot\n  classDef hot fill:#00ff00\n";
        let board = parse_kanban(src).expect("parse");
        assert_eq!(board.columns[0].cards[0].text, "Task");
        assert_eq!(board.columns[0].cards[0].style.fill, Some([0, 255, 0, 255]));
    }

    #[test]
    fn classdef_can_style_a_column_header() {
        let src = "kanban\n  c1[Todo]\n    A card\n  classDef hdr fill:#0000ff\n  class c1 hdr\n";
        let board = parse_kanban(src).expect("parse");
        assert_eq!(board.columns[0].title, "Todo");
        assert_eq!(board.columns[0].style.fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn styled_card_fill_appears_in_svg() {
        let src = "kanban\n  Todo\n    t1[Task]\n  classDef hot fill:#ffcdd2,stroke:#c62828\n  class t1 hot\n";
        let r = render_kanban(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("fill=\"rgb(255,205,210)\""), "card fill in svg: {}", r.svg);
        assert!(r.svg.contains("stroke=\"rgb(198,40,40)\""), "card stroke in svg: {}", r.svg);
    }

    #[test]
    fn render_well_formed_svg() {
        let r = render_kanban(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_one_header_per_column_and_one_box_per_card() {
        let r = render_kanban(SAMPLE, &MermaidOptions::default()).expect("render");
        // 2 headers + 3 cards = 5 rects.
        assert_eq!(r.svg.matches("<rect").count(), 5, "boxes; svg={}", r.svg);
        // Titles + card texts present.
        assert!(r.svg.contains(">Todo<"));
        assert!(r.svg.contains(">In progress<"));
        assert!(r.svg.contains(">Create JISON<"));
        assert!(r.svg.contains(">Design grammar<"));
    }

    #[test]
    fn xml_escapes_text() {
        let src = "kanban\n  Todo\n    A & B <x>\n";
        let r = render_kanban(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("A &amp; B &lt;x&gt;"), "got: {}", r.svg);
        assert!(!r.svg.contains("A & B <x>"));
    }

    #[test]
    fn empty_input_errors() {
        match render_kanban("", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn missing_header_errors() {
        let r = render_kanban("graph TD\nA-->B\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    #[test]
    fn header_only_is_empty() {
        let r = render_kanban("kanban\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Empty)));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let a = render_kanban(SAMPLE, &opts).expect("a");
        let b = render_kanban(SAMPLE, &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
