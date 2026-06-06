//! Flowchart node/edge token grammar: the byte-level recursive-descent scanner
//! for node refs (`A[..]`, `A(..)`, `A@{..}`, `:::class`) and edge operators
//! (`-->`, `-.->`, `==>`, `--text-->`, `-->|text|`, …). Produces the intermediate
//! [`ParsedNode`]/[`ParsedEdge`] records consumed by [`super`]'s statement walk.

use crate::model::{EdgeKind, NodeShape};

use super::directives::Directives;

/// A node ref parsed out of source: id plus optional explicit label/shape.
pub(super) struct ParsedNode {
    pub(super) id: String,
    /// `Some` when the ref carried a bracketed/shaped label; `None` for a bare id.
    pub(super) label: Option<String>,
    pub(super) shape: NodeShape,
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


/// Is `c` a valid node-id character? Lenient: letters, digits, underscore,
/// plus `-`/`.` (mermaid allows these in ids), but we keep it conservative so
/// edge operators (which start with `-`/`=`/`<`) aren't swallowed.
fn is_id_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Parse a node ref at `pos`: an identifier optionally followed by a shape
/// bracket group. Advances `pos` past it. Returns `None` if no id is present.
pub(super) fn parse_node_ref(bytes: &[u8], pos: &mut usize, dir: &mut Directives) -> Option<ParsedNode> {
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
    let (mut label, mut shape) = parse_shape(bytes, pos);

    // Optional mermaid-11 `@{ shape: …, label: … }` suffix (may appear with no
    // bracket shape, i.e. directly after the id). Its `shape:`/`label:` override
    // anything from the bracket form.
    if starts_with(bytes, *pos, b"@{") {
        if let Some((at_shape, at_label)) = parse_at_shape(bytes, pos) {
            if let Some(s) = at_shape {
                shape = s;
            }
            if let Some(l) = at_label {
                label = Some(l);
            } else if label.is_none() {
                // An `@{}` with no `label:` still marks the node as "shaped" so
                // the id is used as the label (mermaid behavior).
                label = Some(id.clone());
            }
        }
    }

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
            // Extended bracket shapes (longest-match first so e.g. `[(` beats
            // `[`): Cylinder `[(..)]`, Subroutine `[[..]]`, and the slant family
            // `[/../]` `[\..\]` `[/..\]` `[\../]`. Otherwise a plain Rect `[..]`.
            match bytes.get(*pos + 1) {
                Some(b'(') => extract_group(bytes, pos, b"[(", b")]")
                    .map_or((None, NodeShape::Rect), |t| (Some(t), NodeShape::Cylinder)),
                Some(b'[') => extract_group(bytes, pos, b"[[", b"]]")
                    .map_or((None, NodeShape::Rect), |t| (Some(t), NodeShape::Subroutine)),
                // `[/…/]` (Parallelogram) vs `[/…\]` (Trapezoid): same `[/`
                // opener, disambiguated by the char just before the closing `]`.
                Some(b'/') => parse_slant_shape(
                    bytes,
                    pos,
                    b"[/",
                    NodeShape::Parallelogram, // closes with `/]`
                    NodeShape::Trapezoid,     // closes with `\]`
                ),
                // `[\…\]` (ParallelogramAlt) vs `[\…/]` (TrapezoidAlt).
                Some(b'\\') => parse_slant_shape(
                    bytes,
                    pos,
                    b"[\\",
                    NodeShape::TrapezoidAlt,     // closes with `/]`
                    NodeShape::ParallelogramAlt, // closes with `\]`
                ),
                _ => extract_group(bytes, pos, b"[", b"]")
                    .map_or((None, NodeShape::Rect), |t| (Some(t), NodeShape::Rect)),
            }
        }
        b'(' => {
            // DoubleCircle A(((..))), Circle A((..)), Stadium A([..]), or
            // RoundRect A(..). Longest-match first so `(((` beats `((`.
            if starts_with(bytes, *pos, b"(((") {
                extract_group(bytes, pos, b"(((", b")))")
                    .map_or((None, NodeShape::Rect), |t| (Some(t), NodeShape::DoubleCircle))
            } else if peek2(bytes, *pos) == Some((b'(', b'(')) {
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

/// Parse a slant-bracket shape sharing the opener `open` (`[/` or `[\`). Both
/// the parallelogram and trapezoid variants open the same way; the closing
/// delimiter (`/]` vs `\]`) decides which. We scan quote-aware to the matching
/// `]` and inspect the char immediately before it: `/` → `if_slash`, `\` →
/// `if_back`. On a missing/ambiguous close, leaves `pos` unchanged → Rect.
fn parse_slant_shape(
    bytes: &[u8],
    pos: &mut usize,
    open: &[u8],
    if_slash: NodeShape,
    if_back: NodeShape,
) -> (Option<String>, NodeShape) {
    debug_assert!(starts_with(bytes, *pos, open));
    let inner_start = *pos + open.len();
    let mut i = inner_start;
    let mut in_quotes = false;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            in_quotes = !in_quotes;
            i += 1;
            continue;
        }
        // The close is a `]` preceded (outside quotes) by `/` or `\`. The slant
        // char sits at i-1 and the `]` at i; the inner label is everything up to
        // that slant char.
        if !in_quotes && bytes[i] == b']' && i > inner_start {
            let close = bytes[i - 1];
            let shape = match close {
                b'/' => if_slash,
                b'\\' => if_back,
                _ => {
                    i += 1;
                    continue;
                }
            };
            let inner = match std::str::from_utf8(&bytes[inner_start..i - 1]) {
                Ok(s) => clean_label(s),
                Err(_) => return (None, NodeShape::Rect),
            };
            *pos = i + 1;
            return (Some(inner), shape);
        }
        i += 1;
    }
    (None, NodeShape::Rect)
}

/// Parse a mermaid-11 `@{ key: value, … }` node suffix starting at `pos` (which
/// must point at `@{`). Advances `pos` past the closing `}`. Returns
/// `(shape, label)` where `shape`/`label` are `Some` if those keys were present.
/// Quote-aware on values; unknown keys (e.g. `icon:`) are ignored. On a missing
/// `}`, leaves `pos` unchanged and returns `None`.
fn parse_at_shape(bytes: &[u8], pos: &mut usize) -> Option<(Option<NodeShape>, Option<String>)> {
    let saved = *pos;
    // Skip `@{`.
    let mut i = *pos + 2;
    let inner_start = i;
    let mut in_quotes = false;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            in_quotes = !in_quotes;
        } else if !in_quotes && bytes[i] == b'}' {
            break;
        }
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'}' {
        *pos = saved;
        return None;
    }
    let inner = std::str::from_utf8(&bytes[inner_start..i]).ok()?;
    *pos = i + 1;

    let mut shape: Option<NodeShape> = None;
    let mut label: Option<String> = None;
    for (key, val) in split_kv_pairs(inner) {
        match key.as_str() {
            "shape" => shape = Some(shape_from_name(&val)),
            "label" | "title" => label = Some(val),
            _ => {} // icon:, etc. — ignored
        }
    }
    Some((shape, label))
}

/// Split a quote-aware comma-separated `key: value` list (the body of an `@{ … }`
/// group). Values may be double-quoted (quotes stripped). Pairs without a `:` are
/// skipped.
fn split_kv_pairs(inner: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = inner.as_bytes();
    let mut i = 0;
    let mut start = 0;
    let mut in_quotes = false;
    // Split on top-level commas (quote-aware), then split each part on its first
    // `:`.
    let mut parts: Vec<&str> = Vec::new();
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_quotes = !in_quotes,
            b',' if !in_quotes => {
                parts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&inner[start..]);

    for part in parts {
        if let Some((k, v)) = part.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            if key.is_empty() {
                continue;
            }
            out.push((key, clean_label(v)));
        }
    }
    out
}

/// Map a mermaid shape name (or one of its aliases) to a [`NodeShape`]. Unknown
/// names fall back to `Rect`.
fn shape_from_name(name: &str) -> NodeShape {
    match name.trim().to_ascii_lowercase().as_str() {
        "cyl" | "cylinder" | "das" | "database" | "db" => NodeShape::Cylinder,
        "subproc" | "subroutine" | "fr-rect" | "framed-rectangle" => NodeShape::Subroutine,
        "doc" | "document" => NodeShape::Document,
        "lean-r" | "lean-right" | "in-out" | "parallelogram" => NodeShape::Parallelogram,
        "lean-l" | "lean-left" | "out-in" => NodeShape::ParallelogramAlt,
        "trap-b" | "trapezoid" | "trapezoid-bottom" | "priority" => NodeShape::Trapezoid,
        "trap-t" | "trapezoid-top" | "manual-input" => NodeShape::TrapezoidAlt,
        "dbl-circ" | "double-circle" => NodeShape::DoubleCircle,
        "rect" | "rectangle" | "process" => NodeShape::Rect,
        "rounded" => NodeShape::RoundRect,
        "stadium" | "pill" => NodeShape::Stadium,
        "circle" | "circ" => NodeShape::Circle,
        "diam" | "diamond" | "decision" => NodeShape::Diamond,
        "hex" | "hexagon" => NodeShape::Hexagon,
        _ => NodeShape::Rect, // unknown → Rect
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
pub(super) fn clean_label(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// A parsed edge operator.
pub(super) struct ParsedEdge {
    pub(super) label: Option<String>,
    pub(super) kind: EdgeKind,
    pub(super) arrow_start: bool,
    pub(super) arrow_end: bool,
}

/// Parse an edge operator starting at `pos`, consuming an optional inline label.
/// Handles the forms:
/// - `-->`, `---`, `<-->`, `<--`, `-->|lbl|`, `<-->|lbl|`
/// - `-- lbl -->`, `--- lbl ---`
/// - thick `==>`, `===`, `<==>`, `==>|lbl|`, `== lbl ==>`
/// - dotted `-.->`, `-.-`, `<-.->`, `-.->|lbl|`, `-. lbl .->`
/// Returns `None` if there's no recognizable edge here.
pub(super) fn parse_edge_op(bytes: &[u8], pos: &mut usize) -> Option<ParsedEdge> {
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
pub(super) fn skip_ws(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && bytes[*pos].is_ascii_whitespace() {
        *pos += 1;
    }
}

