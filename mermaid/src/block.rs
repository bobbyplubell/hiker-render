//! `block` diagram (self-contained: parse + grid self-layout + draw, no dagre).
//!
//! Mermaid block syntax (the subset we support):
//! ```text
//! block-beta
//!   columns 3
//!   a b c
//!   d["Wide block"]:2 space
//!   e("Round") f{"Diamond"}
//!   a --> b
//!   a -- "label" --> c
//! ```
//!
//! Header keyword is `block-beta` (preferred) or `block` (both map to
//! `BLOCK_DIAGRAM_KEY` upstream — see
//! `references/mermaid/packages/mermaid/src/diagrams/block/parser/block.jison`).
//!
//! Supported:
//! - `columns <n>` sets the grid column count for subsequent blocks. Default is
//!   the number of top-level blocks declared (mermaid's `auto`-ish behavior).
//! - Block declarations: bare ids (`a b c`), shaped/labeled forms
//!   `id["Label"]` (rect), `id("Round")` (rounded), `id{"Diamond"}` (diamond),
//!   and a width span `id["Label"]:N` / `id:N` (block spans N columns).
//! - `space` leaves one empty grid cell; `space:N` spans N empty cells.
//! - Edges between blocks: `a --> b` and `a -- "label" --> b`.
//!
//! Skipped (noted): nested `block:id ... end` composite blocks are FLATTENED —
//! the `block:`/`end` tokens are dropped and the inner declarations are treated
//! as top-level blocks (no sub-grid nesting). Also skipped: `<[ ]>` block-arrows,
//! and arrow directions beyond the basic `-->` / `--` / `---` forms (all treated
//! as a simple directed/undirected edge).
//!
//! Styling: `classDef <name> <props>`, `class <id…> <name>`, the `id:::name`
//! shorthand on a block declaration, and inline `style <id> <props>` are parsed
//! and resolved onto a per-block [`ElemStyle`] (reusing the shared flowchart
//! machinery in [`crate::parse::directives`]). A block picks up
//! fill/stroke/stroke-width/dashed and its label picks up
//! color/font-weight/font-style/text-decoration/font-size/opacity, each falling
//! back to the theme default when unset (same pattern as the flowchart renderer).
//! Edges are not classDef/`linkStyle`-targetable here (no edge ids).
//!
//! Layout is a row-major grid: each block consumes `span` columns, `space`
//! consumes empty cells, and the row wraps when it fills. Cell width is uniform
//! (the widest single-column block), a spanning block is correspondingly wider.

use std::collections::HashMap;
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

/// A block's drawn shape, from its declaration brackets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// `id["..."]` or a bare `id` — a rectangle.
    Rect,
    /// `id("...")` — a rounded rectangle.
    Round,
    /// `id{"..."}` — a diamond.
    Diamond,
}

/// One declared block: an id, its label (defaults to the id), shape, how many
/// grid columns it spans, and resolved per-block style overrides.
#[derive(Clone, Debug, PartialEq)]
struct Block {
    id: String,
    label: String,
    shape: Shape,
    span: usize,
    style: ElemStyle,
}

/// An edge between two block ids, with an optional label and whether it is
/// directed (drawn with an arrowhead).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Edge {
    start: String,
    end: String,
    label: String,
    arrow: bool,
}

/// A parsed block diagram: the explicit `columns` count (if any), the blocks in
/// declaration order, and the edges.
#[derive(Clone, Debug, PartialEq)]
struct BlockDiagram {
    columns: Option<usize>,
    blocks: Vec<Block>,
    edges: Vec<Edge>,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse mermaid block source into a [`BlockDiagram`]. Returns `Err(message)`
/// when the header is missing/malformed.
fn parse_block(src: &str) -> Result<BlockDiagram, String> {
    let mut columns: Option<usize> = None;
    let mut blocks: Vec<Block> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut saw_header = false;
    let mut space_counter = 0usize;

    // Styling directives, resolved after parsing (two-pass: a `classDef` may
    // follow the `class`/`:::` that references it). Mirrors the flowchart.
    let mut class_defs: HashMap<String, ElemStyle> = HashMap::new();
    let mut class_assignments: Vec<(String, String)> = Vec::new();
    let mut inline: Vec<(String, ElemStyle)> = Vec::new();

    for raw in src.lines() {
        // Strip `%%` comments and surrounding whitespace.
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if !saw_header {
            // First non-blank line must be the header keyword.
            let head = line.split_whitespace().next().unwrap_or("");
            if head != "block-beta" && head != "block" {
                return Err(format!("expected `block-beta` or `block` header, got {head:?}"));
            }
            saw_header = true;
            // Anything after the header keyword on the same line is ignored.
            continue;
        }

        // `columns <n>` sets the grid width for subsequent blocks.
        if let Some(rest) = line.strip_prefix("columns") {
            let rest = rest.trim();
            if let Ok(n) = rest.parse::<usize>() {
                if n > 0 {
                    columns = Some(n);
                }
            }
            // `columns auto` (or unparsable) leaves it as the default.
            continue;
        }

        // Styling / nesting keyword lines.
        let first = line.split_whitespace().next().unwrap_or("");
        match first {
            "classDef" => {
                let rest = line["classDef".len()..].trim_start();
                let mut parts = rest.splitn(2, char::is_whitespace);
                if let Some(name) = parts.next().filter(|n| !n.is_empty()) {
                    let props = parts.next().unwrap_or("");
                    class_defs.insert(name.to_string(), parse_style_props(props));
                }
                continue;
            }
            "class" => {
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
                let rest = line["style".len()..].trim_start();
                let mut parts = rest.splitn(2, char::is_whitespace);
                if let Some(id) = parts.next().filter(|n| !n.is_empty()) {
                    let props = parts.next().unwrap_or("");
                    inline.push((id.to_string(), parse_style_props(props)));
                }
                continue;
            }
            "end" => continue,
            _ => {}
        }

        parse_statement_line(
            line,
            &mut blocks,
            &mut edges,
            &mut space_counter,
            &mut class_assignments,
        );
    }

    if !saw_header {
        return Err("empty input / missing block header".to_string());
    }

    // Resolve: classDef-via-class first, then inline `style` on top.
    for (id, class_name) in &class_assignments {
        if let Some(style) = class_defs.get(class_name) {
            if let Some(b) = blocks.iter_mut().find(|b| b.id == *id) {
                merge_style(&mut b.style, style);
            }
        }
    }
    for (id, style) in &inline {
        if let Some(b) = blocks.iter_mut().find(|b| b.id == *id) {
            merge_style(&mut b.style, style);
        }
    }

    Ok(BlockDiagram {
        columns,
        blocks,
        edges,
    })
}

/// Parse one statement line, which may declare several space-separated blocks
/// and/or one or more edges (`a --> b -- "l" --> c`). Tokens are pulled left to
/// right; an edge operator links the previous block id to the next one.
fn parse_statement_line(
    line: &str,
    blocks: &mut Vec<Block>,
    edges: &mut Vec<Edge>,
    space_counter: &mut usize,
    class_assignments: &mut Vec<(String, String)>,
) {
    // Flatten a leading `block:` composite-open token to its inner content.
    let line = if let Some(rest) = line.strip_prefix("block:") {
        rest.trim_start()
    } else {
        line
    };

    let tokens = tokenize(line);
    let mut i = 0usize;
    // The id of the most recent block, for connecting an edge's start.
    let mut prev_id: Option<String> = None;

    while i < tokens.len() {
        match &tokens[i] {
            Token::Edge { label, arrow } => {
                // Connect prev_id → next block token (declare it if new).
                let start = prev_id.clone();
                // Find the next block declaration token.
                if i + 1 < tokens.len() {
                    if let Token::Block(decl) = &tokens[i + 1] {
                        let id = decl.id.clone();
                        ensure_block(blocks, decl);
                        record_class(class_assignments, decl);
                        if let Some(s) = start {
                            edges.push(Edge {
                                start: s,
                                end: id.clone(),
                                label: label.clone(),
                                arrow: *arrow,
                            });
                        }
                        prev_id = Some(id);
                        i += 2;
                        continue;
                    }
                }
                i += 1;
            }
            Token::Space(n) => {
                for _ in 0..*n {
                    let id = format!("__space_{space_counter}");
                    *space_counter += 1;
                    blocks.push(Block {
                        id,
                        label: String::new(),
                        shape: Shape::Rect,
                        span: 1,
                        style: ElemStyle::default(),
                    });
                }
                prev_id = None;
                i += 1;
            }
            Token::Block(decl) => {
                ensure_block(blocks, decl);
                record_class(class_assignments, decl);
                prev_id = Some(decl.id.clone());
                i += 1;
            }
        }
    }
}

/// A block declaration parsed from a token (before grid placement).
#[derive(Clone, Debug)]
struct Decl {
    id: String,
    label: String,
    shape: Shape,
    span: usize,
    /// `true` for a real declaration `id[...]`, `false` for a bare reference.
    /// Bare references that name an existing block must not overwrite it.
    explicit: bool,
    /// An `id:::class` shorthand class name attached to this declaration.
    class: Option<String>,
}

/// A `space` cell never collides with a real block (its synthetic id is unique),
/// so it is handled as its own token. Real/blank cells are `Block`.
#[derive(Clone, Debug)]
enum Token {
    Block(Decl),
    Space(usize),
    Edge { label: String, arrow: bool },
}

/// Add or update a block from a declaration. A bare reference to an existing id
/// is a no-op; an explicit shaped/labeled form updates the existing block.
fn ensure_block(blocks: &mut Vec<Block>, decl: &Decl) {
    if let Some(b) = blocks.iter_mut().find(|b| b.id == decl.id) {
        if decl.explicit {
            b.label = decl.label.clone();
            b.shape = decl.shape;
            b.span = decl.span;
        }
        return;
    }
    blocks.push(Block {
        id: decl.id.clone(),
        label: decl.label.clone(),
        shape: decl.shape,
        span: decl.span,
        style: ElemStyle::default(),
    });
}

/// Record an `id:::class` shorthand attached to a declaration as a class
/// assignment for the post-parse style resolve.
fn record_class(class_assignments: &mut Vec<(String, String)>, decl: &Decl) {
    if let Some(class) = &decl.class {
        class_assignments.push((decl.id.clone(), class.clone()));
    }
}

/// Split a statement line into block/space/edge tokens.
fn tokenize(line: &str) -> Vec<Token> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<Token> = Vec::new();
    let mut i = 0usize;
    let n = chars.len();

    while i < n {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Edge operators: a run starting with `-`, `=`, `.`, optionally preceded
        // by an `x`/`o`/`<` marker (handled below via the leading id parse).
        if c == '-' || c == '=' {
            let (tok, next) = parse_edge(&chars, i);
            out.push(tok);
            i = next;
            continue;
        }
        // Otherwise, an id followed by an optional shape and optional `:N` span.
        let (tok, next) = parse_block_or_space(&chars, i);
        out.push(tok);
        i = next;
    }
    out
}

/// Parse an edge operator starting at `i`. Recognizes `-->`, `---`, `--`, `==>`,
/// `===`, plus an inline `-- "label" -->` (the label being the next quoted
/// string up to the closing operator). Returns the token and the next index.
fn parse_edge(chars: &[char], start: usize) -> (Token, usize) {
    let n = chars.len();
    let mut i = start;
    // Consume the leading operator run (`--`, `==`, `-`, etc.).
    while i < n && (chars[i] == '-' || chars[i] == '=' || chars[i] == '.') {
        i += 1;
    }
    let mut arrow = false;
    // An immediate `>` (e.g. `-->`) means a directed arrow.
    if i < n && chars[i] == '>' {
        arrow = true;
        i += 1;
    }
    // Skip whitespace, then look for an inline label `"..."` followed by a
    // closing operator (`-- "x" -->`).
    let mut label = String::new();
    let mut j = i;
    while j < n && chars[j].is_whitespace() {
        j += 1;
    }
    if j < n && chars[j] == '"' {
        // Quoted label.
        j += 1;
        let mut lbl = String::new();
        while j < n && chars[j] != '"' {
            lbl.push(chars[j]);
            j += 1;
        }
        if j < n {
            j += 1; // closing quote
        }
        // Now consume the trailing operator.
        while j < n && chars[j].is_whitespace() {
            j += 1;
        }
        let mut consumed = false;
        while j < n && (chars[j] == '-' || chars[j] == '=' || chars[j] == '.') {
            j += 1;
            consumed = true;
        }
        if j < n && chars[j] == '>' {
            arrow = true;
            j += 1;
            consumed = true;
        }
        if consumed {
            label = lbl;
            i = j;
        }
    }
    (Token::Edge { label, arrow }, i)
}

/// Parse a block declaration or a `space`/`space:N` token starting at `i`.
fn parse_block_or_space(chars: &[char], start: usize) -> (Token, usize) {
    let n = chars.len();
    let mut i = start;
    // Read the id: up to a shape opener, span colon, whitespace, or edge char.
    let mut id = String::new();
    while i < n {
        let c = chars[i];
        if c.is_whitespace()
            || c == '['
            || c == '('
            || c == '{'
            || c == ':'
            || c == '-'
            || c == '='
        {
            break;
        }
        id.push(c);
        i += 1;
    }

    // `space` / `space:N` is special.
    if id == "space" {
        let mut count = 1usize;
        if i < n && chars[i] == ':' {
            i += 1;
            let mut num = String::new();
            while i < n && chars[i].is_ascii_digit() {
                num.push(chars[i]);
                i += 1;
            }
            if let Ok(v) = num.parse::<usize>() {
                count = v.max(1);
            }
        }
        return (Token::Space(count), i);
    }

    // Optional shape + label.
    let mut shape = Shape::Rect;
    let mut label = id.clone();
    let mut explicit = false;
    if i < n {
        let (close, sh) = match chars[i] {
            '[' => (']', Shape::Rect),
            '(' => (')', Shape::Round),
            '{' => ('}', Shape::Diamond),
            _ => ('\0', Shape::Rect),
        };
        if close != '\0' {
            shape = sh;
            explicit = true;
            i += 1; // opener
            // Optional quote.
            let mut lbl = String::new();
            // Skip a leading quote if present and read until the matching close.
            if i < n && chars[i] == '"' {
                i += 1;
                while i < n && chars[i] != '"' {
                    lbl.push(chars[i]);
                    i += 1;
                }
                if i < n {
                    i += 1; // closing quote
                }
                // Skip to the closing bracket.
                while i < n && chars[i] != close {
                    i += 1;
                }
                if i < n {
                    i += 1;
                }
            } else {
                // Unquoted label up to the close bracket.
                while i < n && chars[i] != close {
                    lbl.push(chars[i]);
                    i += 1;
                }
                if i < n {
                    i += 1;
                }
            }
            label = lbl.trim().to_string();
            if label.is_empty() {
                label = id.clone();
            }
        }
    }

    // Optional `:N` span — only when the `:` is immediately followed by a digit
    // (so a `:::class` shorthand is not mistaken for a span).
    let mut span = 1usize;
    if i < n && chars[i] == ':' && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
        i += 1;
        let mut num = String::new();
        while i < n && chars[i].is_ascii_digit() {
            num.push(chars[i]);
            i += 1;
        }
        if let Ok(v) = num.parse::<usize>() {
            span = v.max(1);
            explicit = true;
        }
    }

    // Optional `:::class` shorthand: a run of `:` (>=2) followed by a class name.
    let mut class = None;
    if i + 1 < n && chars[i] == ':' && chars[i + 1] == ':' {
        while i < n && chars[i] == ':' {
            i += 1;
        }
        let mut name = String::new();
        while i < n
            && !chars[i].is_whitespace()
            && chars[i] != '-'
            && chars[i] != '='
            && chars[i] != ':'
        {
            name.push(chars[i]);
            i += 1;
        }
        if !name.is_empty() {
            class = Some(name);
        }
    }

    (
        Token::Block(Decl {
            id,
            label,
            shape,
            span,
            explicit,
            class,
        }),
        i,
    )
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// A block placed in the grid with pixel geometry.
#[derive(Clone, Debug)]
struct Placed {
    block: Block,
    /// Top-left pixel position and pixel size.
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// Whether this is a synthetic `space` cell (drawn as nothing).
    is_space: bool,
}

/// The laid-out diagram: placed blocks + the overall pixel size.
struct Layout {
    placed: Vec<Placed>,
    width: f32,
    height: f32,
}

/// Place the blocks into a row-major grid and compute pixel geometry.
fn layout_block(diagram: &BlockDiagram, opts: &MermaidOptions) -> Layout {
    let cols = diagram
        .columns
        .unwrap_or_else(|| diagram.blocks.len().max(1));
    let cols = cols.max(1);

    // Uniform cell size from the widest single-column block label.
    let mut cell_w = opts.font_size_px * 4.0;
    let mut cell_h = opts.font_size_px * 2.0;
    for b in &diagram.blocks {
        let (tw, th) = text_size(&b.label, opts.font_size_px);
        let bw = (tw + 2.0 * opts.node_padding_x) / b.span as f32;
        let bh = th + 2.0 * opts.node_padding_y;
        cell_w = cell_w.max(bw);
        cell_h = cell_h.max(bh);
    }

    let gap_x = opts.node_sep;
    let gap_y = opts.rank_sep;
    let margin = opts.font_size_px;

    // Assign grid cells row-major, wrapping when a span won't fit the row.
    let mut placed: Vec<Placed> = Vec::new();
    let mut col = 0usize;
    let mut row = 0usize;
    for b in &diagram.blocks {
        let span = b.span.min(cols).max(1);
        if col + span > cols {
            // Wrap to the next row.
            col = 0;
            row += 1;
        }
        let x = margin + col as f32 * (cell_w + gap_x);
        let y = margin + row as f32 * (cell_h + gap_y);
        let w = span as f32 * cell_w + (span as f32 - 1.0) * gap_x;
        let is_space = b.label.is_empty() && b.id.starts_with("__space_");
        placed.push(Placed {
            block: b.clone(),
            x,
            y,
            w,
            h: cell_h,
            is_space,
        });
        col += span;
        if col >= cols {
            col = 0;
            row += 1;
        }
    }

    let rows = if placed.is_empty() {
        0
    } else {
        // Highest row index used + 1.
        let last_row = placed
            .iter()
            .map(|p| ((p.y - margin) / (cell_h + gap_y)).round() as usize)
            .max()
            .unwrap_or(0);
        last_row + 1
    };

    let width = margin * 2.0 + cols as f32 * cell_w + (cols as f32 - 1.0).max(0.0) * gap_x;
    let height = margin * 2.0 + rows as f32 * cell_h + (rows as f32 - 1.0).max(0.0) * gap_y;
    Layout {
        placed,
        width: width.max(margin * 2.0),
        height: height.max(margin * 2.0),
    }
}

// ---------------------------------------------------------------------------
// Draw
// ---------------------------------------------------------------------------

/// Center point of a placed block.
fn center(p: &Placed) -> (f32, f32) {
    (p.x + p.w / 2.0, p.y + p.h / 2.0)
}

/// Clip a point on the segment from `from` toward `to` to the border of the
/// rectangle of placed block `p` (so edges meet the box edge, not the center).
fn clip_to_box(p: &Placed, from: (f32, f32), to: (f32, f32)) -> (f32, f32) {
    let (cx, cy) = from;
    let dx = to.0 - cx;
    let dy = to.1 - cy;
    if dx.abs() < 1e-6 && dy.abs() < 1e-6 {
        return from;
    }
    let hw = p.w / 2.0;
    let hh = p.h / 2.0;
    // Parametric t where the ray exits the box.
    let tx = if dx.abs() > 1e-6 {
        hw / dx.abs()
    } else {
        f32::INFINITY
    };
    let ty = if dy.abs() > 1e-6 {
        hh / dy.abs()
    } else {
        f32::INFINITY
    };
    let t = tx.min(ty);
    (cx + dx * t, cy + dy * t)
}

/// Render the laid-out diagram to an SVG document string.
fn draw_svg(layout: &Layout, diagram: &BlockDiagram, opts: &MermaidOptions) -> String {
    let w = layout.width;
    let h = layout.height;
    let mut s = String::new();
    let _ = write!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.2}\" height=\"{h:.2}\" viewBox=\"0 0 {w:.2} {h:.2}\">"
    );

    // Arrowhead marker.
    let edge_col = rgb(opts.edge_stroke);
    let _ = write!(
        s,
        "<defs><marker id=\"block-arrow\" markerWidth=\"10\" markerHeight=\"10\" refX=\"8\" refY=\"3\" orient=\"auto\" markerUnits=\"userSpaceOnUse\"><path d=\"M0,0 L8,3 L0,6 Z\" fill=\"{edge_col}\"/></marker></defs>"
    );

    let text_col = rgb(opts.text_color);
    let fs = opts.font_size_px;

    // --- Blocks ---
    for p in &layout.placed {
        if p.is_space {
            continue;
        }
        // Per-block style overrides fall back to the theme/options defaults
        // (same pattern as the flowchart node renderer).
        let st = &p.block.style;
        let fillc = st.fill.unwrap_or(opts.node_fill);
        let strokec = st.stroke.unwrap_or(opts.node_stroke);
        let fill = rgb(fillc);
        let fill_op = opacity_attr("fill-opacity", fillc);
        let stroke = rgb(strokec);
        let stroke_op = opacity_attr("stroke-opacity", strokec);
        let sw = st.stroke_width.unwrap_or(1.0);
        let dash = if st.dashed { " stroke-dasharray=\"4 3\"" } else { "" };
        let op = element_opacity_attr(st.opacity);
        let box_attrs = format!(
            "fill=\"{fill}\"{fill_op} stroke=\"{stroke}\"{stroke_op} stroke-width=\"{sw}\"{dash}{op}"
        );
        match p.block.shape {
            Shape::Rect => {
                let _ = write!(
                    s,
                    "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" {box_attrs}/>",
                    p.x, p.y, p.w, p.h
                );
            }
            Shape::Round => {
                let r = (p.h / 2.0).min(p.w / 2.0).min(fs);
                let _ = write!(
                    s,
                    "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" rx=\"{r:.2}\" ry=\"{r:.2}\" {box_attrs}/>",
                    p.x, p.y, p.w, p.h
                );
            }
            Shape::Diamond => {
                let (cx, cy) = center(p);
                let _ = write!(
                    s,
                    "<polygon points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" {box_attrs}/>",
                    cx, p.y, p.x + p.w, cy, cx, p.y + p.h, p.x, cy
                );
            }
        }
        // Centered label with per-block text overrides.
        if !p.block.label.is_empty() {
            let (cx, cy) = center(p);
            let label_col = match st.text_color {
                Some(c) => rgb(c),
                None => text_col.clone(),
            };
            let label_fs = st.font_size.unwrap_or(fs);
            let extra = text_style_attrs(st);
            let _ = write!(
                s,
                "<text x=\"{cx:.2}\" y=\"{cy:.2}\" font-family=\"{ff}\" font-size=\"{label_fs:.2}\" fill=\"{label_col}\" text-anchor=\"middle\" dominant-baseline=\"central\"{extra}>{label}</text>",
                ff = escape(&opts.font_family),
                label = escape(&p.block.label),
            );
        }
    }

    // --- Edges ---
    for e in &diagram.edges {
        let from = layout.placed.iter().find(|p| p.block.id == e.start);
        let to = layout.placed.iter().find(|p| p.block.id == e.end);
        let (Some(a), Some(b)) = (from, to) else {
            continue;
        };
        let ca = center(a);
        let cb = center(b);
        let pa = clip_to_box(a, ca, cb);
        let pb = clip_to_box(b, cb, ca);
        let marker = if e.arrow {
            " marker-end=\"url(#block-arrow)\""
        } else {
            ""
        };
        let _ = write!(
            s,
            "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" stroke=\"{edge_col}\" stroke-width=\"1.5\"{marker}/>",
            pa.0, pa.1, pb.0, pb.1
        );
        if !e.label.is_empty() {
            let mx = (pa.0 + pb.0) / 2.0;
            let my = (pa.1 + pb.1) / 2.0;
            // Small background so the label is readable over the line (only when
            // the edge-label-bg is opaque; a transparent canvas paints no box).
            let (lw, lh) = text_size(&e.label, fs);
            crate::svgutil::label_bg_rect(
                &mut s,
                mx - lw / 2.0 - 2.0,
                my - lh / 2.0,
                lw + 4.0,
                lh,
                0.0,
                opts.edge_label_bg,
            );
            let _ = write!(
                s,
                "<text x=\"{mx:.2}\" y=\"{my:.2}\" font-family=\"{ff}\" font-size=\"{fs:.2}\" fill=\"{text_col}\" text-anchor=\"middle\" dominant-baseline=\"central\">{label}</text>",
                ff = escape(&opts.font_family),
                label = escape(&e.label),
            );
        }
    }

    s.push_str("</svg>");
    s
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Render a mermaid `block` diagram to SVG.
pub fn render_block(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diagram = parse_block(src).map_err(MermaidError::Parse)?;
    // Only non-space blocks count toward "emptiness".
    let has_real = diagram
        .blocks
        .iter()
        .any(|b| !b.id.starts_with("__space_"));
    if !has_real {
        return Err(MermaidError::Empty);
    }
    let layout = layout_block(&diagram, opts);
    let svg = draw_svg(&layout, &diagram, opts);
    Ok(MermaidRender {
        svg,
        width_px: layout.width,
        height_px: layout.height,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    #[test]
    fn parse_columns_and_bare_blocks() {
        let d = parse_block("block-beta\n  columns 3\n  a b c\n").unwrap();
        assert_eq!(d.columns, Some(3));
        assert_eq!(d.blocks.len(), 3);
        assert_eq!(d.blocks[0].id, "a");
        assert_eq!(d.blocks[1].id, "b");
        assert_eq!(d.blocks[2].id, "c");
        // Bare blocks default to a rect, span 1, label == id.
        assert_eq!(d.blocks[0].shape, Shape::Rect);
        assert_eq!(d.blocks[0].span, 1);
        assert_eq!(d.blocks[0].label, "a");
    }

    #[test]
    fn parse_space_and_span() {
        let d =
            parse_block("block-beta\n  columns 3\n  a space b\n  wide[\"W\"]:2\n").unwrap();
        // a, space cell, b, wide → 4 entries (one synthetic space).
        let spaces: Vec<_> = d
            .blocks
            .iter()
            .filter(|b| b.id.starts_with("__space_"))
            .collect();
        assert_eq!(spaces.len(), 1, "one space cell");
        let wide = d.blocks.iter().find(|b| b.id == "wide").unwrap();
        assert_eq!(wide.span, 2, "span :2");
        assert_eq!(wide.label, "W");
    }

    #[test]
    fn parse_space_n() {
        let d = parse_block("block-beta\n  a space:3 b\n").unwrap();
        let spaces = d.blocks.iter().filter(|b| b.id.starts_with("__space_")).count();
        assert_eq!(spaces, 3, "space:3 → 3 cells");
    }

    #[test]
    fn parse_shapes() {
        let d = parse_block(
            "block-beta\n  a[\"Rect\"]\n  b(\"Round\")\n  c{\"Diamond\"}\n",
        )
        .unwrap();
        assert_eq!(d.blocks[0].shape, Shape::Rect);
        assert_eq!(d.blocks[0].label, "Rect");
        assert_eq!(d.blocks[1].shape, Shape::Round);
        assert_eq!(d.blocks[1].label, "Round");
        assert_eq!(d.blocks[2].shape, Shape::Diamond);
        assert_eq!(d.blocks[2].label, "Diamond");
    }

    #[test]
    fn parse_edge_simple() {
        let d = parse_block("block-beta\n  a b\n  a --> b\n").unwrap();
        assert_eq!(d.edges.len(), 1);
        assert_eq!(d.edges[0].start, "a");
        assert_eq!(d.edges[0].end, "b");
        assert!(d.edges[0].arrow);
        assert_eq!(d.edges[0].label, "");
    }

    #[test]
    fn parse_edge_with_label() {
        let d = parse_block("block-beta\n  a b\n  a -- \"hi\" --> b\n").unwrap();
        assert_eq!(d.edges.len(), 1);
        assert_eq!(d.edges[0].label, "hi");
        assert!(d.edges[0].arrow);
    }

    #[test]
    fn parse_undirected_edge() {
        let d = parse_block("block-beta\n  a b\n  a --- b\n").unwrap();
        assert_eq!(d.edges.len(), 1);
        assert!(!d.edges[0].arrow);
    }

    #[test]
    fn bad_header_errs() {
        let e = parse_block("flowchart TD\n a b\n").unwrap_err();
        assert!(e.contains("block"), "msg mentions block: {e}");
    }

    #[test]
    fn empty_errs() {
        // Header present but only space cells → Empty.
        assert_eq!(render_block("block-beta\n  space\n", &opts()), Err(MermaidError::Empty));
        // No header at all → Parse error.
        assert!(matches!(render_block("", &opts()), Err(MermaidError::Parse(_))));
    }

    #[test]
    fn header_plain_block_keyword() {
        let d = parse_block("block\n  a b\n").unwrap();
        assert_eq!(d.blocks.len(), 2);
    }

    #[test]
    fn grid_placement_row_major() {
        let d = parse_block("block-beta\n  columns 3\n  a b c d e f\n").unwrap();
        let layout = layout_block(&d, &opts());
        // First three on row 0 (same y), fourth wraps to row 1.
        let y0 = layout.placed[0].y;
        assert!((layout.placed[1].y - y0).abs() < 0.01);
        assert!((layout.placed[2].y - y0).abs() < 0.01);
        assert!(layout.placed[3].y > y0 + 1.0, "d wraps to a new row");
        // x increases across the row.
        assert!(layout.placed[1].x > layout.placed[0].x);
    }

    #[test]
    fn span_block_is_wider() {
        let d = parse_block("block-beta\n  columns 3\n  a wide[\"W\"]:2\n").unwrap();
        let layout = layout_block(&d, &opts());
        let a = layout.placed.iter().find(|p| p.block.id == "a").unwrap();
        let wide = layout.placed.iter().find(|p| p.block.id == "wide").unwrap();
        assert!(wide.w > a.w * 1.5, "span:2 block is wider: {} vs {}", wide.w, a.w);
    }

    #[test]
    fn render_well_formed() {
        let r = render_block(
            "block-beta\n  columns 3\n  a b c\n  a --> b\n",
            &opts(),
        )
        .unwrap();
        assert!(r.svg.starts_with("<svg xmlns="));
        assert!(r.svg.ends_with("</svg>"));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
        // One <rect> per (rect/round) block; three bare blocks → 3 rects.
        assert_eq!(r.svg.matches("<rect").count() >= 3, true);
        // Edge line + arrow marker.
        assert!(r.svg.contains("<line "));
        assert!(r.svg.contains("marker-end=\"url(#block-arrow)\""));
        assert!(r.svg.contains("<marker"));
        // Labels present.
        assert!(r.svg.contains(">a</text>"));
    }

    #[test]
    fn render_shapes_present() {
        let r = render_block(
            "block-beta\n  a[\"R\"] b(\"Ro\") c{\"D\"}\n",
            &opts(),
        )
        .unwrap();
        // Diamond is a polygon; round/rect are rects.
        assert!(r.svg.contains("<polygon"), "diamond polygon");
        assert!(r.svg.contains("rx="), "rounded rect has rx");
    }

    #[test]
    fn render_xml_escape() {
        let r = render_block("block-beta\n  a[\"x & <y>\"]\n", &opts()).unwrap();
        assert!(r.svg.contains("x &amp; &lt;y&gt;"));
        assert!(!r.svg.contains("x & <y>"));
    }

    #[test]
    fn render_edge_label_present() {
        let r = render_block(
            "block-beta\n  a b\n  a -- \"go\" --> b\n",
            &opts(),
        )
        .unwrap();
        assert!(r.svg.contains(">go</text>"));
    }

    #[test]
    fn classdef_and_class_apply_to_block() {
        let d = parse_block(
            "block-beta\n  a b\n  classDef hot fill:#ffcdd2,stroke:#c62828,stroke-width:3px\n  class a hot\n",
        )
        .unwrap();
        let a = d.blocks.iter().find(|b| b.id == "a").unwrap();
        assert_eq!(a.style.fill, Some([0xff, 0xcd, 0xd2, 255]));
        assert_eq!(a.style.stroke, Some([0xc6, 0x28, 0x28, 255]));
        assert_eq!(a.style.stroke_width, Some(3.0));
        // b is untouched.
        let b = d.blocks.iter().find(|b| b.id == "b").unwrap();
        assert_eq!(b.style, ElemStyle::default());
    }

    #[test]
    fn inline_style_applies_to_block() {
        let d = parse_block("block-beta\n  a b\n  style b fill:#00ff00\n").unwrap();
        let b = d.blocks.iter().find(|b| b.id == "b").unwrap();
        assert_eq!(b.style.fill, Some([0, 255, 0, 255]));
    }

    #[test]
    fn triple_colon_shorthand_on_block() {
        // `:::class` on a declaration; span/label still parse around it.
        let d = parse_block("block-beta\n  a[\"A\"]:::hot b\n  classDef hot fill:#0000ff\n").unwrap();
        let a = d.blocks.iter().find(|b| b.id == "a").unwrap();
        assert_eq!(a.label, "A");
        assert_eq!(a.style.fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn span_still_parses_alongside_class() {
        // A `:N` span and a `:::class` on the same declaration both take effect.
        let d = parse_block("block-beta\n  columns 3\n  w[\"W\"]:2:::hot\n  classDef hot fill:#f00\n").unwrap();
        let w = d.blocks.iter().find(|b| b.id == "w").unwrap();
        assert_eq!(w.span, 2);
        assert_eq!(w.style.fill, Some([255, 0, 0, 255]));
    }

    #[test]
    fn styled_block_fill_appears_in_svg() {
        let r = render_block(
            "block-beta\n  a b\n  classDef hot fill:#ffcdd2,stroke:#c62828\n  class a hot\n",
            &opts(),
        )
        .unwrap();
        assert!(r.svg.contains("fill=\"rgb(255,205,210)\""), "block fill in svg: {}", r.svg);
        assert!(r.svg.contains("stroke=\"rgb(198,40,40)\""), "block stroke in svg: {}", r.svg);
    }

    #[test]
    fn deterministic() {
        let src = "block-beta\n  columns 2\n  a b\n  c[\"C\"]:2\n  a --> b\n";
        let r1 = render_block(src, &opts()).unwrap();
        let r2 = render_block(src, &opts()).unwrap();
        assert_eq!(r1.svg, r2.svg);
        assert_eq!(r1.width_px, r2.width_px);
    }
}
