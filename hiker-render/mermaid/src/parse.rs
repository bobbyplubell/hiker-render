//! Stage 1: parse mermaid flowchart source → [`FlowChart`]. No upstream deps.
//!
//! A pragmatic, hand-written (line-and-token, recursive-descent flavored)
//! parser for a well-defined SUBSET of the mermaid flowchart grammar. See
//! `references/mermaid`'s `packages/mermaid/src/diagrams/flowchart/parser/flow.jison`
//! for the full grammar. Pure std, dependency-free.
//!
//! ## Supported subset
//! - Header: `graph <dir>` / `flowchart <dir>` with `<dir>` ∈ `TB|TD|BT|LR|RL`.
//!   `TD`/`TB` → [`Direction::TopDown`]. Default [`Direction::TopDown`] if absent.
//! - Statement separators: newlines and `;`. Blank lines ignored. `%% ...`
//!   comments stripped to end of line.
//! - Node shapes: `A` / `A[..]` (Rect), `A(..)` (RoundRect), `A([..])` (Stadium),
//!   `A((..))` (Circle), `A{..}` (Diamond), `A{{..}}` (Hexagon). Labels may be
//!   `"quoted"`.
//! - Edges: `-->` `---` `<-->`, thick `==>`/`===`, dotted `-.->`/`-.-`, variable
//!   dash lengths, labels via `-->|text|` and `-- text -->` (and `--- text ---`),
//!   and chaining `A --> B --> C`.
//!
//! Intentionally skipped for v1 (see report): subgraphs, styling/classDef/style,
//! linkStyle, click/interaction, `&` multi-node refs, accessibility directives,
//! markdown/`@{...}` shape syntax, and the `o--o`/`x--x` endpoint markers.
//!
//! Policy: **lenient** — unrecognized lines are skipped rather than erroring, so
//! recovery is per-line. Node label/shape is **last-wins** when a node is
//! redefined. A node referenced before being shaped defaults to `Rect` with
//! `label == id`.

use crate::model::{
    Direction, EdgeKind, ElemStyle, FlowChart, FlowEdge, FlowNode, NodeShape, Subgraph,
};
use std::collections::HashMap;

/// Directive state collected during parsing, resolved onto nodes/edges at the
/// end of `parse_flowchart` (two-pass: classDefs may be defined after the
/// `class`/`:::` statements that reference them).
#[derive(Default)]
struct Directives {
    /// Named `classDef` styles.
    class_defs: HashMap<String, ElemStyle>,
    /// `(node id, class name)` assignments from `class A,B name` and `A:::name`.
    class_assignments: Vec<(String, String)>,
    /// Inline `style <id> ...` overrides applied directly to a node.
    node_inline: Vec<(String, ElemStyle)>,
    /// `linkStyle <n> ...` overrides, keyed by 0-based edge index.
    edge_inline: Vec<(usize, ElemStyle)>,
    /// `linkStyle default ...` overrides applied to every edge.
    edge_default: Vec<ElemStyle>,
    /// `click <id> ...` interaction directives, resolved onto nodes at the end.
    /// `(node id, link, callback, tooltip)`; the node is auto-created if absent.
    clicks: Vec<ClickDirective>,
}

/// One parsed `click` directive (interaction data for a node).
struct ClickDirective {
    id: String,
    link: Option<String>,
    callback: Option<String>,
    tooltip: Option<String>,
}

/// Parse mermaid flowchart source (e.g. `graph TD; A[Start] --> B{Decision}`)
/// into a [`FlowChart`]. Returns `Err(message)` on a syntax error.
pub fn parse_flowchart(src: &str) -> Result<FlowChart, String> {
    let mut chart = FlowChart {
        direction: Direction::TopDown,
        nodes: Vec::new(),
        edges: Vec::new(),
        subgraphs: Vec::new(),
    };
    // Tracks insertion index of each node id so we can update (last-wins) the
    // existing entry rather than appending a duplicate.
    let mut node_index: Vec<(String, usize)> = Vec::new();
    let mut directives = Directives::default();
    // Stack of currently-open subgraph indices (into `chart.subgraphs`), innermost
    // last. A node first seen while this is non-empty joins the innermost subgraph.
    let mut subgraph_stack: Vec<usize> = Vec::new();

    let mut header_seen = false;

    for raw_line in src.lines() {
        let line = strip_comment(raw_line);
        // A physical line may carry multiple `;`-separated statements.
        for stmt in line.split(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }

            // Header line: `graph TD`, `flowchart LR`, etc. Only the first
            // keyword line counts as a header / direction.
            if !header_seen {
                if let Some(dir) = parse_header(stmt) {
                    chart.direction = dir;
                    header_seen = true;
                    continue;
                }
                // First real statement without a header keyword: treat the
                // (absent) header as default TopDown and fall through to parse
                // it as a statement.
                header_seen = true;
            }

            // Subgraph block control: `subgraph …` opens a cluster (push), `end`
            // closes the innermost one (pop). These bracket the statements between
            // them; nodes first seen inside join the innermost open subgraph.
            if let Some((id, title)) = parse_subgraph_open(stmt) {
                let parent = subgraph_stack.last().copied();
                let idx = chart.subgraphs.len();
                chart.subgraphs.push(Subgraph {
                    id,
                    title,
                    node_ids: Vec::new(),
                    parent,
                });
                subgraph_stack.push(idx);
                continue;
            }
            if stmt == "end" {
                subgraph_stack.pop();
                continue;
            }
            // `direction LR` inside a subgraph is parsed-and-ignored (the whole
            // chart keeps its single top-level direction in this renderer).
            if stmt.split_whitespace().next() == Some("direction") {
                continue;
            }

            // Styling directives (`classDef`/`class`/`style`/`linkStyle`) are
            // collected here and resolved after all statements are parsed.
            if parse_directive(stmt, &mut directives) {
                continue;
            }

            parse_statement(
                stmt,
                &mut chart,
                &mut node_index,
                &mut directives,
                &subgraph_stack,
            );
        }
    }

    resolve_styles(&mut chart, &directives);

    Ok(chart)
}

/// Try to parse `stmt` as a styling directive, recording it into `dir`. Returns
/// `true` if it was a (recognized) directive keyword line and should not be
/// treated as a node/edge statement.
fn parse_directive(stmt: &str, dir: &mut Directives) -> bool {
    let mut words = stmt.split_whitespace();
    let kw = match words.next() {
        Some(k) => k,
        None => return false,
    };
    match kw {
        "classDef" => {
            // classDef <name> <prop:val,...>
            let rest = stmt[kw.len()..].trim_start();
            let mut parts = rest.splitn(2, char::is_whitespace);
            if let Some(name) = parts.next().filter(|n| !n.is_empty()) {
                let props = parts.next().unwrap_or("");
                let style = parse_style_props(props);
                dir.class_defs.insert(name.to_string(), style);
            }
            true
        }
        "class" => {
            // class <id1>,<id2>,... <className>
            let rest = stmt[kw.len()..].trim_start();
            // Split off the trailing class name (last whitespace-delimited token).
            if let Some(sp) = rest.rfind(char::is_whitespace) {
                let ids = rest[..sp].trim();
                let class_name = rest[sp..].trim();
                if !class_name.is_empty() {
                    for id in ids.split(',') {
                        let id = id.trim();
                        if !id.is_empty() {
                            dir.class_assignments
                                .push((id.to_string(), class_name.to_string()));
                        }
                    }
                }
            }
            true
        }
        "style" => {
            // style <id> <prop:val,...>
            let rest = stmt[kw.len()..].trim_start();
            let mut parts = rest.splitn(2, char::is_whitespace);
            if let Some(id) = parts.next().filter(|n| !n.is_empty()) {
                let props = parts.next().unwrap_or("");
                dir.node_inline
                    .push((id.to_string(), parse_style_props(props)));
            }
            true
        }
        "linkStyle" => {
            // linkStyle <n[,m,...]|default> <prop:val,...>
            let rest = stmt[kw.len()..].trim_start();
            let mut parts = rest.splitn(2, char::is_whitespace);
            if let Some(sel) = parts.next().filter(|n| !n.is_empty()) {
                let props = parts.next().unwrap_or("");
                let style = parse_style_props(props);
                if sel == "default" {
                    dir.edge_default.push(style);
                } else {
                    for tok in sel.split(',') {
                        if let Ok(n) = tok.trim().parse::<usize>() {
                            dir.edge_inline.push((n, style.clone()));
                        }
                    }
                }
            }
            true
        }
        "click" => {
            // click <id> ... — interaction directive. Always consumed (true) so
            // it isn't parsed as a node/edge statement; a malformed one is a no-op.
            let rest = stmt[kw.len()..].trim_start();
            if let Some(c) = parse_click(rest) {
                dir.clicks.push(c);
            }
            true
        }
        _ => false,
    }
}

/// Parse the body of a `click <id> ...` directive (everything after `click`).
/// Supported forms (quote-aware):
/// - `<id> "<url>" ["<tooltip>"]`            → link (+ tooltip)
/// - `<id> href "<url>" ["<tooltip>"]`       → link (+ tooltip)
/// - `<id> call <name>(<args>) ["<tooltip>"]` → callback = name (args dropped)
/// - `<id> callback` / `<id> <name>` (bareword) → callback = the word
///
/// A trailing `_blank`/`_self` target token after a url is tolerated and ignored.
/// Returns `None` if no id is present.
fn parse_click(rest: &str) -> Option<ClickDirective> {
    let toks = tokenize_click(rest);
    let mut it = toks.into_iter();
    // The id is the first token (a word; a quoted first token is malformed).
    let id = match it.next()? {
        ClickTok::Word(w) => w,
        ClickTok::Quoted(_) => return None,
    };
    let mut link = None;
    let mut callback = None;
    let mut tooltip = None;

    let rest_toks: Vec<ClickTok> = it.collect();
    let mut i = 0;
    while i < rest_toks.len() {
        match &rest_toks[i] {
            ClickTok::Word(w) if w == "href" => {
                // Next quoted token is the url.
                if let Some(ClickTok::Quoted(u)) = rest_toks.get(i + 1) {
                    link = Some(u.clone());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            ClickTok::Word(w) if w == "call" => {
                // Next token is `name(args)` (a bare word possibly with parens).
                if let Some(ClickTok::Word(callee)) = rest_toks.get(i + 1) {
                    let name = callee.split('(').next().unwrap_or(callee).trim();
                    if !name.is_empty() {
                        callback = Some(name.to_string());
                    }
                    i += 2;
                } else {
                    i += 1;
                }
            }
            ClickTok::Word(w) if w == "_blank" || w == "_self" => {
                // Link target — tolerated and ignored.
                i += 1;
            }
            ClickTok::Word(w) => {
                // Bareword callback (`click A callback` / `click A doThing`),
                // only when we haven't already found a link/callback.
                if link.is_none() && callback.is_none() {
                    let name = w.split('(').next().unwrap_or(w).trim();
                    if !name.is_empty() {
                        callback = Some(name.to_string());
                    }
                }
                i += 1;
            }
            ClickTok::Quoted(s) => {
                // First quoted string is the url (if no link yet), else tooltip.
                if link.is_none() && callback.is_none() {
                    link = Some(s.clone());
                } else if tooltip.is_none() {
                    tooltip = Some(s.clone());
                }
                i += 1;
            }
        }
    }

    Some(ClickDirective {
        id,
        link,
        callback,
        tooltip,
    })
}

/// A token from a `click` directive body: a bare word or a double-quoted string.
enum ClickTok {
    Word(String),
    Quoted(String),
}

/// Split a `click` directive body into quote-aware tokens. Double-quoted spans
/// become a single [`ClickTok::Quoted`] (quotes stripped); other whitespace-
/// delimited runs become [`ClickTok::Word`].
fn tokenize_click(s: &str) -> Vec<ClickTok> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip whitespace.
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i] == b'"' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let inner = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
            out.push(ClickTok::Quoted(inner.to_string()));
            if i < bytes.len() {
                i += 1; // consume closing quote
            }
        } else {
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'"' {
                i += 1;
            }
            let word = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
            if !word.is_empty() {
                out.push(ClickTok::Word(word.to_string()));
            }
        }
    }
    out
}

/// Resolve collected directives onto the chart's nodes and edges. Apply order
/// (mermaid): the `class`/`:::` classDef style first, then inline `style`/
/// `linkStyle` overrides on top (field-by-field).
fn resolve_styles(chart: &mut FlowChart, dir: &Directives) {
    // Nodes: classDef-via-class first.
    for (id, class_name) in &dir.class_assignments {
        if let Some(class_style) = dir.class_defs.get(class_name) {
            if let Some(n) = chart.nodes.iter_mut().find(|n| n.id == *id) {
                merge_style(&mut n.style, class_style);
            }
        }
    }
    // Nodes: inline `style` overrides on top.
    for (id, style) in &dir.node_inline {
        if let Some(n) = chart.nodes.iter_mut().find(|n| n.id == *id) {
            merge_style(&mut n.style, style);
        }
    }
    // Edges: `linkStyle default` first (broadest), then per-index.
    for style in &dir.edge_default {
        for e in chart.edges.iter_mut() {
            merge_style(&mut e.style, style);
        }
    }
    for (n, style) in &dir.edge_inline {
        if let Some(e) = chart.edges.get_mut(*n) {
            merge_style(&mut e.style, style);
        }
    }

    // Interaction: apply `click` directives. An unknown id is auto-created as a
    // Rect node with `label == id` so the region still hit-tests.
    for c in &dir.clicks {
        let node = match chart.nodes.iter_mut().find(|n| n.id == c.id) {
            Some(n) => n,
            None => {
                chart.nodes.push(FlowNode {
                    id: c.id.clone(),
                    label: c.id.clone(),
                    shape: NodeShape::Rect,
                    style: ElemStyle::default(),
                    link: None,
                    callback: None,
                    tooltip: None,
                });
                chart.nodes.last_mut().unwrap()
            }
        };
        if c.link.is_some() {
            node.link = c.link.clone();
        }
        if c.callback.is_some() {
            node.callback = c.callback.clone();
        }
        if c.tooltip.is_some() {
            node.tooltip = c.tooltip.clone();
        }
    }
}

/// Merge `src` into `dst` field-by-field: any `Some`/true field in `src`
/// overrides `dst` (so later/inline styles win over earlier/class styles).
fn merge_style(dst: &mut ElemStyle, src: &ElemStyle) {
    if src.fill.is_some() {
        dst.fill = src.fill;
    }
    if src.stroke.is_some() {
        dst.stroke = src.stroke;
    }
    if src.stroke_width.is_some() {
        dst.stroke_width = src.stroke_width;
    }
    if src.text_color.is_some() {
        dst.text_color = src.text_color;
    }
    if src.dashed {
        dst.dashed = true;
    }
}

/// Parse a comma-separated `prop:val,prop:val,...` list into an [`ElemStyle`].
/// Unknown props or unparseable colors are skipped leniently.
fn parse_style_props(props: &str) -> ElemStyle {
    let mut style = ElemStyle::default();
    for part in props.split(',') {
        let part = part.trim();
        let (key, val) = match part.split_once(':') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => continue,
        };
        match key {
            "fill" => {
                if let Some(c) = parse_color(val) {
                    style.fill = Some(c);
                }
            }
            "stroke" => {
                if let Some(c) = parse_color(val) {
                    style.stroke = Some(c);
                }
            }
            "color" => {
                if let Some(c) = parse_color(val) {
                    style.text_color = Some(c);
                }
            }
            "stroke-width" => {
                if let Some(w) = parse_width(val) {
                    style.stroke_width = Some(w);
                }
            }
            "stroke-dasharray" => {
                if !val.is_empty() {
                    style.dashed = true;
                }
            }
            _ => {}
        }
    }
    style
}

/// Parse a stroke width like `2px` / `4` / `1.5` into an f32 (px).
fn parse_width(val: &str) -> Option<f32> {
    let v = val.trim().trim_end_matches("px").trim();
    v.parse::<f32>().ok()
}

/// Parse a CSS-ish color into straight RGBA: `#rgb`, `#rrggbb`, `#rrggbbaa`,
/// `rgb(r,g,b)`, `rgba(r,g,b,a)`, or a small set of named colors. Returns `None`
/// on anything unrecognized so the caller can skip the prop.
fn parse_color(s: &str) -> Option<[u8; 4]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex_color(hex);
    }
    if let Some(inner) = s.strip_prefix("rgba(").and_then(|x| x.strip_suffix(')')) {
        return parse_rgb_func(inner, true);
    }
    if let Some(inner) = s.strip_prefix("rgb(").and_then(|x| x.strip_suffix(')')) {
        return parse_rgb_func(inner, false);
    }
    parse_named_color(s)
}

/// Parse the body of a `#...` hex color (3/6/8 hex digits).
fn parse_hex_color(hex: &str) -> Option<[u8; 4]> {
    let h = hex.trim();
    match h.len() {
        3 => {
            let r = u8::from_str_radix(&h[0..1], 16).ok()?;
            let g = u8::from_str_radix(&h[1..2], 16).ok()?;
            let b = u8::from_str_radix(&h[2..3], 16).ok()?;
            // Expand each nibble (e.g. f -> ff).
            Some([r * 17, g * 17, b * 17, 255])
        }
        6 => {
            let r = u8::from_str_radix(&h[0..2], 16).ok()?;
            let g = u8::from_str_radix(&h[2..4], 16).ok()?;
            let b = u8::from_str_radix(&h[4..6], 16).ok()?;
            Some([r, g, b, 255])
        }
        8 => {
            let r = u8::from_str_radix(&h[0..2], 16).ok()?;
            let g = u8::from_str_radix(&h[2..4], 16).ok()?;
            let b = u8::from_str_radix(&h[4..6], 16).ok()?;
            let a = u8::from_str_radix(&h[6..8], 16).ok()?;
            Some([r, g, b, a])
        }
        _ => None,
    }
}

/// Parse the inside of `rgb(...)`/`rgba(...)`. When `with_alpha`, the 4th
/// component is a 0..1 (or 0..255) alpha; we accept a 0..1 float or a 0..255 int.
fn parse_rgb_func(inner: &str, with_alpha: bool) -> Option<[u8; 4]> {
    let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
    let need = if with_alpha { 4 } else { 3 };
    if parts.len() != need {
        return None;
    }
    let r = parts[0].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
    let g = parts[1].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
    let b = parts[2].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
    let a = if with_alpha {
        let av = parts[3].parse::<f32>().ok()?;
        if av <= 1.0 {
            (av * 255.0).round().clamp(0.0, 255.0) as u8
        } else {
            av.round().clamp(0.0, 255.0) as u8
        }
    } else {
        255
    };
    Some([r, g, b, a])
}

/// Map a small set of CSS named colors to RGBA.
fn parse_named_color(name: &str) -> Option<[u8; 4]> {
    let c = match name.to_ascii_lowercase().as_str() {
        "red" => [255, 0, 0],
        "green" => [0, 128, 0],
        "blue" => [0, 0, 255],
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "yellow" => [255, 255, 0],
        "orange" => [255, 165, 0],
        "purple" => [128, 0, 128],
        "gray" | "grey" => [128, 128, 128],
        "lightblue" => [173, 216, 230],
        "lightgreen" => [144, 238, 144],
        "pink" => [255, 192, 203],
        "cyan" => [0, 255, 255],
        "magenta" => [255, 0, 255],
        "brown" => [165, 42, 42],
        "lightgray" | "lightgrey" => [211, 211, 211],
        _ => return None,
    };
    Some([c[0], c[1], c[2], 255])
}

/// Strip a `%% ...` comment from a single source line.
fn strip_comment(line: &str) -> &str {
    match line.find("%%") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// If `stmt` is a header keyword line (`graph`/`flowchart [dir]`), return the
/// direction (defaulting to `TopDown` when no/unknown dir token follows).
fn parse_header(stmt: &str) -> Option<Direction> {
    let mut words = stmt.split_whitespace();
    let kw = words.next()?;
    if kw != "graph" && kw != "flowchart" {
        return None;
    }
    let dir = match words.next() {
        Some(tok) => parse_direction(tok).unwrap_or(Direction::TopDown),
        None => Direction::TopDown,
    };
    Some(dir)
}

/// If `stmt` opens a subgraph block, return its `(id, title)`. Forms:
/// - `subgraph <id>[<Title>]` — explicit id + bracketed title.
/// - `subgraph <id>` — id only; title = id.
/// - `subgraph "Title"` / `subgraph <Title>` — no brackets; the token(s) form
///   both the title and (for a single bare word) the id.
fn parse_subgraph_open(stmt: &str) -> Option<(String, String)> {
    let rest = stmt.strip_prefix("subgraph")?;
    // Must be a word boundary: `subgraph` followed by whitespace or end-of-line.
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim();
    if rest.is_empty() {
        // Anonymous subgraph: id/title derived from declaration order by caller's
        // index; use an empty title so no label is drawn.
        return Some((String::new(), String::new()));
    }

    // `<id>[<Title>]` — id is the leading id-chars before a `[`.
    if let Some(br) = rest.find('[') {
        let id = rest[..br].trim();
        if rest.ends_with(']') {
            let title = clean_label(rest[br + 1..rest.len() - 1].trim());
            return Some((id.to_string(), title));
        }
    }

    // `"Title"` — quoted title; id derived as the title text (no separate id).
    if rest.starts_with('"') {
        let title = clean_label(rest);
        return Some((title.clone(), title));
    }

    // Bare `<id>` (single token) → title = id. Multi-word bare title → id = whole
    // text, title = whole text.
    Some((rest.to_string(), rest.to_string()))
}

/// Map a direction token to a [`Direction`].
fn parse_direction(tok: &str) -> Option<Direction> {
    match tok {
        "TB" | "TD" => Some(Direction::TopDown),
        "BT" => Some(Direction::BottomUp),
        "LR" => Some(Direction::LeftRight),
        "RL" => Some(Direction::RightLeft),
        _ => None,
    }
}

/// Parse one statement (a single `;`/newline-delimited unit). Handles both
/// standalone node declarations and edge chains. Lenient: bails silently on
/// anything it can't make sense of.
fn parse_statement(
    stmt: &str,
    chart: &mut FlowChart,
    node_index: &mut Vec<(String, usize)>,
    dir: &mut Directives,
    subgraph_stack: &[usize],
) {
    let bytes = stmt.as_bytes();
    let mut pos = 0usize;

    // First node ref is required for any statement we care about.
    let first = match parse_node_ref(bytes, &mut pos, dir) {
        Some(n) => n,
        None => return,
    };
    upsert_node(chart, node_index, first.clone(), subgraph_stack);

    let mut prev_id = first.id;

    // Then zero-or-more (edge, node) pairs, supporting chaining A --> B --> C.
    loop {
        skip_ws(bytes, &mut pos);
        if pos >= bytes.len() {
            break;
        }
        let edge = match parse_edge_op(bytes, &mut pos) {
            Some(e) => e,
            None => break, // not an edge here; stop (lenient)
        };
        skip_ws(bytes, &mut pos);
        let target = match parse_node_ref(bytes, &mut pos, dir) {
            Some(n) => n,
            None => break, // edge with no target; drop it (lenient)
        };
        let target_id = target.id.clone();
        upsert_node(chart, node_index, target, subgraph_stack);

        chart.edges.push(FlowEdge {
            from: prev_id.clone(),
            to: target_id.clone(),
            label: edge.label,
            kind: edge.kind,
            arrow_start: edge.arrow_start,
            arrow_end: edge.arrow_end, style: crate::model::ElemStyle::default(),
        });
        prev_id = target_id;
    }
}

/// A node ref parsed out of source: id plus optional explicit label/shape.
struct ParsedNode {
    id: String,
    /// `Some` when the ref carried a bracketed/shaped label; `None` for a bare id.
    label: Option<String>,
    shape: NodeShape,
}

impl Clone for ParsedNode {
    fn clone(&self) -> Self {
        ParsedNode {
            id: self.id.clone(),
            label: self.label.clone(),
            shape: self.shape,
        }
    }
}

/// Insert or update (last-wins for shape/label) a node into the chart, keeping
/// first-seen ordering.
fn upsert_node(
    chart: &mut FlowChart,
    node_index: &mut Vec<(String, usize)>,
    parsed: ParsedNode,
    subgraph_stack: &[usize],
) {
    let existing = node_index.iter().find(|(id, _)| *id == parsed.id).map(|(_, i)| *i);
    let is_new = existing.is_none();
    match existing {
        Some(i) => {
            // Only override shape/label when this ref actually carried one.
            if let Some(label) = parsed.label {
                chart.nodes[i].label = label;
                chart.nodes[i].shape = parsed.shape;
            }
        }
        None => {
            let idx = chart.nodes.len();
            let label = parsed.label.unwrap_or_else(|| parsed.id.clone());
            chart.nodes.push(FlowNode {
                id: parsed.id.clone(),
                label,
                shape: parsed.shape,
                style: crate::model::ElemStyle::default(),
                link: None,
                callback: None,
                tooltip: None,
            });
            node_index.push((parsed.id.clone(), idx));
        }
    }

    // A node first seen inside a subgraph block belongs to the innermost open
    // subgraph. Membership is keyed on first-seen so a node declared in one
    // subgraph but referenced from another stays in its original subgraph.
    if is_new {
        if let Some(&sg) = subgraph_stack.last() {
            chart.subgraphs[sg].node_ids.push(parsed.id);
        }
    }
}

/// Is `c` a valid node-id character? Lenient: letters, digits, underscore,
/// plus `-`/`.` (mermaid allows these in ids), but we keep it conservative so
/// edge operators (which start with `-`/`=`/`<`) aren't swallowed.
fn is_id_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Parse a node ref at `pos`: an identifier optionally followed by a shape
/// bracket group. Advances `pos` past it. Returns `None` if no id is present.
fn parse_node_ref(bytes: &[u8], pos: &mut usize, dir: &mut Directives) -> Option<ParsedNode> {
    skip_ws(bytes, pos);
    let start = *pos;
    while *pos < bytes.len() && is_id_char(bytes[*pos]) {
        *pos += 1;
    }
    if *pos == start {
        return None;
    }
    let id = std::str::from_utf8(&bytes[start..*pos]).ok()?.to_string();

    // Optional shape group immediately after the id (no space allowed between
    // id and the opening bracket, matching mermaid).
    let (label, shape) = parse_shape(bytes, pos);

    // Optional `:::className` shorthand (after the id and any shape group).
    // Record the class to apply once classDefs are known.
    if starts_with(bytes, *pos, b":::") {
        *pos += 3;
        let name_start = *pos;
        while *pos < bytes.len() && is_id_char(bytes[*pos]) {
            *pos += 1;
        }
        if *pos > name_start {
            if let Ok(name) = std::str::from_utf8(&bytes[name_start..*pos]) {
                dir.class_assignments.push((id.clone(), name.to_string()));
            }
        }
    }

    Some(ParsedNode { id, label, shape })
}

/// Parse an optional shape group `[..]`, `(..)`, `([..])`, `((..))`, `{..}`,
/// `{{..}}` starting at `pos`. Returns `(label, shape)` where `label` is `None`
/// if there was no group (the caller defaults shape to `Rect`).
fn parse_shape(bytes: &[u8], pos: &mut usize) -> (Option<String>, NodeShape) {
    if *pos >= bytes.len() {
        return (None, NodeShape::Rect);
    }
    match bytes[*pos] {
        b'[' => {
            // Rect: A[..]
            extract_group(bytes, pos, b"[", b"]").map_or((None, NodeShape::Rect), |t| {
                (Some(t), NodeShape::Rect)
            })
        }
        b'(' => {
            // Stadium A([..]), Circle A((..)), or RoundRect A(..)
            if peek2(bytes, *pos) == Some((b'(', b'(')) {
                extract_group(bytes, pos, b"((", b"))")
                    .map_or((None, NodeShape::Rect), |t| (Some(t), NodeShape::Circle))
            } else if peek2(bytes, *pos) == Some((b'(', b'[')) {
                extract_group(bytes, pos, b"([", b"])")
                    .map_or((None, NodeShape::Rect), |t| (Some(t), NodeShape::Stadium))
            } else {
                extract_group(bytes, pos, b"(", b")")
                    .map_or((None, NodeShape::Rect), |t| (Some(t), NodeShape::RoundRect))
            }
        }
        b'{' => {
            // Hexagon A{{..}} or Diamond A{..}
            if peek2(bytes, *pos) == Some((b'{', b'{')) {
                extract_group(bytes, pos, b"{{", b"}}")
                    .map_or((None, NodeShape::Rect), |t| (Some(t), NodeShape::Hexagon))
            } else {
                extract_group(bytes, pos, b"{", b"}")
                    .map_or((None, NodeShape::Rect), |t| (Some(t), NodeShape::Diamond))
            }
        }
        _ => (None, NodeShape::Rect),
    }
}

/// Peek the two bytes at `i` (for distinguishing `((` from `(` etc.).
fn peek2(bytes: &[u8], i: usize) -> Option<(u8, u8)> {
    if i + 1 < bytes.len() {
        Some((bytes[i], bytes[i + 1]))
    } else {
        None
    }
}

/// Extract a bracketed group: consume `open`, read until `close`, return the
/// trimmed/unquoted inner text. Advances `pos` past `close`. On a missing close
/// delimiter, leaves `pos` unchanged and returns `None`.
fn extract_group(bytes: &[u8], pos: &mut usize, open: &[u8], close: &[u8]) -> Option<String> {
    let saved = *pos;
    if !starts_with(bytes, *pos, open) {
        return None;
    }
    let inner_start = *pos + open.len();
    let mut i = inner_start;
    // Quote-aware: a `"…"` span suppresses close-delimiter matching, so a quoted
    // label may itself contain the close bracket — e.g. `["a [link](u)"]`.
    let mut in_quotes = false;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            in_quotes = !in_quotes;
            i += 1;
            continue;
        }
        if !in_quotes && starts_with(bytes, i, close) {
            let inner = std::str::from_utf8(&bytes[inner_start..i]).ok()?;
            *pos = i + close.len();
            return Some(clean_label(inner));
        }
        i += 1;
    }
    *pos = saved;
    None
}

/// Does `bytes[i..]` start with `needle`?
fn starts_with(bytes: &[u8], i: usize, needle: &[u8]) -> bool {
    i + needle.len() <= bytes.len() && &bytes[i..i + needle.len()] == needle
}

/// Trim whitespace and strip a single pair of surrounding double-quotes.
fn clean_label(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// A parsed edge operator.
struct ParsedEdge {
    label: Option<String>,
    kind: EdgeKind,
    arrow_start: bool,
    arrow_end: bool,
}

/// Parse an edge operator starting at `pos`, consuming an optional inline label.
/// Handles the forms:
/// - `-->`, `---`, `<-->`, `<--`, `-->|lbl|`, `<-->|lbl|`
/// - `-- lbl -->`, `--- lbl ---`
/// - thick `==>`, `===`, `<==>`, `==>|lbl|`, `== lbl ==>`
/// - dotted `-.->`, `-.-`, `<-.->`, `-.->|lbl|`, `-. lbl .->`
/// Returns `None` if there's no recognizable edge here.
fn parse_edge_op(bytes: &[u8], pos: &mut usize) -> Option<ParsedEdge> {
    let saved = *pos;

    // Optional leading arrowhead for `<-->`, `<==>`, `<-.->` forms.
    let mut arrow_start = false;
    if *pos < bytes.len() && bytes[*pos] == b'<' {
        arrow_start = true;
        *pos += 1;
    }

    // Determine the line character: thick uses `=`, otherwise `-` (which also
    // covers dotted, since dotted is `-.-`).
    let kind = if *pos < bytes.len() && bytes[*pos] == b'=' {
        EdgeKind::Thick
    } else if *pos < bytes.len() && bytes[*pos] == b'-' {
        EdgeKind::Normal // refined to Dotted below if a `.` is seen
    } else {
        *pos = saved;
        return None;
    };

    let line_ch = if kind == EdgeKind::Thick { b'=' } else { b'-' };

    // Consume the left run of line chars (and dots for dotted edges).
    let mut saw_dot = false;
    let consumed = consume_line_run(bytes, pos, line_ch, &mut saw_dot);
    if consumed == 0 {
        *pos = saved;
        return None;
    }

    // Two label styles after the left run:
    //   1. `|label|`            (pipe-delimited)
    //   2. `-- text -->` style  (text between two line runs); detected by the
    //      left run NOT ending in an arrow and a non-line, non-pipe char next.
    let mut label: Option<String> = None;
    let mut arrow_end = false;

    skip_ws(bytes, pos);

    // Did the left run already terminate with an arrowhead `>`?
    // consume_line_run stops at `>`; check for it now.
    if *pos < bytes.len() && bytes[*pos] == b'>' {
        arrow_end = true;
        *pos += 1;
        // Possible trailing `|label|` after `-->|lbl|`.
        skip_ws(bytes, pos);
        if *pos < bytes.len() && bytes[*pos] == b'|' {
            label = extract_pipe_label(bytes, pos);
        }
        return Some(finish_edge(kind, saw_dot, arrow_start, arrow_end, label));
    }

    // Position right after the left run (and its trailing whitespace). If the
    // inline-label form below turns out not to apply (no closing run), we rewind
    // here: the operator was a complete no-arrow edge (`A --- B`) and what we
    // tentatively read as "text" is actually the target node.
    let after_left_run = *pos;

    // No arrow yet. Either `|label|` follows, or it's `-- text -->`.
    if *pos < bytes.len() && bytes[*pos] == b'|' {
        label = extract_pipe_label(bytes, pos);
        skip_ws(bytes, pos);
        // After the pipe label there should be the rest of the arrow (rare for
        // the `--|lbl|-->` form), but the common `-->|lbl|` was handled above.
        // Consume any trailing run/arrow if present.
        if *pos < bytes.len() && (bytes[*pos] == line_ch) {
            let mut d = false;
            consume_line_run(bytes, pos, line_ch, &mut d);
        }
        if *pos < bytes.len() && bytes[*pos] == b'>' {
            arrow_end = true;
            *pos += 1;
        }
        return Some(finish_edge(kind, saw_dot, arrow_start, arrow_end, label));
    }

    // `-- text -->` form: read text up to the next line-char run.
    let text_start = *pos;
    while *pos < bytes.len() {
        let c = bytes[*pos];
        if c == line_ch || c == b'.' {
            // Could be the start of the closing run. Peek: a closing run is a
            // line char (optionally `.`) eventually followed by `>` or end.
            break;
        }
        *pos += 1;
    }
    let text = std::str::from_utf8(&bytes[text_start..*pos]).ok()?.trim();
    // Tentatively consume a closing run. The inline-label form (`-- text -->`)
    // requires one; without it, this is a plain no-arrow edge and `text` is the
    // target node, so we rewind to `after_left_run`.
    let mut d2 = false;
    let closing = consume_line_run(bytes, pos, line_ch, &mut d2);
    if closing == 0 {
        *pos = after_left_run;
        return Some(finish_edge(kind, saw_dot, arrow_start, arrow_end, None));
    }
    saw_dot = saw_dot || d2;
    if !text.is_empty() {
        label = Some(clean_label(text));
    }
    if *pos < bytes.len() && bytes[*pos] == b'>' {
        arrow_end = true;
        *pos += 1;
    }

    Some(finish_edge(kind, saw_dot, arrow_start, arrow_end, label))
}

/// Consume a run of `line_ch` characters (and intervening `.` for dotted
/// edges). Stops before `>` or any other char. Returns the count of line chars
/// consumed and sets `saw_dot` if a `.` appeared in the run.
fn consume_line_run(bytes: &[u8], pos: &mut usize, line_ch: u8, saw_dot: &mut bool) -> usize {
    let mut count = 0;
    while *pos < bytes.len() {
        let c = bytes[*pos];
        if c == line_ch {
            count += 1;
            *pos += 1;
        } else if c == b'.' {
            *saw_dot = true;
            *pos += 1;
        } else {
            break;
        }
    }
    count
}

/// Extract a `|label|` group starting at the `|` under `pos`.
fn extract_pipe_label(bytes: &[u8], pos: &mut usize) -> Option<String> {
    // bytes[*pos] == '|'
    *pos += 1;
    let start = *pos;
    while *pos < bytes.len() && bytes[*pos] != b'|' {
        *pos += 1;
    }
    let inner = std::str::from_utf8(&bytes[start..*pos]).ok()?;
    if *pos < bytes.len() {
        *pos += 1; // consume closing '|'
    }
    let cleaned = clean_label(inner);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Assemble a [`ParsedEdge`], promoting to dotted when a dot was seen.
fn finish_edge(
    kind: EdgeKind,
    saw_dot: bool,
    arrow_start: bool,
    arrow_end: bool,
    label: Option<String>,
) -> ParsedEdge {
    let kind = if saw_dot && kind == EdgeKind::Normal {
        EdgeKind::Dotted
    } else {
        kind
    };
    ParsedEdge {
        label,
        kind,
        arrow_start,
        arrow_end,
    }
}

/// Advance `pos` past ASCII whitespace.
fn skip_ws(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && bytes[*pos].is_ascii_whitespace() {
        *pos += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> FlowChart {
        parse_flowchart(src).expect("parse ok")
    }

    fn node<'a>(c: &'a FlowChart, id: &str) -> &'a FlowNode {
        c.nodes.iter().find(|n| n.id == id).expect("node present")
    }

    // ── Direction ──────────────────────────────────────────────────────────

    #[test]
    fn direction_td_and_tb_are_topdown() {
        assert_eq!(parse("graph TD\nA-->B").direction, Direction::TopDown);
        assert_eq!(parse("graph TB\nA-->B").direction, Direction::TopDown);
        assert_eq!(parse("flowchart TD\nA-->B").direction, Direction::TopDown);
    }

    #[test]
    fn direction_bt_lr_rl() {
        assert_eq!(parse("graph BT\nA-->B").direction, Direction::BottomUp);
        assert_eq!(parse("graph LR\nA-->B").direction, Direction::LeftRight);
        assert_eq!(parse("flowchart RL\nA-->B").direction, Direction::RightLeft);
    }

    #[test]
    fn direction_defaults_to_topdown_without_header() {
        let c = parse("A --> B");
        assert_eq!(c.direction, Direction::TopDown);
        assert_eq!(c.nodes.len(), 2);
        assert_eq!(c.edges.len(), 1);
    }

    #[test]
    fn header_without_dir_is_topdown() {
        assert_eq!(parse("graph\nA-->B").direction, Direction::TopDown);
    }

    // ── Node shapes ────────────────────────────────────────────────────────

    #[test]
    fn all_node_shapes() {
        let c = parse(
            "graph TD\n\
             A[rect]\n\
             B(round)\n\
             C([stad])\n\
             D((circ))\n\
             E{diam}\n\
             F{{hex}}",
        );
        assert_eq!(node(&c, "A").shape, NodeShape::Rect);
        assert_eq!(node(&c, "A").label, "rect");
        assert_eq!(node(&c, "B").shape, NodeShape::RoundRect);
        assert_eq!(node(&c, "B").label, "round");
        assert_eq!(node(&c, "C").shape, NodeShape::Stadium);
        assert_eq!(node(&c, "C").label, "stad");
        assert_eq!(node(&c, "D").shape, NodeShape::Circle);
        assert_eq!(node(&c, "D").label, "circ");
        assert_eq!(node(&c, "E").shape, NodeShape::Diamond);
        assert_eq!(node(&c, "E").label, "diam");
        assert_eq!(node(&c, "F").shape, NodeShape::Hexagon);
        assert_eq!(node(&c, "F").label, "hex");
    }

    #[test]
    fn bare_node_defaults_rect_label_is_id() {
        let c = parse("graph TD\nHello");
        assert_eq!(c.nodes.len(), 1);
        assert_eq!(node(&c, "Hello").shape, NodeShape::Rect);
        assert_eq!(node(&c, "Hello").label, "Hello");
    }

    #[test]
    fn default_rect_node_from_bare_edge_endpoint() {
        let c = parse("graph TD\nA --> B");
        assert_eq!(node(&c, "A").shape, NodeShape::Rect);
        assert_eq!(node(&c, "A").label, "A");
        assert_eq!(node(&c, "B").label, "B");
    }

    #[test]
    fn quoted_labels() {
        let c = parse("graph TD\nA[\"hello world\"] --> B{\"is it?\"}");
        assert_eq!(node(&c, "A").label, "hello world");
        assert_eq!(node(&c, "B").label, "is it?");
        assert_eq!(node(&c, "B").shape, NodeShape::Diamond);
    }

    // ── Edge kinds / arrowheads ────────────────────────────────────────────

    #[test]
    fn edge_arrow_end_only() {
        let c = parse("A --> B");
        let e = &c.edges[0];
        assert_eq!(e.kind, EdgeKind::Normal);
        assert!(e.arrow_end);
        assert!(!e.arrow_start);
    }

    #[test]
    fn edge_no_arrowheads() {
        let c = parse("A --- B");
        let e = &c.edges[0];
        assert!(!e.arrow_end);
        assert!(!e.arrow_start);
        assert_eq!(e.kind, EdgeKind::Normal);
    }

    #[test]
    fn edge_bidirectional() {
        let c = parse("A <--> B");
        let e = &c.edges[0];
        assert!(e.arrow_start);
        assert!(e.arrow_end);
    }

    #[test]
    fn edge_thick() {
        let c = parse("A ==> B");
        assert_eq!(c.edges[0].kind, EdgeKind::Thick);
        assert!(c.edges[0].arrow_end);
        let c2 = parse("A === B");
        assert_eq!(c2.edges[0].kind, EdgeKind::Thick);
        assert!(!c2.edges[0].arrow_end);
    }

    #[test]
    fn edge_dotted() {
        let c = parse("A -.-> B");
        assert_eq!(c.edges[0].kind, EdgeKind::Dotted);
        assert!(c.edges[0].arrow_end);
        let c2 = parse("A -.- B");
        assert_eq!(c2.edges[0].kind, EdgeKind::Dotted);
        assert!(!c2.edges[0].arrow_end);
    }

    #[test]
    fn edge_variable_dash_length() {
        let c = parse("A ---> B");
        assert_eq!(c.edges[0].kind, EdgeKind::Normal);
        assert!(c.edges[0].arrow_end);
        let c2 = parse("A ====> B");
        assert_eq!(c2.edges[0].kind, EdgeKind::Thick);
        assert!(c2.edges[0].arrow_end);
    }

    #[test]
    fn edge_label_pipe_form() {
        let c = parse("A -->|yes| B");
        assert_eq!(c.edges[0].label.as_deref(), Some("yes"));
        assert!(c.edges[0].arrow_end);
    }

    #[test]
    fn edge_label_inline_form_arrow() {
        let c = parse("A -- maybe --> B");
        assert_eq!(c.edges[0].label.as_deref(), Some("maybe"));
        assert!(c.edges[0].arrow_end);
        assert_eq!(c.edges[0].kind, EdgeKind::Normal);
    }

    #[test]
    fn edge_label_inline_form_no_arrow() {
        let c = parse("A --- link --- B");
        assert_eq!(c.edges[0].label.as_deref(), Some("link"));
        assert!(!c.edges[0].arrow_end);
    }

    #[test]
    fn edge_label_quoted() {
        let c = parse("A -->|\"a b\"| B");
        assert_eq!(c.edges[0].label.as_deref(), Some("a b"));
    }

    // ── Node ordering / dedup ──────────────────────────────────────────────

    #[test]
    fn first_seen_ordering_and_dedup() {
        let c = parse("graph TD\nB --> A\nA --> C\nB[bee]");
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["B", "A", "C"]);
        // B's label was set last by `B[bee]` (last-wins).
        assert_eq!(node(&c, "B").label, "bee");
    }

    #[test]
    fn last_wins_shape_and_label() {
        let c = parse("graph TD\nA[first] --> B\nA{second}");
        assert_eq!(node(&c, "A").label, "second");
        assert_eq!(node(&c, "A").shape, NodeShape::Diamond);
    }

    #[test]
    fn ref_before_def_defaults_then_updates() {
        // A referenced bare first, then shaped later.
        let c = parse("graph TD\nA --> B\nA([later])");
        assert_eq!(node(&c, "A").shape, NodeShape::Stadium);
        assert_eq!(node(&c, "A").label, "later");
    }

    // ── Chaining ───────────────────────────────────────────────────────────

    #[test]
    fn chaining_produces_sequential_edges() {
        let c = parse("graph TD\nA --> B --> C");
        assert_eq!(c.nodes.len(), 3);
        assert_eq!(c.edges.len(), 2);
        assert_eq!((c.edges[0].from.as_str(), c.edges[0].to.as_str()), ("A", "B"));
        assert_eq!((c.edges[1].from.as_str(), c.edges[1].to.as_str()), ("B", "C"));
    }

    // ── Separators / comments / blank lines ────────────────────────────────

    #[test]
    fn semicolon_separated_statements() {
        let c = parse("graph TD; A --> B; B --> C");
        assert_eq!(c.nodes.len(), 3);
        assert_eq!(c.edges.len(), 2);
    }

    #[test]
    fn comments_and_blank_lines_ignored() {
        let c = parse(
            "graph TD\n\
             %% this is a comment\n\
             \n\
             A --> B %% trailing comment\n\
             \n",
        );
        assert_eq!(c.nodes.len(), 2);
        assert_eq!(c.edges.len(), 1);
        assert!(c.edges[0].label.is_none());
    }

    #[test]
    fn unknown_line_is_skipped_not_errored() {
        let c = parse("graph TD\n!!!garbage###\nA --> B");
        assert_eq!(c.edges.len(), 1);
    }

    // ── Realistic multi-line flowchart ─────────────────────────────────────

    #[test]
    fn realistic_flowchart() {
        let c = parse(
            "graph TD\n\
             A[Start] --> B{OK?}\n\
             B -->|yes| C(Done)\n\
             B -->|no| A",
        );
        assert_eq!(c.direction, Direction::TopDown);

        // Nodes in first-seen order: A, B, C
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B", "C"]);
        assert_eq!(c.nodes.len(), 3);

        assert_eq!(node(&c, "A").shape, NodeShape::Rect);
        assert_eq!(node(&c, "A").label, "Start");
        assert_eq!(node(&c, "B").shape, NodeShape::Diamond);
        assert_eq!(node(&c, "B").label, "OK?");
        assert_eq!(node(&c, "C").shape, NodeShape::RoundRect);
        assert_eq!(node(&c, "C").label, "Done");

        // Edges: A->B (no label), B->C (yes), B->A (no)
        assert_eq!(c.edges.len(), 3);

        assert_eq!((c.edges[0].from.as_str(), c.edges[0].to.as_str()), ("A", "B"));
        assert!(c.edges[0].label.is_none());
        assert!(c.edges[0].arrow_end);

        assert_eq!((c.edges[1].from.as_str(), c.edges[1].to.as_str()), ("B", "C"));
        assert_eq!(c.edges[1].label.as_deref(), Some("yes"));

        assert_eq!((c.edges[2].from.as_str(), c.edges[2].to.as_str()), ("B", "A"));
        assert_eq!(c.edges[2].label.as_deref(), Some("no"));
    }

    #[test]
    fn empty_source_yields_empty_chart() {
        let c = parse("");
        assert!(c.nodes.is_empty());
        assert!(c.edges.is_empty());
        assert_eq!(c.direction, Direction::TopDown);
    }

    // ── Styling directives ─────────────────────────────────────────────────

    #[test]
    fn color_parser_forms() {
        assert_eq!(parse_color("#f00"), Some([255, 0, 0, 255]));
        assert_eq!(parse_color("#00ff00"), Some([0, 255, 0, 255]));
        assert_eq!(parse_color("#0000ff80"), Some([0, 0, 255, 128]));
        assert_eq!(parse_color("rgb(1,2,3)"), Some([1, 2, 3, 255]));
        assert_eq!(parse_color("rgba(1,2,3,0.5)"), Some([1, 2, 3, 128]));
        assert_eq!(parse_color("red"), Some([255, 0, 0, 255]));
        assert_eq!(parse_color("RED"), Some([255, 0, 0, 255]));
        assert_eq!(parse_color("notacolor"), None);
    }

    #[test]
    fn width_parser() {
        assert_eq!(parse_width("2px"), Some(2.0));
        assert_eq!(parse_width("4"), Some(4.0));
        assert_eq!(parse_width("1.5"), Some(1.5));
    }

    #[test]
    fn classdef_and_class_apply() {
        let c = parse(
            "graph TD\n\
             A --> B\n\
             classDef hot fill:#f00,stroke:#900,stroke-width:3px\n\
             class A hot",
        );
        let a = node(&c, "A");
        assert_eq!(a.style.fill, Some([255, 0, 0, 255]));
        assert!(a.style.stroke.is_some());
        assert_eq!(a.style.stroke_width, Some(3.0));
        // B untouched.
        assert_eq!(node(&c, "B").style, ElemStyle::default());
    }

    #[test]
    fn classdef_defined_after_class_still_resolves() {
        // Two-pass: `class` references `hot` before its classDef appears.
        let c = parse(
            "graph TD\n\
             class A hot\n\
             A --> B\n\
             classDef hot fill:#0f0",
        );
        assert_eq!(node(&c, "A").style.fill, Some([0, 255, 0, 255]));
    }

    #[test]
    fn class_shorthand_triple_colon() {
        let c = parse(
            "graph TD\n\
             A:::hot --> B\n\
             classDef hot fill:#f00",
        );
        assert_eq!(node(&c, "A").style.fill, Some([255, 0, 0, 255]));
        // A is still a normal node with an edge to B.
        assert_eq!(c.edges.len(), 1);
        assert_eq!((c.edges[0].from.as_str(), c.edges[0].to.as_str()), ("A", "B"));
    }

    #[test]
    fn class_shorthand_with_shape() {
        let c = parse(
            "graph TD\n\
             A[Start]:::hot --> B\n\
             classDef hot fill:#0000ff",
        );
        assert_eq!(node(&c, "A").label, "Start");
        assert_eq!(node(&c, "A").style.fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn style_directive_direct() {
        let c = parse("graph TD\nA --> B\nstyle B fill:#0f0");
        assert_eq!(node(&c, "B").style.fill, Some([0, 255, 0, 255]));
    }

    #[test]
    fn style_overrides_class() {
        // Inline `style` wins over `class` (applied on top).
        let c = parse(
            "graph TD\n\
             A --> B\n\
             classDef hot fill:#f00\n\
             class A hot\n\
             style A fill:#00f",
        );
        assert_eq!(node(&c, "A").style.fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn linkstyle_sets_edge() {
        let c = parse("graph TD\nA --> B\nlinkStyle 0 stroke:#00f");
        assert_eq!(c.edges[0].style.stroke, Some([0, 0, 255, 255]));
    }

    #[test]
    fn linkstyle_default_and_multi_index() {
        let c = parse(
            "graph TD\n\
             A --> B\n\
             B --> C\n\
             linkStyle default stroke:#000\n\
             linkStyle 0,1 stroke-width:4px",
        );
        assert_eq!(c.edges[0].style.stroke, Some([0, 0, 0, 255]));
        assert_eq!(c.edges[1].style.stroke, Some([0, 0, 0, 255]));
        assert_eq!(c.edges[0].style.stroke_width, Some(4.0));
        assert_eq!(c.edges[1].style.stroke_width, Some(4.0));
    }

    #[test]
    fn dasharray_sets_dashed() {
        let c = parse("graph TD\nA --> B\nstyle A stroke-dasharray:5 5");
        assert!(node(&c, "A").style.dashed);
    }

    #[test]
    fn unknown_color_prop_skipped() {
        let c = parse("graph TD\nA --> B\nstyle A fill:notacolor,stroke:#f00");
        assert_eq!(node(&c, "A").style.fill, None);
        assert_eq!(node(&c, "A").style.stroke, Some([255, 0, 0, 255]));
    }

    // ── Subgraphs ──────────────────────────────────────────────────────────

    #[test]
    fn subgraph_groups_members_titled() {
        let c = parse(
            "flowchart TD\n\
             subgraph one [Group One]\n\
             A --> B\n\
             end\n\
             B --> C",
        );
        assert_eq!(c.subgraphs.len(), 1);
        let sg = &c.subgraphs[0];
        assert_eq!(sg.id, "one");
        assert_eq!(sg.title, "Group One");
        assert_eq!(sg.node_ids, vec!["A", "B"]);
        assert!(sg.parent.is_none());
        // C is top-level (declared outside the subgraph block).
        assert!(!sg.node_ids.contains(&"C".to_string()));
        // Nodes & edges still parse normally.
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B", "C"]);
        assert_eq!(c.edges.len(), 2);
    }

    #[test]
    fn subgraph_bare_id_uses_id_as_title() {
        let c = parse("flowchart TD\nsubgraph svc\nA --> B\nend");
        assert_eq!(c.subgraphs.len(), 1);
        assert_eq!(c.subgraphs[0].id, "svc");
        assert_eq!(c.subgraphs[0].title, "svc");
        assert_eq!(c.subgraphs[0].node_ids, vec!["A", "B"]);
    }

    #[test]
    fn subgraph_quoted_title() {
        let c = parse("flowchart TD\nsubgraph \"My Title\"\nA --> B\nend");
        assert_eq!(c.subgraphs.len(), 1);
        assert_eq!(c.subgraphs[0].title, "My Title");
    }

    #[test]
    fn nested_subgraphs_set_parent() {
        let c = parse(
            "flowchart TD\n\
             subgraph outer [Outer]\n\
             A --> B\n\
             subgraph inner [Inner]\n\
             C --> D\n\
             end\n\
             end",
        );
        assert_eq!(c.subgraphs.len(), 2);
        let outer = &c.subgraphs[0];
        let inner = &c.subgraphs[1];
        assert_eq!(outer.title, "Outer");
        assert_eq!(outer.node_ids, vec!["A", "B"]);
        assert!(outer.parent.is_none());
        assert_eq!(inner.title, "Inner");
        assert_eq!(inner.node_ids, vec!["C", "D"]);
        // inner's parent is the index of `outer` (0).
        assert_eq!(inner.parent, Some(0));
    }

    #[test]
    fn direction_inside_subgraph_ignored() {
        let c = parse(
            "flowchart TD\n\
             subgraph one\n\
             direction LR\n\
             A --> B\n\
             end",
        );
        // Whole-chart direction is unchanged; direction line creates no node.
        assert_eq!(c.direction, Direction::TopDown);
        assert_eq!(c.subgraphs[0].node_ids, vec!["A", "B"]);
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B"]);
    }

    #[test]
    fn no_subgraph_chart_has_no_subgraphs() {
        let c = parse("flowchart TD\nA --> B --> C");
        assert!(c.subgraphs.is_empty());
    }

    // ── Click / interaction directives ─────────────────────────────────────

    #[test]
    fn click_url_sets_link() {
        let c = parse("graph TD\nA[Start]\nclick A \"https://x\"");
        assert_eq!(node(&c, "A").link.as_deref(), Some("https://x"));
        assert!(node(&c, "A").tooltip.is_none());
        assert!(node(&c, "A").callback.is_none());
    }

    #[test]
    fn click_href_url_sets_link() {
        let c = parse("graph TD\nA[Start]\nclick A href \"https://x\"");
        assert_eq!(node(&c, "A").link.as_deref(), Some("https://x"));
    }

    #[test]
    fn click_url_and_tooltip() {
        let c = parse("graph TD\nA[Start]\nclick A \"https://x\" \"go there\"");
        assert_eq!(node(&c, "A").link.as_deref(), Some("https://x"));
        assert_eq!(node(&c, "A").tooltip.as_deref(), Some("go there"));
    }

    #[test]
    fn click_href_url_and_tooltip() {
        let c = parse("graph TD\nA[Start]\nclick A href \"u\" \"tip\"");
        assert_eq!(node(&c, "A").link.as_deref(), Some("u"));
        assert_eq!(node(&c, "A").tooltip.as_deref(), Some("tip"));
    }

    #[test]
    fn click_call_sets_callback_dropping_args() {
        let c = parse("graph TD\nA\nclick A call doThing()");
        assert_eq!(node(&c, "A").callback.as_deref(), Some("doThing"));
        assert!(node(&c, "A").link.is_none());
    }

    #[test]
    fn click_call_with_args_and_tooltip() {
        let c = parse("graph TD\nA\nclick A call doThing(1, 2) \"tip\"");
        assert_eq!(node(&c, "A").callback.as_deref(), Some("doThing"));
        assert_eq!(node(&c, "A").tooltip.as_deref(), Some("tip"));
    }

    #[test]
    fn click_bareword_callback() {
        let c = parse("graph TD\nA\nclick A myCallback");
        assert_eq!(node(&c, "A").callback.as_deref(), Some("myCallback"));
    }

    #[test]
    fn click_url_with_target_ignored() {
        let c = parse("graph TD\nA\nclick A \"https://x\" _blank");
        assert_eq!(node(&c, "A").link.as_deref(), Some("https://x"));
        // _blank is tolerated and not recorded as a callback.
        assert!(node(&c, "A").callback.is_none());
    }

    #[test]
    fn click_unknown_id_autocreates_node() {
        let c = parse("graph TD\nA --> B\nclick Z \"https://z\"");
        let z = node(&c, "Z");
        assert_eq!(z.shape, NodeShape::Rect);
        assert_eq!(z.label, "Z");
        assert_eq!(z.link.as_deref(), Some("https://z"));
    }

    #[test]
    fn click_is_not_an_edge_or_node_statement() {
        // A click line must not create phantom nodes beyond its target, nor edges.
        let c = parse("graph TD\nA --> B\nclick A \"u\"");
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B"]);
        assert_eq!(c.edges.len(), 1);
    }

    #[test]
    fn directives_do_not_create_phantom_nodes() {
        let c = parse(
            "graph TD\n\
             A --> B\n\
             classDef hot fill:#f00\n\
             class A hot",
        );
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B"]);
    }
}
