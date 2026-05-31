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

use crate::model::{Direction, EdgeKind, FlowChart, FlowEdge, FlowNode, NodeShape};

/// Parse mermaid flowchart source (e.g. `graph TD; A[Start] --> B{Decision}`)
/// into a [`FlowChart`]. Returns `Err(message)` on a syntax error.
pub fn parse_flowchart(src: &str) -> Result<FlowChart, String> {
    let mut chart = FlowChart {
        direction: Direction::TopDown,
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    // Tracks insertion index of each node id so we can update (last-wins) the
    // existing entry rather than appending a duplicate.
    let mut node_index: Vec<(String, usize)> = Vec::new();

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

            parse_statement(stmt, &mut chart, &mut node_index);
        }
    }

    Ok(chart)
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
fn parse_statement(stmt: &str, chart: &mut FlowChart, node_index: &mut Vec<(String, usize)>) {
    let bytes = stmt.as_bytes();
    let mut pos = 0usize;

    // First node ref is required for any statement we care about.
    let first = match parse_node_ref(bytes, &mut pos) {
        Some(n) => n,
        None => return,
    };
    upsert_node(chart, node_index, first.clone());

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
        let target = match parse_node_ref(bytes, &mut pos) {
            Some(n) => n,
            None => break, // edge with no target; drop it (lenient)
        };
        let target_id = target.id.clone();
        upsert_node(chart, node_index, target);

        chart.edges.push(FlowEdge {
            from: prev_id.clone(),
            to: target_id.clone(),
            label: edge.label,
            kind: edge.kind,
            arrow_start: edge.arrow_start,
            arrow_end: edge.arrow_end,
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
fn upsert_node(chart: &mut FlowChart, node_index: &mut Vec<(String, usize)>, parsed: ParsedNode) {
    let existing = node_index.iter().find(|(id, _)| *id == parsed.id).map(|(_, i)| *i);
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
            });
            node_index.push((parsed.id, idx));
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
fn parse_node_ref(bytes: &[u8], pos: &mut usize) -> Option<ParsedNode> {
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
    while i < bytes.len() {
        if starts_with(bytes, i, close) {
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
}
