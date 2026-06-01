//! `state` diagram (`stateDiagram` / `stateDiagram-v2`).
//!
//! Self-contained: parse → build a `hiker_graph` layered (dagre) graph →
//! lay out → draw one SVG document. Supported subset:
//!
//! * states `s1` and described states `s1 : Some text` (label = text).
//! * start / end pseudo-state `[*]`: as a transition **source** it is a start
//!   (small filled circle); as a **target** it is an end (filled circle with an
//!   outer ring). A single synthetic start node and a single synthetic end node
//!   are shared across all occurrences (matching mermaid's one-start/one-end).
//! * transitions `s1 --> s2` and `s1 --> s2 : label`.
//! * composite/nested states `state X { ... }` (rendered as a labeled boundary
//!   box via the cluster API), `state f <<fork>>` / `<<join>>` (a thick bar),
//!   `state c <<choice>>` (a diamond), the `state "desc" as id` alias form, and
//!   `note left/right of S: text` / `note over S: text` / block notes.
//!
//! Skipped (note in report): `--` concurrency dividers.

use std::collections::HashMap;
use std::fmt::Write as _;

use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

use crate::model::ElemStyle;
use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ── Styling directives (classDef / class / style / :::) ───────────────────────
//
// Self-contained re-implementation of the flowchart styling parser (`parse.rs`),
// mirroring its syntax/semantics: same prop names, same color formats, same
// two-pass resolve (classDef-via-`class` first, inline `style` on top).

/// Directive state collected during parsing, resolved onto states at the end.
#[derive(Default)]
struct Directives {
    class_defs: HashMap<String, ElemStyle>,
    /// `(state id, class name)` from `class A,B name` and `A:::name`.
    class_assignments: Vec<(String, String)>,
    /// Inline `style <id> ...` overrides.
    inline: Vec<(String, ElemStyle)>,
}

impl Directives {
    /// Try to parse `line` as a styling directive; `true` if recognized.
    fn try_parse(&mut self, line: &str) -> bool {
        let mut words = line.split_whitespace();
        let kw = match words.next() {
            Some(k) => k,
            None => return false,
        };
        match kw {
            "classDef" => {
                let rest = line[kw.len()..].trim_start();
                let mut parts = rest.splitn(2, char::is_whitespace);
                if let Some(name) = parts.next().filter(|n| !n.is_empty()) {
                    let props = parts.next().unwrap_or("");
                    self.class_defs
                        .insert(name.to_string(), parse_style_props(props));
                }
                true
            }
            "class" => {
                let rest = line[kw.len()..].trim_start();
                if let Some(sp) = rest.rfind(char::is_whitespace) {
                    let ids = rest[..sp].trim();
                    let class_name = rest[sp..].trim();
                    if !class_name.is_empty() {
                        for id in ids.split(',') {
                            let id = id.trim();
                            if !id.is_empty() {
                                self.class_assignments
                                    .push((id.to_string(), class_name.to_string()));
                            }
                        }
                    }
                }
                true
            }
            "style" => {
                let rest = line[kw.len()..].trim_start();
                let mut parts = rest.splitn(2, char::is_whitespace);
                if let Some(id) = parts.next().filter(|n| !n.is_empty()) {
                    let props = parts.next().unwrap_or("");
                    self.inline.push((id.to_string(), parse_style_props(props)));
                }
                true
            }
            _ => false,
        }
    }

    fn add_shorthand(&mut self, id: &str, class_name: &str) {
        if !id.is_empty() && !class_name.is_empty() {
            self.class_assignments
                .push((id.to_string(), class_name.to_string()));
        }
    }

    /// Resolve onto each state's `style`: classDef-via-`class` first, then inline.
    fn resolve(&self, states: &mut [State]) {
        for (id, class_name) in &self.class_assignments {
            if let Some(cs) = self.class_defs.get(class_name) {
                if let Some(s) = states.iter_mut().find(|s| s.id == *id) {
                    merge_style(&mut s.style, cs);
                }
            }
        }
        for (id, st) in &self.inline {
            if let Some(s) = states.iter_mut().find(|s| s.id == *id) {
                merge_style(&mut s.style, st);
            }
        }
    }
}

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

fn parse_style_props(props: &str) -> ElemStyle {
    let mut style = ElemStyle::default();
    for part in props.split(',') {
        let (key, val) = match part.split_once(':') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => continue,
        };
        match key {
            "fill" => style.fill = parse_color(val).or(style.fill),
            "stroke" => style.stroke = parse_color(val).or(style.stroke),
            "color" => style.text_color = parse_color(val).or(style.text_color),
            "stroke-width" => {
                if let Ok(w) = val.trim_end_matches("px").trim().parse::<f32>() {
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

fn parse_hex_color(h: &str) -> Option<[u8; 4]> {
    let h = h.trim();
    match h.len() {
        3 => {
            let r = u8::from_str_radix(&h[0..1], 16).ok()?;
            let g = u8::from_str_radix(&h[1..2], 16).ok()?;
            let b = u8::from_str_radix(&h[2..3], 16).ok()?;
            Some([r * 17, g * 17, b * 17, 255])
        }
        6 => Some([
            u8::from_str_radix(&h[0..2], 16).ok()?,
            u8::from_str_radix(&h[2..4], 16).ok()?,
            u8::from_str_radix(&h[4..6], 16).ok()?,
            255,
        ]),
        8 => Some([
            u8::from_str_radix(&h[0..2], 16).ok()?,
            u8::from_str_radix(&h[2..4], 16).ok()?,
            u8::from_str_radix(&h[4..6], 16).ok()?,
            u8::from_str_radix(&h[6..8], 16).ok()?,
        ]),
        _ => None,
    }
}

fn parse_rgb_func(inner: &str, with_alpha: bool) -> Option<[u8; 4]> {
    let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
    if parts.len() != if with_alpha { 4 } else { 3 } {
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

/// Synthetic id for the shared start pseudo-state.
const START_ID: &str = "\0start";
/// Synthetic id for the shared end pseudo-state.
const END_ID: &str = "\0end";
/// Diameter of a pseudo-state circle, px.
const PSEUDO_SIZE: f32 = 18.0;
/// Length (long axis) of a fork/join bar, px.
const FORK_LEN: f32 = 70.0;
/// Thickness (short axis) of a fork/join bar, px.
const FORK_THICK: f32 = 10.0;
/// Bounding size of a choice diamond, px.
const CHOICE_SIZE: f32 = 40.0;

/// A state node. `pseudo` is `None` for a real state, or `Start`/`End` for the
/// two synthetic pseudo-states.
#[derive(Clone, Debug, PartialEq)]
struct State {
    id: String,
    label: String,
    pseudo: Option<Pseudo>,
    /// Special marker shape, if any (`<<fork>>`/`<<join>>`/`<<choice>>`).
    kind: StateKind,
    /// Index (into `states`) of the composite that directly contains this state,
    /// or `None` for a top-level state.
    parent: Option<usize>,
    /// True if this state is itself a composite (has a `{ … }` body).
    composite: bool,
    /// Per-state style overrides (from `classDef`/`class`/`style`/`:::`).
    style: ElemStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pseudo {
    Start,
    End,
}

/// Special marker shape for `<<fork>>` / `<<join>>` / `<<choice>>` states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum StateKind {
    #[default]
    Normal,
    Fork,
    Join,
    Choice,
}

/// Where a note is anchored relative to its target state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NotePos {
    Left,
    Right,
    Over,
}

/// A note attached to a state. Not part of the dagre graph: placed beside the
/// target's final position after layout.
#[derive(Clone, Debug, PartialEq)]
struct Note {
    target: String,
    pos: NotePos,
    text: String,
}

/// A transition `from --> to` with an optional label.
#[derive(Clone, Debug, PartialEq)]
struct Transition {
    from: String,
    to: String,
    label: Option<String>,
}

/// Parsed state diagram.
#[derive(Clone, Debug, Default, PartialEq)]
struct StateDiagram {
    /// States in first-seen order.
    states: Vec<State>,
    transitions: Vec<Transition>,
    notes: Vec<Note>,
}

/// Parse a state diagram source. Errors on a missing/wrong header.
fn parse(src: &str) -> Result<StateDiagram, String> {
    // Header: first non-blank, non-comment line must be stateDiagram[-v2].
    let mut saw_header = false;
    let mut diag = StateDiagram::default();
    let mut directives = Directives::default();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    // Stack of currently-open composite container indices (innermost last).
    let mut parents: Vec<usize> = Vec::new();
    // Pending block note: `Some((target, pos, accumulated lines))` between a
    // `note … <S>` opener (no inline `:`) and its `end note`.
    let mut pending_note: Option<(String, NotePos, Vec<String>)> = None;

    // Walk every line; the first meaningful one is the header.
    let mut pending_header = true;
    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if pending_header {
            let kw = line.split_whitespace().next().unwrap_or("");
            if kw != "stateDiagram" && kw != "stateDiagram-v2" {
                return Err(format!("expected `stateDiagram` header, got {kw:?}"));
            }
            saw_header = true;
            pending_header = false;
            continue;
        }

        // Inside an open block note: accumulate until `end note`.
        if let Some((target, pos, lines)) = pending_note.as_mut() {
            if line == "end note" || line == "end" {
                let text = lines.join("\n");
                diag.notes.push(Note {
                    target: std::mem::take(target),
                    pos: *pos,
                    text,
                });
                pending_note = None;
            } else {
                lines.push(line.to_string());
            }
            continue;
        }

        // `direction TB` etc. — accepted but only the default Tb is used in v1.
        if line.starts_with("direction") {
            continue;
        }
        // Styling directives (`classDef`/`class`/`style`). These contain `:` and
        // must be intercepted before the generic state/description line parser.
        if line.starts_with("classDef")
            || line.starts_with("class ")
            || line.starts_with("style ")
        {
            directives.try_parse(line);
            continue;
        }

        // Note (inline `note … of S: text` / `note over S: text`, or a block
        // note opener `note … of S` followed by lines and `end note`).
        if line == "note" || line.starts_with("note ") {
            if let Some((target, pos, inline)) = parse_note_header(line) {
                match inline {
                    Some(text) => diag.notes.push(Note { target, pos, text }),
                    None => pending_note = Some((target, pos, Vec::new())),
                }
            }
            continue;
        }

        // Close the current composite.
        if line == "}" {
            parents.pop();
            continue;
        }

        // Concurrency divider — skipped.
        if line == "--" || (line.starts_with("--") && !line.contains("-->")) {
            continue;
        }

        // `state …` declarations: composite opener, marker, alias, or plain.
        if line == "state" || line.starts_with("state ") {
            parse_state_decl(line, &mut diag, &mut directives, &mut index_of, &mut parents);
            continue;
        }

        parse_line(line, &mut diag, &mut directives, &mut index_of, &parents);
    }

    if !saw_header {
        return Err("empty input / no stateDiagram header".to_string());
    }
    directives.resolve(&mut diag.states);
    Ok(diag)
}

/// Parse a `note left of S: text` / `note right of S` / `note over S, T` header.
/// Returns `(target id, position, Some(inline text) | None for a block note)`.
/// `None` overall when the line isn't a well-formed note.
fn parse_note_header(line: &str) -> Option<(String, NotePos, Option<String>)> {
    // Split off the inline `: text` (a block note has none).
    let (head, inline) = match line.split_once(':') {
        Some((h, t)) => (h.trim(), Some(t.trim().to_string())),
        None => (line, None),
    };
    let rest = head.strip_prefix("note")?.trim_start();
    let (pos, after) = if let Some(a) = rest.strip_prefix("left of") {
        (NotePos::Left, a)
    } else if let Some(a) = rest.strip_prefix("right of") {
        (NotePos::Right, a)
    } else if let Some(a) = rest.strip_prefix("over") {
        (NotePos::Over, a)
    } else {
        return None;
    };
    // Target is the first state id (mermaid allows `note over A, B`; take A).
    let target = after.trim().split(',').next().unwrap_or("").trim();
    if target.is_empty() {
        return None;
    }
    Some((target.to_string(), pos, inline))
}

/// Parse a `state …` declaration. Handles:
/// * `state Composite {` — opens a composite (pushes onto `parents`).
/// * `state id <<fork>>` / `<<join>>` / `<<choice>>` — a marker state.
/// * `state "Long description" as id` — an aliased state (label = description).
/// * `state id` / `state id : desc` — plain declaration inside a composite.
fn parse_state_decl(
    line: &str,
    diag: &mut StateDiagram,
    directives: &mut Directives,
    index_of: &mut HashMap<String, usize>,
    parents: &mut Vec<usize>,
) {
    let body = line.strip_prefix("state").unwrap_or(line).trim();

    // Composite opener: `state X {` (optionally `state "Desc" as X {`).
    if let Some(head) = body.strip_suffix('{') {
        let head = head.trim();
        let (id, label) = parse_state_id_alias(head);
        if id.is_empty() {
            return;
        }
        let i = ensure_state(&id, false, diag, index_of, parents.last().copied());
        diag.states[i].composite = true;
        if !label.is_empty() {
            diag.states[i].label = label;
        }
        parents.push(i);
        return;
    }

    // Marker: `state id <<fork|join|choice>>`.
    if let Some(open) = body.find("<<") {
        if let Some(close) = body[open..].find(">>") {
            let id = body[..open].trim();
            let marker = body[open + 2..open + close].trim();
            let kind = match marker {
                "fork" => StateKind::Fork,
                "join" => StateKind::Join,
                "choice" => StateKind::Choice,
                _ => StateKind::Normal,
            };
            if !id.is_empty() && kind != StateKind::Normal {
                let i = ensure_state(id, false, diag, index_of, parents.last().copied());
                diag.states[i].kind = kind;
                diag.states[i].label = String::new();
                return;
            }
        }
    }

    // Alias / description / bare id.
    let (id, label) = parse_state_id_alias(body);
    if id.is_empty() {
        return;
    }
    let (id, css) = peel_css(&id);
    let i = ensure_state(&id, false, diag, index_of, parents.last().copied());
    if let Some(css) = css {
        directives.add_shorthand(&id, &css);
    }
    if !label.is_empty() {
        diag.states[i].label = label;
    }
}

/// Parse a `state` head into `(id, label)`. Forms:
/// * `"Long description" as id` → (`id`, `Long description`).
/// * `id : description` → (`id`, `description`).
/// * `id` → (`id`, `""`).
fn parse_state_id_alias(head: &str) -> (String, String) {
    let head = head.trim();
    // Quoted-alias form: `"desc" as id`.
    if let Some(rest) = head.strip_prefix('"') {
        if let Some(close) = rest.find('"') {
            let desc = &rest[..close];
            let after = rest[close + 1..].trim();
            if let Some(id) = after.strip_prefix("as") {
                return (id.trim().to_string(), desc.to_string());
            }
        }
    }
    // `id : description`.
    if let Some((id, desc)) = head.split_once(':') {
        return (id.trim().to_string(), desc.trim().to_string());
    }
    // Bare `id` (possibly `id as …` without quotes — keep id only).
    let id = head.split_whitespace().next().unwrap_or("").to_string();
    (id, String::new())
}

/// Parse one body line: either a transition (`a --> b [: label]`) or a state
/// declaration / description (`s1` or `s1 : text`).
fn parse_line(
    line: &str,
    diag: &mut StateDiagram,
    directives: &mut Directives,
    index_of: &mut HashMap<String, usize>,
    parents: &[usize],
) {
    let parent = parents.last().copied();
    if let Some(idx) = line.find("-->") {
        let from_raw = line[..idx].trim();
        let rest = line[idx + 3..].trim();
        // The target may carry a `:::class` shorthand and/or a `: label`. Peel
        // the `:::class` first (it binds to the target id), then split a label.
        let (from_raw, from_css) = peel_css(from_raw);
        let (to_part, to_css) = peel_css(rest);
        let (to_raw, label) = match to_part.split_once(':') {
            Some((t, l)) => (t.trim().to_string(), Some(l.trim().to_string())),
            None => (to_part, None),
        };
        if from_raw.is_empty() || to_raw.is_empty() {
            return;
        }
        let from_i = ensure_state(&from_raw, true, diag, index_of, parent);
        let to_i = ensure_state(&to_raw, false, diag, index_of, parent);
        let from = diag.states[from_i].id.clone();
        let to = diag.states[to_i].id.clone();
        if let Some(css) = from_css {
            directives.add_shorthand(&from, &css);
        }
        if let Some(css) = to_css {
            directives.add_shorthand(&to, &css);
        }
        diag.transitions.push(Transition {
            from,
            to,
            label: label.filter(|l| !l.is_empty()),
        });
        return;
    }

    // State declaration or description: `s1` or `s1 : description`. A `:::class`
    // shorthand (`s1:::hot`) is peeled off first.
    let (line, css) = peel_css(line);
    if let Some((id_raw, desc)) = line.split_once(':') {
        let id_raw = id_raw.trim();
        if id_raw.is_empty() {
            return;
        }
        let i = ensure_state(id_raw, false, diag, index_of, parent);
        if let Some(css) = css {
            let id = diag.states[i].id.clone();
            directives.add_shorthand(&id, &css);
        }
        let desc = desc.trim();
        if !desc.is_empty() {
            diag.states[i].label = desc.to_string();
        }
    } else {
        // Bare state id.
        let i = ensure_state(&line, false, diag, index_of, parent);
        if let Some(css) = css {
            let id = diag.states[i].id.clone();
            directives.add_shorthand(&id, &css);
        }
    }
}

/// Peel a trailing `:::className` off a state token, returning the bare token
/// and the class name (if any). The bare token is trimmed.
fn peel_css(tok: &str) -> (String, Option<String>) {
    match split_css_class(tok) {
        Some((base, css)) => (base, Some(css)),
        None => (tok.trim().to_string(), None),
    }
}

/// Upsert a state by its raw token. `[*]` maps to the shared start pseudo-state
/// when `as_source` is true, otherwise the shared end pseudo-state. Returns the
/// canonical id used in the graph.
fn ensure_state(
    raw: &str,
    as_source: bool,
    diag: &mut StateDiagram,
    index_of: &mut HashMap<String, usize>,
    parent: Option<usize>,
) -> usize {
    // A `[*]` inside a composite is that composite's *own* start/end: give it a
    // distinct synthetic id per parent so each composite gets its own pair.
    let (id, label, pseudo) = if raw == "[*]" {
        let suffix = parent.map(|p| format!("{p}")).unwrap_or_default();
        if as_source {
            (format!("{START_ID}{suffix}"), String::new(), Some(Pseudo::Start))
        } else {
            (format!("{END_ID}{suffix}"), String::new(), Some(Pseudo::End))
        }
    } else {
        (raw.to_string(), raw.to_string(), None)
    };

    if let Some(&i) = index_of.get(&id) {
        // Set parent on first real placement if not yet known (e.g. the state
        // was first seen as a transition endpoint at top level).
        if diag.states[i].parent.is_none() && parent.is_some() {
            diag.states[i].parent = parent;
        }
        return i;
    }
    let i = diag.states.len();
    index_of.insert(id.clone(), i);
    diag.states.push(State {
        id,
        label,
        pseudo,
        kind: StateKind::Normal,
        parent,
        composite: false,
        style: ElemStyle::default(),
    });
    i
}

/// A representative descendant node index for composite `i`, used to redirect
/// edges that target the composite (the layout engine can't rank a container as
/// an edge endpoint). Prefers `i`'s own start pseudo-state, then its first
/// non-composite child, then its first child; falls back to `i` itself.
fn composite_rep(diag: &StateDiagram, i: u32) -> u32 {
    if !diag.states[i as usize].composite {
        return i;
    }
    let children: Vec<u32> = diag
        .states
        .iter()
        .enumerate()
        .filter(|(_, s)| s.parent == Some(i as usize))
        .map(|(k, _)| k as u32)
        .collect();
    // Own start pseudo-state first.
    if let Some(&c) = children
        .iter()
        .find(|&&c| diag.states[c as usize].pseudo == Some(Pseudo::Start))
    {
        return resolve_rep(diag, c);
    }
    if let Some(&c) = children
        .iter()
        .find(|&&c| !diag.states[c as usize].composite)
    {
        return resolve_rep(diag, c);
    }
    if let Some(&c) = children.first() {
        return resolve_rep(diag, c);
    }
    i
}

/// Resolve a child to a concrete (non-composite) node, recursing into nested
/// composites so the redirected endpoint is always rankable.
fn resolve_rep(diag: &StateDiagram, c: u32) -> u32 {
    if diag.states[c as usize].composite {
        composite_rep(diag, c)
    } else {
        c
    }
}

/// Split a `name:::className [rest]` token into `(name + rest, className)`; the
/// className runs to the next whitespace and is removed from the token. `None`
/// when there is no `:::` shorthand.
fn split_css_class(tok: &str) -> Option<(String, String)> {
    let (name, after) = tok.split_once(":::")?;
    // className is the first whitespace-delimited token after `:::`; any
    // remainder (e.g. ` : label`) is preserved on the name side.
    let after = after.trim_start();
    let (css, rest) = match after.find(char::is_whitespace) {
        Some(i) => (&after[..i], after[i..].trim_start()),
        None => (after, ""),
    };
    if css.is_empty() {
        return None;
    }
    let mut bare = name.trim().to_string();
    if !rest.is_empty() {
        bare.push(' ');
        bare.push_str(rest);
    }
    Some((bare, css.to_string()))
}

/// Render a mermaid `state` diagram to SVG.
pub fn render_state(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diag = parse(src).map_err(MermaidError::Parse)?;
    if diag.states.is_empty() {
        return Err(MermaidError::Empty);
    }

    // id → node index, first-seen order (matches dagre node indices).
    let index_of: HashMap<&str, u32> = diag
        .states
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i as u32))
        .collect();

    // Node sizes: real states from text + padding, pseudo-states fixed, special
    // markers fixed (bar / diamond), composites are containers sized (0,0) by
    // the engine from their members.
    let sizes: Vec<(f32, f32)> = diag
        .states
        .iter()
        .map(|s| {
            if s.composite {
                return (0.0, 0.0);
            }
            match s.kind {
                StateKind::Fork | StateKind::Join => (FORK_LEN, FORK_THICK),
                StateKind::Choice => (CHOICE_SIZE, CHOICE_SIZE),
                StateKind::Normal => match s.pseudo {
                    Some(_) => (PSEUDO_SIZE, PSEUDO_SIZE),
                    None => {
                        // Rich-aware so a state name with markdown/math is sized
                        // to its rendered width (== text_size for plain labels).
                        let (tw, th) = crate::label::measure(&s.label, opts.font_size_px);
                        (tw + 2.0 * opts.node_padding_x, th + 2.0 * opts.node_padding_y)
                    }
                },
            }
        })
        .collect();

    // The layout engine cannot rank a container (composite) node as an edge
    // endpoint, so an edge touching a composite is redirected to a representative
    // descendant: its own start pseudo-state if present, else its first child.
    // (`rep[i]` is the node index to use when an edge references composite `i`.)
    let rep: Vec<u32> = (0..diag.states.len() as u32)
        .map(|i| composite_rep(&diag, i))
        .collect();
    let remap = |i: u32| -> u32 {
        if diag.states[i as usize].composite {
            rep[i as usize]
        } else {
            i
        }
    };

    // Build the dagre edge list; keep the mapping back to original transitions.
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(diag.transitions.len());
    let mut kept: Vec<usize> = Vec::with_capacity(diag.transitions.len());
    // Per-edge label box size (aligned to `edges`) so dagre reserves a gap and
    // positions the label there; None for unlabeled transitions.
    let mut label_sizes: Vec<Option<Vec2>> = Vec::with_capacity(diag.transitions.len());
    for (j, t) in diag.transitions.iter().enumerate() {
        if let (Some(&a), Some(&b)) =
            (index_of.get(t.from.as_str()), index_of.get(t.to.as_str()))
        {
            let (a, b) = (remap(a), remap(b));
            if a == b {
                // Self-edge after redirection (e.g. a composite with one child):
                // skip to avoid a degenerate route.
                continue;
            }
            edges.push((a, b));
            kept.push(j);
            label_sizes.push(
                t.label
                    .as_deref()
                    .filter(|l| !l.is_empty())
                    .map(|l| {
                        let (w, h) = crate::label::measure(l, opts.font_size_px);
                        Vec2::new(w + 10.0, h + 6.0)
                    }),
            );
        }
    }

    let node_sizes: Vec<Vec2> = sizes.iter().map(|&(w, h)| Vec2::new(w, h)).collect();

    // Cluster wiring for composite states: a child node's `node_parents[i]` is
    // the dagre index of the composite that directly contains it. Built only
    // when there is at least one composite, so simple diagrams pass `None` and
    // keep the byte-for-byte-unchanged no-composite path.
    let has_composite = diag.states.iter().any(|s| s.composite);
    let node_parents: Option<Vec<Option<usize>>> = if has_composite {
        Some(diag.states.iter().map(|s| s.parent).collect())
    } else {
        None
    };

    let engine = LayeredEngine {
        rankdir: RankDir::Tb,
        ranksep: opts.rank_sep,
        nodesep: opts.node_sep,
        edgesep: 20.0,
        default_node_size: Vec2::new(50.0, 50.0),
    };
    let out = engine.layout(&GraphInput {
        node_count: diag.states.len(),
        edges: &edges,
        node_sizes: Some(&node_sizes),
        edge_label_sizes: Some(&label_sizes),
        node_parents: node_parents.as_deref(),
        directed: true,
    });

    let width = (out.size.x.ceil() + 1.0).max(1.0);
    let height = (out.size.y.ceil() + 1.0).max(1.0);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\">"
    );
    emit_defs(&mut svg, opts);

    // Composite boundary boxes first (behind everything), largest-first so a
    // nested composite layers on top of its enclosing one. The container rect is
    // read back from the engine: center = positions[i], size = node_sizes[i].
    let mut composites: Vec<usize> = diag
        .states
        .iter()
        .enumerate()
        .filter(|(_, s)| s.composite)
        .map(|(i, _)| i)
        .collect();
    composites.sort_by(|&a, &b| {
        let area = |i: usize| {
            out.node_sizes
                .get(i)
                .map(|v| v.x * v.y)
                .unwrap_or(0.0)
        };
        area(b).total_cmp(&area(a))
    });
    for &i in &composites {
        let center = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
        let size = out.node_sizes.get(i).copied().unwrap_or(Vec2::ZERO);
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        emit_composite(&mut svg, &diag.states[i], center, size, opts);
    }

    // Group edges by unordered endpoint pair so bidirectional / parallel
    // transitions spread their labels instead of stacking at one midpoint.
    let mut pair_members: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (k, &(a, b)) in edges.iter().enumerate() {
        pair_members.entry((a.min(b), a.max(b))).or_default().push(k);
    }
    let mut group = vec![(0usize, 1usize); edges.len()];
    for members in pair_members.values() {
        let cnt = members.len();
        for (idx, &k) in members.iter().enumerate() {
            group[k] = (idx, cnt);
        }
    }

    // Edges first so node fills paint over the line ends.
    for (dagre_idx, &orig) in kept.iter().enumerate() {
        let route = out.edge_routes.get(dagre_idx);
        let pts: Vec<(f32, f32)> = route
            .map(|r| r.iter().map(|p| (p.x, p.y)).collect())
            .unwrap_or_default();
        let (idx, cnt) = group[dagre_idx];
        // Dagre's reserved label center, if it placed one for this edge.
        let dagre_label = out.edge_label_positions.get(dagre_idx).copied().flatten();
        emit_edge(
            &mut svg,
            &pts,
            diag.transitions[orig].label.as_deref(),
            idx,
            cnt,
            dagre_label,
            opts,
        );
    }

    // Nodes (composites already drawn as boundary boxes above).
    for (i, s) in diag.states.iter().enumerate() {
        if s.composite {
            continue;
        }
        let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
        let (w, h) = sizes[i];
        emit_node(&mut svg, s, pos.x, pos.y, w, h, opts);
    }

    // Notes: not part of the dagre graph — placed beside their target's final
    // position. Drawn last so they overlay.
    let pos_of = |id: &str| -> Option<(Vec2, (f32, f32))> {
        let &n = index_of.get(id)?;
        let i = n as usize;
        let center = out.positions.get(i).copied()?;
        Some((center, sizes[i]))
    };
    for note in &diag.notes {
        if let Some((center, (w, h))) = pos_of(&note.target) {
            emit_note(&mut svg, note, center, w, h, opts);
        }
    }

    svg.push_str("</svg>");

    Ok(MermaidRender {
        svg,
        width_px: width,
        height_px: height,
    })
}

/// Arrowhead marker shared by every transition.
fn emit_defs(svg: &mut String, opts: &MermaidOptions) {
    let len = 9.0_f32;
    let half = 4.0_f32;
    let _ = write!(
        svg,
        "<defs><marker id=\"state-arrow\" markerWidth=\"{len}\" markerHeight=\"{w}\" \
         refX=\"{len}\" refY=\"{half}\" orient=\"auto\" markerUnits=\"userSpaceOnUse\">\
         <path d=\"M0,0 L{len},{half} L0,{w} Z\" fill=\"{fill}\"{fo}/></marker></defs>",
        w = half * 2.0,
        fill = rgb(opts.edge_stroke),
        fo = opacity_attr("fill-opacity", opts.edge_stroke),
    );
}

/// One transition polyline (with an arrowhead and optional centered label).
/// `index`/`count` give the edge's position within its parallel group so the
/// label is nudged perpendicular to the route for bidirectional/parallel pairs.
#[allow(clippy::too_many_arguments)]
fn emit_edge(
    svg: &mut String,
    points: &[(f32, f32)],
    label: Option<&str>,
    index: usize,
    count: usize,
    dagre_label: Option<Vec2>,
    opts: &MermaidOptions,
) {
    if points.len() < 2 {
        return;
    }
    let mut pts = points.to_vec();
    pullback_end(&mut pts, 9.0);

    let mut d = String::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        let cmd = if i == 0 { 'M' } else { 'L' };
        let _ = write!(d, "{cmd}{x:.2},{y:.2} ");
    }
    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\"{so} \
         marker-end=\"url(#state-arrow)\"/>",
        d.trim_end(),
        stroke = rgb(opts.edge_stroke),
        so = opacity_attr("stroke-opacity", opts.edge_stroke),
    );

    if let Some(label) = label.filter(|l| !l.is_empty()) {
        // Prefer dagre's reserved label center; fall back to the
        // perpendicular-nudged midpoint when dagre didn't place it.
        let anchor = match dagre_label {
            Some(p) => Some((p.x, p.y)),
            None => edge_label_anchor(points, index, count, opts.font_size_px),
        };
        if let Some((cx, cy)) = anchor {
            emit_label(svg, label, cx, cy, opts);
        }
    }
}

/// A state node: pseudo-states are circles, real states are rounded rects.
fn emit_node(
    svg: &mut String,
    s: &State,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    opts: &MermaidOptions,
) {
    // Special-marker shapes take precedence over the rounded-rect path.
    match s.kind {
        StateKind::Fork | StateKind::Join => {
            // A thin filled bar.
            let x = cx - w / 2.0;
            let y = cy - h / 2.0;
            let fill_c = s.style.fill.unwrap_or(opts.node_stroke);
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
                 rx=\"2\" ry=\"2\" fill=\"{fill}\"{fo}/>",
                fill = rgb(fill_c),
                fo = opacity_attr("fill-opacity", fill_c),
            );
            return;
        }
        StateKind::Choice => {
            // A small diamond (rotated square) centered on the node.
            let r = w / 2.0;
            let fill_c = s.style.fill.unwrap_or(opts.node_fill);
            let stroke_c = s.style.stroke.unwrap_or(opts.node_stroke);
            let sw = s.style.stroke_width.unwrap_or(1.5);
            let _ = write!(
                svg,
                "<polygon points=\"{x0:.2},{cy:.2} {cx:.2},{y0:.2} {x1:.2},{cy:.2} {cx:.2},{y1:.2}\" \
                 fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
                x0 = cx - r,
                x1 = cx + r,
                y0 = cy - r,
                y1 = cy + r,
                fill = rgb(fill_c),
                fo = opacity_attr("fill-opacity", fill_c),
                stroke = rgb(stroke_c),
                so = opacity_attr("stroke-opacity", stroke_c),
            );
            return;
        }
        StateKind::Normal => {}
    }
    match s.pseudo {
        Some(Pseudo::Start) => {
            // Small solid filled circle.
            let _ = write!(
                svg,
                "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" fill=\"{fill}\"{fo}/>",
                r = w / 2.0,
                fill = rgb(opts.node_stroke),
                fo = opacity_attr("fill-opacity", opts.node_stroke),
            );
        }
        Some(Pseudo::End) => {
            // Outer ring + inner solid circle.
            let _ = write!(
                svg,
                "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" fill=\"none\" \
                 stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>\
                 <circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{ri:.2}\" fill=\"{fill}\"{fo}/>",
                r = w / 2.0,
                ri = (w / 2.0 - 4.0).max(1.0),
                stroke = rgb(opts.node_stroke),
                so = opacity_attr("stroke-opacity", opts.node_stroke),
                fill = rgb(opts.node_stroke),
                fo = opacity_attr("fill-opacity", opts.node_stroke),
            );
        }
        None => {
            let x = cx - w / 2.0;
            let y = cy - h / 2.0;
            // Per-state style overrides, falling back to theme defaults.
            let fill_c = s.style.fill.unwrap_or(opts.node_fill);
            let stroke_c = s.style.stroke.unwrap_or(opts.node_stroke);
            let sw = s.style.stroke_width.unwrap_or(1.5);
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
                 rx=\"6\" ry=\"6\" fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
                fill = rgb(fill_c),
                fo = opacity_attr("fill-opacity", fill_c),
                stroke = rgb(stroke_c),
                so = opacity_attr("stroke-opacity", stroke_c),
            );
            let text_c = s.style.text_color.unwrap_or(opts.text_color);
            emit_label_colored(svg, &s.label, cx, cy, opts, text_c);
        }
    }
}

/// A composite (nested) state: a labeled rounded boundary box with a title at
/// the top-left and a faint themed fill, enclosing its laid-out children.
/// `center`/`size` are the container rect read back from the layout engine.
fn emit_composite(
    svg: &mut String,
    s: &State,
    center: Vec2,
    size: Vec2,
    opts: &MermaidOptions,
) {
    // Reserve a strip at the top for the title.
    let fs = opts.font_size_px;
    let title_h = fs + 6.0;
    let x = center.x - size.x / 2.0;
    let y = center.y - size.y / 2.0 - title_h;
    let w = size.x;
    let h = size.y + title_h;

    // Faint tint of the node fill so the box reads as a grouping.
    let mut fill = s.style.fill.unwrap_or(opts.node_fill);
    fill[3] = 51; // ~0.2 opacity
    let stroke_c = s.style.stroke.unwrap_or(opts.node_stroke);
    let sw = s.style.stroke_width.unwrap_or(1.5);
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         rx=\"6\" ry=\"6\" fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
        fill = rgb(fill),
        fo = opacity_attr("fill-opacity", fill),
        stroke = rgb(stroke_c),
        so = opacity_attr("stroke-opacity", stroke_c),
    );
    // Separator line under the title.
    let sep_y = y + title_h;
    let _ = write!(
        svg,
        "<line x1=\"{x:.2}\" y1=\"{sep_y:.2}\" x2=\"{x2:.2}\" y2=\"{sep_y:.2}\" \
         stroke=\"{stroke}\"{so} stroke-width=\"1\"/>",
        x2 = x + w,
        stroke = rgb(stroke_c),
        so = opacity_attr("stroke-opacity", stroke_c),
    );

    let title = s.label.trim();
    if !title.is_empty() {
        let text_c = s.style.text_color.unwrap_or(opts.text_color);
        let tx = x + 8.0;
        let ty = y + title_h / 2.0;
        let _ = write!(
            svg,
            "<text x=\"{tx:.2}\" y=\"{ty:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}>{txt}</text>",
            family = escape(&opts.font_family),
            fill = rgb(text_c),
            fo = opacity_attr("fill-opacity", text_c),
            txt = escape(title),
        );
    }
}

/// A note: a pale-filled rectangle placed beside (`pos`) its target state, with
/// the (possibly multi-line) note text. `(cx, cy)`/`(tw, th)` describe the
/// target's final box.
fn emit_note(
    svg: &mut String,
    note: &Note,
    target_center: Vec2,
    tw: f32,
    th: f32,
    opts: &MermaidOptions,
) {
    let fs = opts.font_size_px;
    let lines: Vec<&str> = note.text.lines().collect();
    let line_count = lines.len().max(1) as f32;
    let max_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0).max(1);
    let nw = (max_chars as f32 * fs * 0.6 + 12.0).max(40.0);
    let nh = line_count * (fs + 4.0) + 8.0;

    let gap = 12.0;
    let (nx, ny) = match note.pos {
        NotePos::Left => (target_center.x - tw / 2.0 - gap - nw, target_center.y - nh / 2.0),
        NotePos::Right => (target_center.x + tw / 2.0 + gap, target_center.y - nh / 2.0),
        NotePos::Over => (target_center.x - nw / 2.0, target_center.y - th / 2.0 - gap - nh),
    };

    // Pale yellow note fill (mermaid's note color), themed stroke.
    let fill = [255u8, 245, 181, 255];
    let stroke = [170u8, 170, 51, 255];
    let _ = write!(
        svg,
        "<rect x=\"{nx:.2}\" y=\"{ny:.2}\" width=\"{nw:.2}\" height=\"{nh:.2}\" \
         rx=\"2\" ry=\"2\" fill=\"{f}\" stroke=\"{s}\" stroke-width=\"1\"/>",
        f = rgb(fill),
        s = rgb(stroke),
    );
    let cx = nx + nw / 2.0;
    let line_h = fs + 4.0;
    let mut ty = ny + 4.0 + line_h / 2.0;
    for line in &lines {
        emit_label_colored(svg, line, cx, ty, opts, opts.text_color);
        ty += line_h;
    }
}

/// Centered `<text>` (single line) at `(cx, cy)` in the theme text color.
fn emit_label(svg: &mut String, label: &str, cx: f32, cy: f32, opts: &MermaidOptions) {
    emit_label_colored(svg, label, cx, cy, opts, opts.text_color);
}

/// Centered label (single string) at `(cx, cy)` in the given color, routed
/// through the rich-label renderer so state names, transition labels, and note
/// lines support markdown (`**bold**`/`*italic*`/`<br>`) and inline math
/// (`$…$`). Plain labels emit a single centered `<text>` identical to before.
fn emit_label_colored(
    svg: &mut String,
    label: &str,
    cx: f32,
    cy: f32,
    opts: &MermaidOptions,
    color: [u8; 4],
) {
    crate::label::emit(
        svg,
        label,
        cx,
        cy,
        crate::label::Anchor::Middle,
        opts.font_size_px,
        color,
        &opts.font_family,
    );
}

/// Shorten the polyline's last segment by `amount` px so an arrowhead tip lands
/// on the target border.
fn pullback_end(pts: &mut [(f32, f32)], amount: f32) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let (tx, ty) = pts[n - 1];
    let (px, py) = pts[n - 2];
    let (dx, dy) = (tx - px, ty - py);
    let len = dx.hypot(dy);
    if len <= amount || len == 0.0 {
        return;
    }
    let t = (len - amount) / len;
    pts[n - 1] = (px + dx * t, py + dy * t);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    #[test]
    fn parse_states_and_transitions() {
        let src = "stateDiagram-v2\n  s1 --> s2\n  s2 --> s3 : go";
        let d = parse(src).unwrap();
        assert_eq!(d.states.len(), 3);
        assert_eq!(d.states[0].id, "s1");
        assert_eq!(d.transitions.len(), 2);
        assert_eq!(d.transitions[1].label.as_deref(), Some("go"));
    }

    #[test]
    fn parse_description() {
        let src = "stateDiagram\n  s1 : First state\n  s1 --> s2";
        let d = parse(src).unwrap();
        // s1 created by the description, label set to the text.
        assert_eq!(d.states[0].id, "s1");
        assert_eq!(d.states[0].label, "First state");
    }

    #[test]
    fn start_and_end_pseudo_states() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> [*]";
        let d = parse(src).unwrap();
        // start, s1, end → three nodes.
        assert_eq!(d.states.len(), 3);
        assert_eq!(d.states[0].pseudo, Some(Pseudo::Start));
        assert_eq!(d.states[1].id, "s1");
        assert_eq!(d.states[2].pseudo, Some(Pseudo::End));
    }

    #[test]
    fn bad_header_errors() {
        assert!(parse("graph TD\n a --> b").is_err());
    }

    #[test]
    fn empty_input_errors() {
        // No header at all.
        assert!(parse("\n\n").is_err());
    }

    #[test]
    fn render_wellformed_svg() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> s2 : next\n  s2 --> [*]";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_node_and_edge_counts() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> s2\n  s2 --> [*]";
        let r = render_state(src, &opts()).unwrap();
        // Two real states → two <rect>.
        assert_eq!(r.svg.matches("<rect").count(), 2);
        // Three transitions → three edge paths (each references the arrow marker).
        assert_eq!(r.svg.matches("marker-end=\"url(#state-arrow)\"").count(), 3);
    }

    #[test]
    fn start_and_end_markers_drawn() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> [*]";
        let r = render_state(src, &opts()).unwrap();
        // Start = 1 circle, end = 2 circles → 3 <circle> total.
        assert_eq!(r.svg.matches("<circle").count(), 3);
    }

    #[test]
    fn edge_label_rendered() {
        let src = "stateDiagram-v2\n  s1 --> s2 : hello";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains(">hello<"));
    }

    #[test]
    fn xml_escapes_label() {
        let src = "stateDiagram-v2\n  s1 : a & b < c\n  s1 --> s2";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains("a &amp; b &lt; c"));
        assert!(!r.svg.contains("a & b"));
    }

    #[test]
    fn empty_diagram_errors() {
        // Header only, no states.
        assert_eq!(render_state("stateDiagram-v2\n", &opts()), Err(MermaidError::Empty));
    }

    #[test]
    fn bidirectional_labels_separated() {
        // Idle<->Running with both directions labeled: the two labels must not
        // overlap (the "stostart" bug). Both texts render, at distinct y.
        let src = "stateDiagram-v2\n  Idle --> Running : start\n  Running --> Idle : stop";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains(">start<"));
        assert!(r.svg.contains(">stop<"));

        // Read the (x, y) anchor of each label's <text> element; the two must
        // differ in at least one coordinate (perpendicular nudge).
        fn label_xy(svg: &str, text: &str) -> (f32, f32) {
            let needle = format!(">{text}<");
            let at = svg.find(&needle).expect("label text present");
            let tag_start = svg[..at].rfind("<text").expect("text tag");
            let tag = &svg[tag_start..at];
            let attr = |name: &str| {
                let k = tag.find(name).expect("attr") + name.len();
                let end = tag[k..].find('"').unwrap() + k;
                tag[k..end].parse::<f32>().unwrap()
            };
            (attr("x=\""), attr("y=\""))
        }
        let s = label_xy(&r.svg, "start");
        let t = label_xy(&r.svg, "stop");
        assert!(
            (s.0 - t.0).abs() > 1.0 || (s.1 - t.1).abs() > 1.0,
            "bidirectional labels overlap: start={s:?}, stop={t:?}"
        );
    }

    #[test]
    fn deterministic() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> s2 : x\n  s2 --> [*]";
        let a = render_state(src, &opts()).unwrap();
        let b = render_state(src, &opts()).unwrap();
        assert_eq!(a, b);
    }

    // ---- styling directives ----

    fn style_of<'a>(d: &'a StateDiagram, id: &str) -> &'a ElemStyle {
        &d.states.iter().find(|s| s.id == id).expect("state").style
    }

    #[test]
    fn classdef_and_class_apply() {
        let src = "stateDiagram-v2\n  Running --> Idle\n  classDef hl fill:#ff0\n  class Running hl";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Running").fill, Some([255, 255, 0, 255]));
        // Idle untouched.
        assert_eq!(style_of(&d, "Idle").fill, None);
    }

    #[test]
    fn triple_colon_shorthand() {
        let src = "stateDiagram-v2\n  Running:::hl --> Idle\n  classDef hl fill:#ff0";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Running").fill, Some([255, 255, 0, 255]));
        // Transition recorded with bare ids.
        assert_eq!(d.transitions[0].from, "Running");
        assert_eq!(d.transitions[0].to, "Idle");
    }

    #[test]
    fn triple_colon_with_label() {
        // `:::class` on the target plus a `: label` after it.
        let src = "stateDiagram-v2\n  Idle --> Running:::hl : go\n  classDef hl fill:#ff0";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Running").fill, Some([255, 255, 0, 255]));
        assert_eq!(d.transitions[0].to, "Running");
        assert_eq!(d.transitions[0].label.as_deref(), Some("go"));
    }

    #[test]
    fn style_directive_overrides_class() {
        let src = "stateDiagram-v2\n  Running --> Idle\n  classDef hl fill:#ff0\n  class Running hl\n  style Running fill:#00f";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Running").fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn style_override_in_rendered_svg() {
        let src = "stateDiagram-v2\n  Running --> Idle\n  classDef hl fill:#ff0\n  class Running hl";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains(&rgb([255, 255, 0, 255])), "override fill present: {}", r.svg);
    }

    #[test]
    fn unstyled_states_unchanged() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> s2 : x\n  s2 --> [*]";
        let d = parse(src).unwrap();
        for s in &d.states {
            assert_eq!(s.style, ElemStyle::default());
        }
    }

    // ---- composite / nested states ----

    #[test]
    fn parse_composite_nesting() {
        let src = "stateDiagram-v2\n state Active {\n  [*] --> Running\n  Running --> Idle\n }\n [*] --> Active";
        let d = parse(src).unwrap();
        // Active is a composite.
        let active = d.states.iter().find(|s| s.id == "Active").expect("Active");
        assert!(active.composite, "Active should be a composite");
        let active_i = d.states.iter().position(|s| s.id == "Active").unwrap();
        // Running and Idle are children of Active.
        let running = d.states.iter().find(|s| s.id == "Running").expect("Running");
        let idle = d.states.iter().find(|s| s.id == "Idle").expect("Idle");
        assert_eq!(running.parent, Some(active_i));
        assert_eq!(idle.parent, Some(active_i));
        // Active itself is top-level.
        assert_eq!(active.parent, None);
        // Transition Running --> Idle recorded.
        assert!(d
            .transitions
            .iter()
            .any(|t| t.from == "Running" && t.to == "Idle"));
    }

    #[test]
    fn composite_box_encloses_children() {
        let src = "stateDiagram-v2\n state Active {\n  [*] --> Running\n  Running --> Idle\n }\n [*] --> Active";
        let r = render_state(src, &opts()).unwrap();
        // The composite is drawn as a boundary box with its title.
        assert!(r.svg.contains(">Active<"), "composite title present");
        // Children render as their own rects.
        assert!(r.svg.contains(">Running<"));
        assert!(r.svg.contains(">Idle<"));
        // A separator <line> is part of the composite chrome.
        assert!(r.svg.contains("<line"), "composite separator line present");
        assert!(r.svg.starts_with("<svg") && r.svg.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn composite_box_geometrically_encloses_children() {
        let src = "stateDiagram-v2\n state Active {\n  [*] --> Running\n  Running --> Idle\n }\n [*] --> Active";
        let r = render_state(src, &opts()).unwrap();

        // Read attrs of the <rect> immediately preceding a given label text.
        fn rect_before(svg: &str, label: &str) -> (f32, f32, f32, f32) {
            let at = svg.find(&format!(">{label}<")).expect("label");
            let start = svg[..at].rfind("<rect").expect("rect");
            let rect = &svg[start..at];
            let attr = |name: &str| {
                let k = rect.find(name).unwrap() + name.len();
                let end = rect[k..].find('"').unwrap() + k;
                rect[k..end].parse::<f32>().unwrap()
            };
            (attr("x=\""), attr("y=\""), attr("width=\""), attr("height=\""))
        }

        let (bx, by, bw, bh) = rect_before(&r.svg, "Active");
        let (rx, ry, rw, rh) = rect_before(&r.svg, "Running");
        let (ix, iy, iw, ih) = rect_before(&r.svg, "Idle");

        // The composite box must enclose both children's rects.
        for (x, y, w, h) in [(rx, ry, rw, rh), (ix, iy, iw, ih)] {
            assert!(bx <= x + 0.5, "composite left {bx} <= child left {x}");
            assert!(by <= y + 0.5, "composite top {by} <= child top {y}");
            assert!(bx + bw >= x + w - 0.5, "composite right encloses child right");
            assert!(by + bh >= y + h - 0.5, "composite bottom encloses child bottom");
        }
    }

    // ---- fork / join / choice ----

    #[test]
    fn parse_fork_join_choice() {
        let src = "stateDiagram-v2\n state f <<fork>>\n state j <<join>>\n state c <<choice>>";
        let d = parse(src).unwrap();
        assert_eq!(d.states.iter().find(|s| s.id == "f").unwrap().kind, StateKind::Fork);
        assert_eq!(d.states.iter().find(|s| s.id == "j").unwrap().kind, StateKind::Join);
        assert_eq!(d.states.iter().find(|s| s.id == "c").unwrap().kind, StateKind::Choice);
    }

    #[test]
    fn fork_join_render_bars() {
        let src = "stateDiagram-v2\n state f <<fork>>\n [*] --> f\n f --> A\n f --> B";
        let r = render_state(src, &opts()).unwrap();
        // The fork bar is a thin rect of fixed bar size.
        assert!(
            r.svg.contains(&format!("width=\"{:.2}\"", FORK_LEN)),
            "fork bar width present"
        );
        // Two transitions split out of the fork.
        assert_eq!(r.svg.matches("marker-end=\"url(#state-arrow)\"").count(), 3);
    }

    #[test]
    fn join_render_bar() {
        let src = "stateDiagram-v2\n state j <<join>>\n A --> j\n B --> j\n j --> [*]";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains(&format!("width=\"{:.2}\"", FORK_LEN)));
    }

    #[test]
    fn choice_render_diamond() {
        let src = "stateDiagram-v2\n state c <<choice>>\n [*] --> c\n c --> A\n c --> B";
        let r = render_state(src, &opts()).unwrap();
        // Choice renders as a polygon (diamond).
        assert!(r.svg.contains("<polygon"), "choice diamond present");
    }

    // ---- notes ----

    #[test]
    fn parse_note_inline() {
        let src = "stateDiagram-v2\n A --> B\n note right of A: hello";
        let d = parse(src).unwrap();
        assert_eq!(d.notes.len(), 1);
        assert_eq!(d.notes[0].target, "A");
        assert_eq!(d.notes[0].pos, NotePos::Right);
        assert_eq!(d.notes[0].text, "hello");
    }

    #[test]
    fn note_renders_rect_and_text() {
        let src = "stateDiagram-v2\n A --> B\n note right of A: hello";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains(">hello<"), "note text present");
        // The pale note fill color is present.
        assert!(r.svg.contains(&rgb([255, 245, 181, 255])), "note fill present");
    }

    #[test]
    fn parse_note_block_multiline() {
        let src =
            "stateDiagram-v2\n A --> B\n note left of A\n  line one\n  line two\n end note";
        let d = parse(src).unwrap();
        assert_eq!(d.notes.len(), 1);
        assert_eq!(d.notes[0].pos, NotePos::Left);
        assert_eq!(d.notes[0].text, "line one\nline two");
    }

    #[test]
    fn note_over_parsed() {
        let src = "stateDiagram-v2\n A --> B\n note over A: spanning";
        let d = parse(src).unwrap();
        assert_eq!(d.notes[0].pos, NotePos::Over);
        assert_eq!(d.notes[0].target, "A");
    }

    // ---- alias / direction ----

    #[test]
    fn state_alias_form() {
        let src = "stateDiagram-v2\n state \"Long description\" as S\n [*] --> S";
        let d = parse(src).unwrap();
        let s = d.states.iter().find(|s| s.id == "S").expect("S");
        assert_eq!(s.label, "Long description");
    }

    #[test]
    fn direction_ignored() {
        let src = "stateDiagram-v2\n direction LR\n A --> B";
        let d = parse(src).unwrap();
        assert_eq!(d.transitions.len(), 1);
    }

    #[test]
    fn state_keyword_bare_decl() {
        // `state X` declares X; a later `X : desc` sets its label.
        let src = "stateDiagram-v2\n state Foo\n Foo --> Bar";
        let d = parse(src).unwrap();
        assert!(d.states.iter().any(|s| s.id == "Foo"));
        assert!(d.transitions.iter().any(|t| t.from == "Foo" && t.to == "Bar"));
    }

    #[test]
    fn simple_diagram_no_clusters_unchanged() {
        // A no-composite/no-special diagram must produce no composite chrome.
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> s2 : next\n  s2 --> [*]";
        let r = render_state(src, &opts()).unwrap();
        assert!(!r.svg.contains("<line"), "no composite separator in simple diagram");
        assert!(!r.svg.contains("<polygon"), "no diamonds in simple diagram");
        // Two real states → two <rect>.
        assert_eq!(r.svg.matches("<rect").count(), 2);
    }

    #[test]
    fn state_name_renders_inline_math() {
        // A state described with `$…$` renders the embedded math group.
        let src = "stateDiagram-v2\n  s1 : energy $x^2$\n  s1 --> s2";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains("<g transform"), "expected math group: {}", r.svg);
        assert!(r.svg.contains("<path"), "expected math path: {}", r.svg);
    }

    #[test]
    fn transition_label_renders_bold_markdown() {
        // A `**bold**` transition label renders a bold run, not literal `**`.
        let src = "stateDiagram-v2\n  s1 --> s2 : **go**";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains("font-weight=\"bold\""), "expected bold run: {}", r.svg);
        assert!(!r.svg.contains("**go**"), "raw markdown leaked: {}", r.svg);
    }

    #[test]
    fn composite_deterministic() {
        let src = "stateDiagram-v2\n state Active {\n  [*] --> Running\n  Running --> Idle\n }\n [*] --> Active";
        let a = render_state(src, &opts()).unwrap();
        let b = render_state(src, &opts()).unwrap();
        assert_eq!(a, b);
    }
}
