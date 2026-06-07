//! State-diagram parser: `stateDiagram[-v2]` source → [`super::model::StateDiagram`].
//! Handles state declarations and aliases, composite `{ … }` nesting, `[*]`
//! start/end pseudo-states, `<<fork>>`/`<<join>>`/`<<choice>>` markers, notes,
//! styling directives (`classDef`/`class`/`style`/`:::`), and `click`.

use std::collections::HashMap;

use crate::model::ElemStyle;

use super::model;

/// Synthetic id for the shared start pseudo-state.
const START_ID: &str = "\0start";
/// Synthetic id for the shared end pseudo-state.
const END_ID: &str = "\0end";

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
    fn resolve(&self, states: &mut [model::State]) {
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


/// Parse a state diagram source. Errors on a missing/wrong header.
pub(super) fn parse(src: &str) -> Result<model::StateDiagram, String> {
    // Header: first non-blank, non-comment line must be stateDiagram[-v2].
    let mut saw_header = false;
    let mut diag = model::StateDiagram::default();
    let mut directives = Directives::default();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    // Stack of currently-open composite container indices (innermost last).
    let mut parents: Vec<usize> = Vec::new();
    // Pending block note: `Some((target, pos, accumulated lines))` between a
    // `note … <S>` opener (no inline `:`) and its `end note`.
    let mut pending_note: Option<(String, model::NotePos, Vec<String>)> = None;

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
                diag.notes.push(model::Note {
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
                    Some(text) => diag.notes.push(model::Note { target, pos, text }),
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

        // Interaction directives: `click <id> ...` (same grammar as flowchart).
        // States are declared/inferred elsewhere; an unknown id is skipped, never
        // fabricated.
        if line.starts_with("click ") {
            let rest = line["click".len()..].trim_start();
            if let Some(c) = parse_click(rest) {
                apply_click(&mut diag, &index_of, &c);
            }
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
fn parse_note_header(line: &str) -> Option<(String, model::NotePos, Option<String>)> {
    // Split off the inline `: text` (a block note has none).
    let (head, inline) = match line.split_once(':') {
        Some((h, t)) => (h.trim(), Some(t.trim().to_string())),
        None => (line, None),
    };
    let rest = head.strip_prefix("note")?.trim_start();
    let (pos, after) = if let Some(a) = rest.strip_prefix("left of") {
        (model::NotePos::Left, a)
    } else if let Some(a) = rest.strip_prefix("right of") {
        (model::NotePos::Right, a)
    } else if let Some(a) = rest.strip_prefix("over") {
        (model::NotePos::Over, a)
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
    diag: &mut model::StateDiagram,
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
                "fork" => model::StateKind::Fork,
                "join" => model::StateKind::Join,
                "choice" => model::StateKind::Choice,
                _ => model::StateKind::Normal,
            };
            if !id.is_empty() && kind != model::StateKind::Normal {
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
    diag: &mut model::StateDiagram,
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
        diag.transitions.push(model::Transition {
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

/// Attach a parsed `click` directive's link/callback/tooltip to the named state
/// (looked up by canonical id). Unknown ids are skipped — states are declared or
/// inferred from transitions, so we never fabricate one here.
fn apply_click(diag: &mut model::StateDiagram, index_of: &HashMap<String, usize>, c: &ClickDirective) {
    let Some(&i) = index_of.get(&c.id) else {
        return;
    };
    let s = &mut diag.states[i];
    if c.link.is_some() {
        s.link = c.link.clone();
    }
    if c.callback.is_some() {
        s.callback = c.callback.clone();
    }
    if c.tooltip.is_some() {
        s.tooltip = c.tooltip.clone();
    }
}

// ── Click directives ──────────────────────────────────────────────────────────
//
// Same quote-aware grammar as the flowchart `click` parser (`parse.rs`):
// - `<id> "<url>" ["<tooltip>"]`             → link (+ tooltip)
// - `<id> href "<url>" ["<tooltip>"]`        → link (+ tooltip)
// - `<id> call <name>(<args>) ["<tooltip>"]` → callback = name (args dropped)
// - `<id> <name>` (bareword)                 → callback = the word

/// A parsed `click` directive body.
struct ClickDirective {
    id: String,
    link: Option<String>,
    callback: Option<String>,
    tooltip: Option<String>,
}

/// A token from a `click` directive body: a bare word or a double-quoted string.
enum ClickTok {
    Word(String),
    Quoted(String),
}

/// Parse a `click` directive body (everything after the `click` keyword).
/// Returns `None` if no id is present.
fn parse_click(rest: &str) -> Option<ClickDirective> {
    let toks = tokenize_click(rest);
    let mut it = toks.into_iter();
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
                if let Some(ClickTok::Quoted(u)) = rest_toks.get(i + 1) {
                    link = Some(u.clone());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            ClickTok::Word(w) if w == "call" => {
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
                i += 1;
            }
            ClickTok::Word(w) => {
                if link.is_none() && callback.is_none() {
                    let name = w.split('(').next().unwrap_or(w).trim();
                    if !name.is_empty() {
                        callback = Some(name.to_string());
                    }
                }
                i += 1;
            }
            ClickTok::Quoted(s) => {
                if link.is_none() && callback.is_none() {
                    link = Some(s.clone());
                } else if tooltip.is_none() {
                    tooltip = Some(s.clone());
                }
                i += 1;
            }
        }
    }

    Some(ClickDirective { id, link, callback, tooltip })
}

/// Split a `click` directive body into quote-aware tokens.
fn tokenize_click(s: &str) -> Vec<ClickTok> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
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
                i += 1;
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
            i += 1;
        }
    }
    out
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
    diag: &mut model::StateDiagram,
    index_of: &mut HashMap<String, usize>,
    parent: Option<usize>,
) -> usize {
    // A `[*]` inside a composite is that composite's *own* start/end: give it a
    // distinct synthetic id per parent so each composite gets its own pair.
    let (id, label, pseudo) = if raw == "[*]" {
        let suffix = parent.map(|p| format!("{p}")).unwrap_or_default();
        if as_source {
            (format!("{START_ID}{suffix}"), String::new(), Some(model::Pseudo::Start))
        } else {
            (format!("{END_ID}{suffix}"), String::new(), Some(model::Pseudo::End))
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
    diag.states.push(model::State {
        id,
        label,
        pseudo,
        kind: model::StateKind::Normal,
        parent,
        composite: false,
        style: ElemStyle::default(),
        link: None,
        callback: None,
        tooltip: None,
    });
    i
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
