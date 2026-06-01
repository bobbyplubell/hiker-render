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
//! priority / icon / ticket) is parsed-and-discarded — no badges are drawn;
//! `style ...`/`class` directives and `:::class` are ignored.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/kanban/` for the upstream
//! parser/renderer this mirrors.

use std::fmt::Write as _;

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One card within a column: just its display text (metadata is discarded).
#[derive(Clone, Debug, PartialEq)]
struct Card {
    text: String,
}

/// One board column: its header title and its cards (in source order).
#[derive(Clone, Debug, PartialEq)]
struct Column {
    title: String,
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

/// Strip an optional `id[Text]` / `id(Text)` bracket wrapper, returning the inner
/// text. A leading id token before the bracket is dropped. Bare text (no bracket)
/// is returned trimmed as-is. Trailing `:::class` styling is removed.
fn node_text(content: &str) -> String {
    let mut content = content.trim();
    if let Some(i) = content.find(":::") {
        content = content[..i].trim_end();
    }
    for (open, close) in [("[", "]"), ("(", ")")] {
        if let Some(start) = content.find(open) {
            let after = &content[start + open.len()..];
            if let Some(end) = after.rfind(close) {
                let inner = after[..end].trim();
                if !inner.is_empty() {
                    return inner.to_string();
                }
            }
        }
    }
    content.to_string()
}

/// Parse kanban source into a [`Board`]. Returns `Err(message)` when the `kanban`
/// header is missing.
///
/// Indentation model: the first content line after the header sets the "column"
/// indent level. Any subsequent line at that same (or shallower) indent starts a
/// new column; any line indented deeper is a card of the current column.
fn parse_kanban(src: &str) -> Result<Board, String> {
    let mut saw_header = false;
    let mut columns: Vec<Column> = Vec::new();
    let mut column_indent: Option<usize> = None;
    // Set when we're inside a card's `@{ ... }` metadata block: skip lines until
    // the closing `}`.
    let mut in_meta = false;

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
        // `style ...` / `class ...` directives → ignore.
        let first = line.split_whitespace().next().unwrap_or("");
        if first == "style" || first == "class" || first == "classDef" {
            continue;
        }

        let indent = indent_of(no_comment);
        match column_indent {
            None => {
                // First content line defines the column indent level.
                column_indent = Some(indent);
                columns.push(Column { title: node_text(line), cards: Vec::new() });
            }
            Some(ci) => {
                if indent <= ci {
                    // Same/shallower → a new column.
                    columns.push(Column { title: node_text(line), cards: Vec::new() });
                } else if let Some(col) = columns.last_mut() {
                    // Deeper → a card of the current column.
                    col.cards.push(Card { text: node_text(line) });
                }
            }
        }
    }

    if !saw_header {
        return Err("empty input / no 'kanban' header".to_string());
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
        let color = PALETTE[i % PALETTE.len()];

        // Header box.
        let _ = write!(
            svg,
            "<rect x=\"{x:.2}\" y=\"{header_top:.2}\" width=\"{COL_W:.2}\" \
             height=\"{header_h:.2}\" rx=\"{CORNER_R}\" ry=\"{CORNER_R}\" \
             fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
            fill = rgb(color),
            fo = opacity_attr("fill-opacity", color),
            stroke = rgb(opts.node_stroke),
            so = opacity_attr("stroke-opacity", opts.node_stroke),
        );
        draw_text(&mut svg, x + COL_W / 2.0, header_top + header_h / 2.0, &col.title, fs, opts, true);

        // Cards stacked below.
        let mut cy = cards_top;
        for card in &col.cards {
            let lines = wrap_lines(&card.text, inner_w, fs);
            let bh = lines.len() as f32 * line_h + 2.0 * py;
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{cy:.2}\" width=\"{COL_W:.2}\" height=\"{bh:.2}\" \
                 rx=\"{CORNER_R}\" ry=\"{CORNER_R}\" \
                 fill=\"{cfill}\"{cfo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
                cfill = rgb(opts.node_fill),
                cfo = opacity_attr("fill-opacity", opts.node_fill),
                stroke = rgb(opts.node_stroke),
                so = opacity_attr("stroke-opacity", opts.node_stroke),
            );
            let total = line_h * lines.len() as f32;
            let mut ty = cy + bh / 2.0 - total / 2.0 + line_h / 2.0;
            for ln in &lines {
                draw_text(&mut svg, x + COL_W / 2.0, ty, ln, fs, opts, false);
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

/// Draw a single centered text line. `bold` emphasises column headers.
fn draw_text(svg: &mut String, cx: f32, cy: f32, text: &str, font_size: f32, opts: &MermaidOptions, bold: bool) {
    let [tr, tg, tb, _] = opts.text_color;
    let weight = if bold { " font-weight=\"bold\"" } else { "" };
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" \
         dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{fs}\"{weight} \
         fill=\"rgb({tr},{tg},{tb})\">{txt}</text>",
        family = escape(&opts.font_family),
        fs = font_size,
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
    fn style_directive_is_ignored() {
        let src = "kanban\n  Todo\n    A card\n  style n2 stroke:#AA00FF,fill:#E1BEE7\n";
        let board = parse_kanban(src).expect("parse");
        assert_eq!(board.columns.len(), 1);
        assert_eq!(board.columns[0].cards.len(), 1);
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
