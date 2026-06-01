//! `er` (entity-relationship) diagram (`erDiagram`).
//!
//! Self-contained: parse → build a `hiker_graph` layered (dagre) graph → lay
//! out → draw one SVG document. Supported subset:
//!
//! * relationships `CUSTOMER ||--o{ ORDER : places` — two entity names, a
//!   cardinality token pair `<left>--<right>` (identifying / solid) or
//!   `<left>..<right>` (non-identifying / dashed), and an optional `: label`.
//!   Cardinality tokens: `||` exactly-one, `|{`/`}|` one-or-many, `o{`/`}o`
//!   zero-or-many, `o|`/`|o` zero-or-one.
//! * entities are auto-created in first-seen order.
//! * an entity attribute block `CUSTOMER { string name PK \n int age }` — each
//!   row is `type name [keys...]`; rows render under the entity's name header.
//!
//! Cardinality is rendered with proper **crow's-foot notation**: small
//! line/path marks drawn at each entity end of the relationship line, oriented
//! along that line's terminal segment (a double bar for exactly-one, an open
//! circle for the zero forms, a splayed crow's foot for the many forms).
//! Non-identifying relationships draw a dashed line.

use std::collections::HashMap;
use std::fmt::Write as _;

use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

use crate::model::ElemStyle;
use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb, text_size};
use crate::{HitRegion, MermaidError, MermaidOptions, MermaidRender};

// ── Styling directives (classDef / class / style / :::) ───────────────────────
//
// Self-contained re-implementation of the flowchart styling parser (`parse.rs`),
// mirroring its syntax/semantics: same prop names, same color formats, same
// two-pass resolve (classDef-via-`class` first, inline `style` on top).

/// Directive state collected during parsing, resolved onto entities at the end.
#[derive(Default)]
struct Directives {
    class_defs: HashMap<String, ElemStyle>,
    /// `(entity name, class name)` from `class A,B name` and `A:::name`.
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
                // class <id1>,<id2>,... <className>  (applies to entity names)
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

    /// Resolve onto each entity's `style`: classDef-via-`class` first, then inline.
    fn resolve(&self, entities: &mut [Entity]) {
        for (id, class_name) in &self.class_assignments {
            if let Some(cs) = self.class_defs.get(class_name) {
                if let Some(e) = entities.iter_mut().find(|e| e.name == *id) {
                    merge_style(&mut e.style, cs);
                }
            }
        }
        for (id, st) in &self.inline {
            if let Some(e) = entities.iter_mut().find(|e| e.name == *id) {
                merge_style(&mut e.style, st);
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

/// Split a `name:::className` token into `(name, className)`; the className runs
/// to the next whitespace. `None` when there is no `:::` shorthand.
fn split_css_class(tok: &str) -> Option<(String, String)> {
    let (name, after) = tok.split_once(":::")?;
    let after = after.trim_start();
    let css = match after.find(char::is_whitespace) {
        Some(i) => &after[..i],
        None => after,
    };
    if css.is_empty() {
        None
    } else {
        Some((name.trim().to_string(), css.to_string()))
    }
}

/// Remove every `<id>:::<className>` shorthand from `line`, recording each
/// `(id, className)` assignment. The `id` is the run of id-chars immediately
/// preceding `:::`; the `className` runs to the next non-id char. Returns the
/// line with the `:::className` portions deleted (the id left in place).
fn strip_inline_classes(line: &str, dir: &mut Directives) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(pos) = rest.find(":::") {
        let before = &rest[..pos];
        let id_start = before
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map(|i| i + 1)
            .unwrap_or(0);
        let id = before[id_start..].trim();
        let after = &rest[pos + 3..];
        let name_len = after
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let class_name = &after[..name_len];
        if !id.is_empty() && !class_name.is_empty() {
            dir.add_shorthand(id, class_name);
        }
        out.push_str(before);
        rest = &after[name_len..];
    }
    out.push_str(rest);
    out
}

/// One cardinality end of a relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cardinality {
    /// `||` exactly one.
    ExactlyOne,
    /// `|{` / `}|` one or more.
    OneOrMore,
    /// `o{` / `}o` zero or more.
    ZeroOrMore,
    /// `o|` / `|o` zero or one.
    ZeroOrOne,
}

impl Cardinality {
    /// True when this end's outer mark is an open circle (the `o…` forms,
    /// i.e. the "zero" cardinalities).
    fn has_circle(self) -> bool {
        matches!(self, Cardinality::ZeroOrMore | Cardinality::ZeroOrOne)
    }

    /// True when this end fans out into a crow's foot (the "many" forms).
    fn has_foot(self) -> bool {
        matches!(self, Cardinality::OneOrMore | Cardinality::ZeroOrMore)
    }
}

/// An attribute row inside an entity box: `<type> <name> [<keys>] ["<comment>"]`.
/// `keys` holds the recognized `PK`/`FK`/`UK` markers in source order; `comment`
/// is the optional quoted trailing text.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Attribute {
    ty: String,
    name: String,
    /// Recognized key markers (`PK`/`FK`/`UK`), in source order.
    keys: Vec<String>,
    /// Optional quoted comment.
    comment: Option<String>,
}

impl Attribute {
    /// Whether this attribute is (part of) a primary key — rendered emphasized.
    fn is_pk(&self) -> bool {
        self.keys.iter().any(|k| k == "PK")
    }

    /// The keys joined for display, e.g. `PK,FK`. Empty when no keys.
    fn keys_text(&self) -> String {
        self.keys.join(",")
    }
}

/// An entity (table). `attrs` empty → name-only box.
#[derive(Clone, Debug, Default, PartialEq)]
struct Entity {
    name: String,
    attrs: Vec<Attribute>,
    /// Per-entity style overrides (from `classDef`/`class`/`style`/`:::`).
    style: ElemStyle,
    /// `click` interaction data: open URL (`link`), host callback name
    /// (`callback`), and hover `tooltip`. `None` unless a `click` directive
    /// targeted this entity.
    link: Option<String>,
    callback: Option<String>,
    tooltip: Option<String>,
}

/// A relationship between two entities.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Relationship {
    left: String,
    right: String,
    left_card: Cardinality,
    right_card: Cardinality,
    /// Non-identifying (`..`) → dashed line.
    dashed: bool,
    label: Option<String>,
}

/// Parsed ER diagram.
#[derive(Clone, Debug, Default, PartialEq)]
struct ErDiagram {
    /// Entities in first-seen order.
    entities: Vec<Entity>,
    relationships: Vec<Relationship>,
}

/// Parse an ER diagram source. Errors on a missing/wrong header.
fn parse(src: &str) -> Result<ErDiagram, String> {
    let mut diag = ErDiagram::default();
    let mut directives = Directives::default();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut saw_header = false;
    let mut pending_header = true;
    // When inside `ENTITY { ... }`, this holds the entity index.
    let mut in_block: Option<usize> = None;

    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if pending_header {
            let kw = line.split_whitespace().next().unwrap_or("");
            if kw != "erDiagram" {
                return Err(format!("expected `erDiagram` header, got {kw:?}"));
            }
            saw_header = true;
            pending_header = false;
            continue;
        }

        // Inside an attribute block.
        if let Some(ei) = in_block {
            if line == "}" {
                in_block = None;
                continue;
            }
            if let Some(attr) = parse_attribute(line) {
                diag.entities[ei].attrs.push(attr);
            }
            continue;
        }

        // Styling directives (`classDef`/`class`/`style`).
        if line.starts_with("classDef")
            || line.starts_with("class ")
            || line.starts_with("style ")
        {
            directives.try_parse(line);
            continue;
        }

        // Interaction directive: `click <id> ...` (same grammar as flowchart).
        // Entities are declared elsewhere; an unknown id is skipped, never
        // fabricated.
        if line.starts_with("click ") {
            let rest = line["click".len()..].trim_start();
            if let Some(c) = parse_click(rest) {
                if let Some(&i) = index_of.get(&c.id) {
                    let e = &mut diag.entities[i];
                    if c.link.is_some() {
                        e.link = c.link.clone();
                    }
                    if c.callback.is_some() {
                        e.callback = c.callback.clone();
                    }
                    if c.tooltip.is_some() {
                        e.tooltip = c.tooltip.clone();
                    }
                }
            }
            continue;
        }

        // Entity attribute-block open: `ENTITY {`. An entity name may carry a
        // `:::class` shorthand (`CUSTOMER:::big {`).
        if let Some(name) = line.strip_suffix('{') {
            let name = name.trim();
            if !name.is_empty() {
                let (name, css) = peel_css(name);
                let ei = ensure_entity(&name, &mut diag, &mut index_of);
                if let Some(css) = css {
                    directives.add_shorthand(&name, &css);
                }
                in_block = Some(ei);
                continue;
            }
        }

        // Relationship line. Either endpoint may carry a `:::class` shorthand
        // (which contains `:` and would confuse the `: label` split), so peel
        // those out of the line first.
        let stripped = strip_inline_classes(line, &mut directives);
        if let Some(rel) = parse_relationship(&stripped) {
            ensure_entity(&rel.left, &mut diag, &mut index_of);
            ensure_entity(&rel.right, &mut diag, &mut index_of);
            diag.relationships.push(rel);
            continue;
        }

        // Bare entity declaration: a single token (optionally with `:::class`).
        let mut toks = line.split_whitespace();
        if let Some(name) = toks.next() {
            if toks.next().is_none() {
                let (name, css) = peel_css(name);
                ensure_entity(&name, &mut diag, &mut index_of);
                if let Some(css) = css {
                    directives.add_shorthand(&name, &css);
                }
            }
        }
    }

    if !saw_header {
        return Err("empty input / no erDiagram header".to_string());
    }
    directives.resolve(&mut diag.entities);
    Ok(diag)
}

/// Peel a trailing `:::className` off an entity token, returning the bare name
/// and the class (if any).
fn peel_css(tok: &str) -> (String, Option<String>) {
    match split_css_class(tok) {
        Some((base, css)) => (base, Some(css)),
        None => (tok.trim().to_string(), None),
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

/// Upsert an entity by name, returning its index.
fn ensure_entity(
    name: &str,
    diag: &mut ErDiagram,
    index_of: &mut HashMap<String, usize>,
) -> usize {
    if let Some(&i) = index_of.get(name) {
        return i;
    }
    let i = diag.entities.len();
    index_of.insert(name.to_string(), i);
    diag.entities.push(Entity {
        name: name.to_string(),
        attrs: Vec::new(),
        style: ElemStyle::default(),
        link: None,
        callback: None,
        tooltip: None,
    });
    i
}

/// Parse `<type> <name> [<keys>] ["<comment>"]` → an [`Attribute`].
/// `None` if it has no name. Keys are one or more of `PK`/`FK`/`UK`, comma or
/// whitespace separated; the comment is the optional quoted trailing text.
fn parse_attribute(line: &str) -> Option<Attribute> {
    // Peel off a trailing quoted comment first, so commas/keywords inside it
    // are never mistaken for keys.
    let (head, comment) = split_trailing_quoted(line);

    let mut toks = head.split_whitespace();
    let ty = toks.next()?.to_string();
    let name = toks.next()?.to_string();

    // Remaining tokens are keys; accept comma- or space-separated PK/FK/UK and
    // ignore anything unrecognized.
    let mut keys = Vec::new();
    for tok in toks {
        for part in tok.split(',') {
            let part = part.trim();
            if matches!(part, "PK" | "FK" | "UK") {
                keys.push(part.to_string());
            }
        }
    }

    Some(Attribute {
        ty,
        name,
        keys,
        comment,
    })
}

/// Split a trailing `"..."` quoted comment off `line`, returning
/// `(head_without_comment, Some(comment))` when a closing-quoted segment ends
/// the line, else `(line, None)`.
fn split_trailing_quoted(line: &str) -> (&str, Option<String>) {
    let trimmed = line.trim_end();
    if !trimmed.ends_with('"') {
        return (line, None);
    }
    // Find the opening quote for this trailing closing quote.
    let body = &trimmed[..trimmed.len() - 1];
    if let Some(open) = body.rfind('"') {
        let comment = body[open + 1..].to_string();
        (&trimmed[..open], Some(comment))
    } else {
        (line, None)
    }
}

/// Parse a relationship line `LEFT <card>--<card> RIGHT [: label]`. Returns
/// `None` if the line has no relationship token.
fn parse_relationship(line: &str) -> Option<Relationship> {
    // Split off an optional `: label`.
    let (body, label) = match line.split_once(':') {
        Some((b, l)) => (b.trim(), Some(l.trim().to_string())),
        None => (line, None),
    };

    // Find the cardinality connector: `--` (identifying) or `..` (non-id).
    // The connector is flanked by cardinality tokens with no spaces, e.g.
    // `||--o{`. We locate the connector inside the middle whitespace-delimited
    // token.
    let mut toks = body.split_whitespace();
    let left = toks.next()?.to_string();
    let mid = toks.next()?;
    let right = toks.next()?.to_string();
    if toks.next().is_some() {
        // Extra tokens → not a simple relationship.
        return None;
    }

    let (dashed, conn_at) = if let Some(i) = mid.find("--") {
        (false, i)
    } else if let Some(i) = mid.find("..") {
        (true, i)
    } else {
        return None;
    };

    let left_tok = &mid[..conn_at];
    let right_tok = &mid[conn_at + 2..];
    let left_card = parse_card(left_tok, true)?;
    let right_card = parse_card(right_tok, false)?;

    Some(Relationship {
        left,
        right,
        left_card,
        right_card,
        dashed,
        label: label.filter(|l| !l.is_empty()),
    })
}

/// Parse a cardinality token. `left` chooses the orientation for the
/// one-or-many / zero-or-many forms (`|{` vs `}|`).
fn parse_card(tok: &str, left: bool) -> Option<Cardinality> {
    match tok {
        "||" => Some(Cardinality::ExactlyOne),
        "|{" | "}|" => Some(Cardinality::OneOrMore),
        "o{" | "}o" => Some(Cardinality::ZeroOrMore),
        "o|" | "|o" => Some(Cardinality::ZeroOrOne),
        _ => {
            let _ = left;
            None
        }
    }
}

/// Header-bar height for an entity box, px.
const HEADER_PAD_Y: f32 = 8.0;
/// Per-attribute-row height factor (× font size).
const ROW_H_EM: f32 = 1.5;

/// Render a mermaid `er` diagram to SVG.
pub fn render_er(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    Ok(render_er_inner(src, opts)?.0)
}

/// Like [`render_er`], but also returns one [`HitRegion`] per entity box (its
/// drawn rect plus any `click` data), in SVG-px coords. Used by
/// `render_with_regions` to make ER diagrams interactive.
pub fn render_er_with_regions(
    src: &str,
    opts: &MermaidOptions,
) -> Result<(MermaidRender, Vec<HitRegion>), MermaidError> {
    render_er_inner(src, opts)
}

/// Shared pipeline for [`render_er`] / [`render_er_with_regions`].
fn render_er_inner(
    src: &str,
    opts: &MermaidOptions,
) -> Result<(MermaidRender, Vec<HitRegion>), MermaidError> {
    let diag = parse(src).map_err(MermaidError::Parse)?;
    if diag.entities.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let row_h = fs * ROW_H_EM;

    // id → node index (first-seen order matches dagre node indices).
    let index_of: HashMap<&str, u32> = diag
        .entities
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.as_str(), i as u32))
        .collect();

    // Size each entity box. Attribute rows are laid out as up to 4 columns —
    // `type | name | keys | comment` — each sized to its widest cell across all
    // rows. The box width is the larger of the name header and the summed
    // columns (+ padding); height = header + attr rows.
    let sizes: Vec<(f32, f32)> = diag
        .entities
        .iter()
        .map(|e| {
            let (name_w, name_h) = text_size(&e.name, fs);
            let cols = attr_columns(e, fs);
            let rows_w = cols.iter().sum::<f32>();
            let w = name_w.max(rows_w) + 2.0 * opts.node_padding_x;
            let header_h = name_h + 2.0 * HEADER_PAD_Y;
            let h = header_h + e.attrs.len() as f32 * row_h;
            (w, h)
        })
        .collect();

    // Build the dagre edge list, plus a parallel list of edge-label box sizes so
    // dagre reserves space and positions each relationship label.
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(diag.relationships.len());
    let mut kept: Vec<usize> = Vec::with_capacity(diag.relationships.len());
    let mut label_sizes: Vec<Option<Vec2>> = Vec::with_capacity(diag.relationships.len());
    for (j, r) in diag.relationships.iter().enumerate() {
        if let (Some(&a), Some(&b)) =
            (index_of.get(r.left.as_str()), index_of.get(r.right.as_str()))
        {
            edges.push((a, b));
            kept.push(j);
            label_sizes.push(r.label.as_deref().filter(|l| !l.is_empty()).map(|l| {
                let (w, h) = text_size(l, fs);
                Vec2::new(w + 10.0, h + 6.0)
            }));
        }
    }

    let node_sizes: Vec<Vec2> = sizes.iter().map(|&(w, h)| Vec2::new(w, h)).collect();
    let engine = LayeredEngine {
        rankdir: RankDir::Tb,
        ranksep: opts.rank_sep,
        nodesep: opts.node_sep,
        edgesep: 20.0,
        default_node_size: Vec2::new(60.0, 40.0),
    };
    let out = engine.layout(&GraphInput {
        node_count: diag.entities.len(),
        edges: &edges,
        node_sizes: Some(&node_sizes),
        edge_label_sizes: Some(&label_sizes),
        node_parents: None,
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

    // Group relationships by unordered entity pair so multiple relationships
    // between the same two entities spread their labels apart (index/count fed
    // to `edge_label_anchor`).
    let mut group_total: HashMap<(String, String), usize> = HashMap::new();
    for &orig in &kept {
        let r = &diag.relationships[orig];
        *group_total.entry(pair_key(&r.left, &r.right)).or_insert(0) += 1;
    }
    let mut group_seen: HashMap<(String, String), usize> = HashMap::new();

    // Relationship lines first, then entity boxes on top.
    for (dagre_idx, &orig) in kept.iter().enumerate() {
        let r = &diag.relationships[orig];
        let pts: Vec<(f32, f32)> = out
            .edge_routes
            .get(dagre_idx)
            .map(|route| route.iter().map(|p| (p.x, p.y)).collect())
            .unwrap_or_default();
        let key = pair_key(&r.left, &r.right);
        let count = group_total.get(&key).copied().unwrap_or(1);
        let slot = group_seen.entry(key).or_insert(0);
        let index = *slot;
        *slot += 1;
        let label_pos = out
            .edge_label_positions
            .get(dagre_idx)
            .copied()
            .flatten()
            .map(|p| (p.x, p.y));
        emit_relationship(&mut svg, &pts, r, index, count, label_pos, opts);
    }

    let mut regions: Vec<HitRegion> = Vec::with_capacity(diag.entities.len());
    for (i, e) in diag.entities.iter().enumerate() {
        let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
        let (w, h) = sizes[i];
        emit_entity(&mut svg, e, pos.x, pos.y, w, h, opts);
        regions.push(HitRegion {
            id: e.name.clone(),
            x: pos.x - w / 2.0,
            y: pos.y - h / 2.0,
            w,
            h,
            link: e.link.clone(),
            callback: e.callback.clone(),
            tooltip: e.tooltip.clone(),
        });
    }

    svg.push_str("</svg>");

    Ok((
        MermaidRender {
            svg,
            width_px: width,
            height_px: height,
        },
        regions,
    ))
}

/// Inter-cell gap between attribute columns, px.
const CELL_GAP: f32 = 12.0;

/// Compute the four attribute-column widths for an entity:
/// `[type, name, keys, comment]`. Each is the widest cell in that column across
/// all rows; empty columns (e.g. no keys/comments anywhere) collapse to 0. A
/// non-empty column carries a trailing `CELL_GAP` so columns don't touch.
fn attr_columns(e: &Entity, fs: f32) -> [f32; 4] {
    let mut cols = [0.0_f32; 4];
    for a in &e.attrs {
        cols[0] = cols[0].max(text_size(&a.ty, fs).0);
        cols[1] = cols[1].max(text_size(&a.name, fs).0);
        let kt = a.keys_text();
        if !kt.is_empty() {
            cols[2] = cols[2].max(text_size(&kt, fs).0);
        }
        if let Some(c) = a.comment.as_deref() {
            cols[3] = cols[3].max(text_size(c, fs).0);
        }
    }
    // Add a trailing gap to every non-empty column except the last present one.
    for w in cols.iter_mut() {
        if *w > 0.0 {
            *w += CELL_GAP;
        }
    }
    cols
}

/// One relationship: a line (dashed for non-identifying), a crow's-foot
/// cardinality marker at each entity end, and an optional label spread off the
/// midpoint so parallel relationships don't collide.
fn emit_relationship(
    svg: &mut String,
    points: &[(f32, f32)],
    rel: &Relationship,
    index: usize,
    count: usize,
    label_pos: Option<(f32, f32)>,
    opts: &MermaidOptions,
) {
    if points.len() < 2 {
        return;
    }
    // Smooth curve through the route points (endpoints already clipped to the
    // entity borders); markers are placed separately from the original points.
    let d = crate::svgutil::smooth_path_d(points);
    let dash = if rel.dashed {
        " stroke-dasharray=\"4 3\""
    } else {
        ""
    };
    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\"{so}{dash}/>",
        d.trim_end(),
        stroke = rgb(opts.edge_stroke),
        so = opacity_attr("stroke-opacity", opts.edge_stroke),
    );

    // Crow's-foot markers at each entity end. The FROM marker sits at
    // `points[0]` pointing toward `points[1]`; the TO marker at the last point
    // pointing toward the previous one.
    let n = points.len();
    if let Some(dir) = unit(points[0], points[1]) {
        draw_crows_foot(svg, points[0], dir, rel.left_card, opts);
    }
    if let Some(dir) = unit(points[n - 1], points[n - 2]) {
        draw_crows_foot(svg, points[n - 1], dir, rel.right_card, opts);
    }

    // Relationship label, spread perpendicular off the midpoint by the edge's
    // slot within its (unordered) entity-pair group, on a light background.
    if let Some(label) = rel.label.as_deref().filter(|l| !l.is_empty()) {
        // Prefer the dagre-reserved label center; fall back to the perpendicular
        // midpoint anchor when dagre didn't position it.
        let anchor = label_pos
            .or_else(|| edge_label_anchor(points, index, count, opts.font_size_px));
        if let Some((cx, cy)) = anchor {
            let chars = label.chars().count() as f32;
            let tw = chars * opts.font_size_px * 0.6;
            let th = opts.font_size_px * 1.2;
            let bw = tw + 4.0;
            let bh = th + 4.0;
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{bw:.2}\" height=\"{bh:.2}\" \
                 fill=\"{bg}\" fill-opacity=\"0.85\"/>",
                x = cx - bw / 2.0,
                y = cy - bh / 2.0,
                bg = rgb(opts.background),
            );
            emit_text(svg, label, cx, cy, opts, opts.text_color);
        }
    }
}

/// Unit vector from `a` toward `b`, or `None` for a degenerate segment.
fn unit(a: (f32, f32), b: (f32, f32)) -> Option<(f32, f32)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    if len <= 1e-3 {
        None
    } else {
        Some((dx / len, dy / len))
    }
}

/// Draw a crow's-foot cardinality marker at `tip` (the entity end of the line),
/// oriented along `dir` — the unit vector pointing *into* the line (away from
/// the entity, toward the relationship's interior). Marks are placed a few px
/// back from `tip` so they don't overlap the entity border.
///
/// Layout, walking from the entity outward into the line:
/// * `o` forms (`has_circle`): a small open circle nearest the entity.
/// * `{`/`}` forms (`has_foot`): a crow's foot splaying toward the entity, plus
///   one perpendicular tick further in.
/// * one forms (no foot): one or two perpendicular ticks (a double bar for
///   exactly-one).
fn draw_crows_foot(
    svg: &mut String,
    tip: (f32, f32),
    dir: (f32, f32),
    card: Cardinality,
    opts: &MermaidOptions,
) {
    let stroke = rgb(opts.edge_stroke);
    let so = opacity_attr("stroke-opacity", opts.edge_stroke);
    // Perpendicular to the line direction.
    let perp = (-dir.1, dir.0);
    let half = 5.0_f32; // tick / foot half-width
    let circle_r = 3.5_f32; // open-circle radius

    // A point `t` px along the line from `tip` (into the diagram interior).
    let along = |t: f32| (tip.0 + dir.0 * t, tip.1 + dir.1 * t);
    // A short perpendicular tick centered on the line at distance `t`.
    let tick = |svg: &mut String, t: f32| {
        let (cx, cy) = along(t);
        let _ = write!(
            svg,
            "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" \
             stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
            cx - perp.0 * half,
            cy - perp.1 * half,
            cx + perp.0 * half,
            cy + perp.1 * half,
        );
    };

    if card.has_foot() {
        // Crow's foot: three lines from an apex (further into the line) splaying
        // out to the entity edge near `tip`. Plus one tick just past the apex.
        let apex = along(12.0);
        let base_t = 2.0;
        let (bx, by) = along(base_t);
        // Three splay endpoints near the entity: center + two perpendicular.
        let center = (bx, by);
        let up = (bx + perp.0 * half, by + perp.1 * half);
        let down = (bx - perp.0 * half, by - perp.1 * half);
        for end in [center, up, down] {
            let _ = write!(
                svg,
                "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" \
                 stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
                apex.0, apex.1, end.0, end.1,
            );
        }
        // Perpendicular tick just past the apex.
        tick(svg, 14.0);
        // Open circle for the zero-or-many form, beyond the tick.
        if card.has_circle() {
            let (cx, cy) = along(14.0 + circle_r + 1.5);
            let _ = write!(
                svg,
                "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{circle_r:.2}\" fill=\"none\" \
                 stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
            );
        }
    } else if card.has_circle() {
        // Zero-or-one: one tick toward the entity, an open circle further in.
        tick(svg, 5.0);
        let (cx, cy) = along(5.0 + circle_r + 3.0);
        let _ = write!(
            svg,
            "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{circle_r:.2}\" fill=\"none\" \
             stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
        );
    } else {
        // Exactly-one: a double bar (two perpendicular ticks).
        tick(svg, 4.0);
        tick(svg, 8.0);
    }
}

/// An order-independent key for an entity pair, used to group parallel
/// relationships so their labels spread apart.
fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// An entity box: a header bar with the name, then attribute rows beneath.
fn emit_entity(
    svg: &mut String,
    e: &Entity,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    opts: &MermaidOptions,
) {
    let x = cx - w / 2.0;
    let y = cy - h / 2.0;
    let header_h = opts.font_size_px * 1.2 + 2.0 * HEADER_PAD_Y;

    // Per-entity style overrides, falling back to theme defaults.
    let fill_c = e.style.fill.unwrap_or(opts.node_fill);
    let stroke_c = e.style.stroke.unwrap_or(opts.node_stroke);
    let text_c = e.style.text_color.unwrap_or(opts.text_color);
    let sw = e.style.stroke_width.unwrap_or(1.5);

    // Outer box.
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
        fill = rgb(fill_c),
        fo = opacity_attr("fill-opacity", fill_c),
        stroke = rgb(stroke_c),
        so = opacity_attr("stroke-opacity", stroke_c),
    );

    // Header name centered in the header band.
    emit_text(svg, &e.name, cx, y + header_h / 2.0, opts, text_c);

    if e.attrs.is_empty() {
        return;
    }

    // Separator under the header.
    let _ = write!(
        svg,
        "<line x1=\"{x:.2}\" y1=\"{hy:.2}\" x2=\"{x2:.2}\" y2=\"{hy:.2}\" \
         stroke=\"{stroke}\"{so} stroke-width=\"1\"/>",
        hy = y + header_h,
        x2 = x + w,
        stroke = rgb(stroke_c),
        so = opacity_attr("stroke-opacity", stroke_c),
    );

    let row_h = opts.font_size_px * ROW_H_EM;
    let fs = opts.font_size_px;
    let cols = attr_columns(e, fs);
    // A lighter color for comment text (blend text toward fill/background).
    let comment_c = lighten(text_c);
    for (i, a) in e.attrs.iter().enumerate() {
        let row_cy = y + header_h + row_h * (i as f32 + 0.5);
        // Walk the four columns left-to-right from the inner-left padding.
        let mut tx = x + opts.node_padding_x;
        // type
        emit_cell(svg, &a.ty, tx, row_cy, opts, text_c, a.is_pk());
        tx += cols[0];
        // name
        emit_cell(svg, &a.name, tx, row_cy, opts, text_c, a.is_pk());
        tx += cols[1];
        // keys (PK/FK/UK) — emphasized like the rest of a PK row.
        if cols[2] > 0.0 {
            let kt = a.keys_text();
            if !kt.is_empty() {
                emit_cell(svg, &kt, tx, row_cy, opts, text_c, true);
            }
            tx += cols[2];
        }
        // comment — lighter color.
        if cols[3] > 0.0 {
            if let Some(c) = a.comment.as_deref() {
                emit_cell(svg, c, tx, row_cy, opts, comment_c, false);
            }
        }
    }
}

/// One left-aligned attribute cell at baseline-centered `(tx, cy)`. `bold`
/// renders bolder text (used for PK rows and the key markers).
fn emit_cell(
    svg: &mut String,
    text: &str,
    tx: f32,
    cy: f32,
    opts: &MermaidOptions,
    color: [u8; 4],
    bold: bool,
) {
    if text.is_empty() {
        return;
    }
    let weight = if bold {
        " font-weight=\"bold\""
    } else {
        ""
    };
    let _ = write!(
        svg,
        "<text x=\"{tx:.2}\" y=\"{cy:.2}\" text-anchor=\"start\" \
         dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{fs}\"{weight} \
         fill=\"{fill}\"{fo}>{txt}</text>",
        family = escape(&opts.font_family),
        fs = opts.font_size_px,
        fill = rgb(color),
        fo = opacity_attr("fill-opacity", color),
        txt = escape(text),
    );
}

/// A lighter variant of `c` for de-emphasized comment text (move RGB halfway
/// toward mid-gray, keep alpha).
fn lighten(c: [u8; 4]) -> [u8; 4] {
    let mix = |v: u8| ((v as u16 + 128) / 2) as u8;
    [mix(c[0]), mix(c[1]), mix(c[2]), c[3]]
}

/// A centered single-line `<text>` in the given color.
fn emit_text(
    svg: &mut String,
    label: &str,
    cx: f32,
    cy: f32,
    opts: &MermaidOptions,
    color: [u8; 4],
) {
    if label.is_empty() {
        return;
    }
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}>{txt}</text>",
        family = escape(&opts.font_family),
        fs = opts.font_size_px,
        fill = rgb(color),
        fo = opacity_attr("fill-opacity", color),
        txt = escape(label),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    #[test]
    fn parse_relationship_basic() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places";
        let d = parse(src).unwrap();
        assert_eq!(d.entities.len(), 2);
        assert_eq!(d.entities[0].name, "CUSTOMER");
        assert_eq!(d.entities[1].name, "ORDER");
        assert_eq!(d.relationships.len(), 1);
        let r = &d.relationships[0];
        assert_eq!(r.left_card, Cardinality::ExactlyOne);
        assert_eq!(r.right_card, Cardinality::ZeroOrMore);
        assert!(!r.dashed);
        assert_eq!(r.label.as_deref(), Some("places"));
    }

    #[test]
    fn parse_all_cardinalities() {
        let src = "erDiagram\n  A |{--}o B\n  C o|--|o D";
        let d = parse(src).unwrap();
        assert_eq!(d.relationships[0].left_card, Cardinality::OneOrMore);
        assert_eq!(d.relationships[0].right_card, Cardinality::ZeroOrMore);
        assert_eq!(d.relationships[1].left_card, Cardinality::ZeroOrOne);
        assert_eq!(d.relationships[1].right_card, Cardinality::ZeroOrOne);
    }

    #[test]
    fn parse_non_identifying_is_dashed() {
        let src = "erDiagram\n  A ||..o{ B";
        let d = parse(src).unwrap();
        assert!(d.relationships[0].dashed);
    }

    #[test]
    fn parse_attribute_block() {
        let src = "erDiagram\n  CUSTOMER {\n    string name PK\n    int age\n  }";
        let d = parse(src).unwrap();
        assert_eq!(d.entities.len(), 1);
        let attrs = &d.entities[0].attrs;
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].ty, "string");
        assert_eq!(attrs[0].name, "name");
        assert_eq!(attrs[0].keys, vec!["PK"]);
        assert_eq!(attrs[0].comment, None);
        assert_eq!(attrs[1].ty, "int");
        assert_eq!(attrs[1].name, "age");
        assert!(attrs[1].keys.is_empty());
        assert_eq!(attrs[1].comment, None);
    }

    #[test]
    fn parse_attribute_keys_and_comment() {
        // From the task: `CUSTOMER { string name PK "the name" int age }`.
        let src = "erDiagram\n  CUSTOMER {\n    string name PK \"the name\"\n    int age\n  }";
        let d = parse(src).unwrap();
        let attrs = &d.entities[0].attrs;
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].name, "name");
        assert_eq!(attrs[0].keys, vec!["PK"]);
        assert_eq!(attrs[0].comment.as_deref(), Some("the name"));
        assert!(attrs[0].is_pk());
        // `age` has no key and no comment.
        assert_eq!(attrs[1].name, "age");
        assert!(attrs[1].keys.is_empty());
        assert_eq!(attrs[1].comment, None);
        assert!(!attrs[1].is_pk());
    }

    #[test]
    fn parse_multiple_keys_comma_and_space() {
        let src = "erDiagram\n  T {\n    int a PK,FK \"c1\"\n    int b PK UK\n  }";
        let d = parse(src).unwrap();
        let attrs = &d.entities[0].attrs;
        assert_eq!(attrs[0].keys, vec!["PK", "FK"]);
        assert_eq!(attrs[0].comment.as_deref(), Some("c1"));
        assert_eq!(attrs[1].keys, vec!["PK", "UK"]);
        assert_eq!(attrs[1].comment, None);
    }

    #[test]
    fn bad_header_errors() {
        assert!(parse("graph TD\n a --> b").is_err());
    }

    #[test]
    fn no_header_errors() {
        assert!(parse("\n\n").is_err());
    }

    #[test]
    fn render_wellformed_svg() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_entity_and_relationship_counts() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE : has";
        let r = render_er(src, &opts()).unwrap();
        // Three entities → at least three entity <rect> (label bg rects add more).
        assert!(r.svg.matches("<rect").count() >= 3);
        // Two relationships → two edge <path>s (fill="none"). Crow's-foot
        // circles also use fill="none", so count the <path…fill="none"> form.
        assert_eq!(r.svg.matches("<path d=").count(), 2);
        // Relationship labels present.
        assert!(r.svg.contains(">places<"));
        assert!(r.svg.contains(">has<"));
    }

    #[test]
    fn dashed_line_for_non_identifying() {
        let src = "erDiagram\n  A ||..o{ B";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains("stroke-dasharray=\"4 3\""));
    }

    #[test]
    fn solid_line_has_no_dash() {
        let src = "erDiagram\n  A ||--o{ B";
        let r = render_er(src, &opts()).unwrap();
        assert!(!r.svg.contains("stroke-dasharray"));
    }

    #[test]
    fn attribute_rows_rendered() {
        let src = "erDiagram\n  CUSTOMER {\n    string name PK\n  }\n  CUSTOMER ||--o{ ORDER : x";
        let r = render_er(src, &opts()).unwrap();
        // Attribute cells appear (type, name, key each in their own <text>).
        assert!(r.svg.contains(">string<"));
        assert!(r.svg.contains(">name<"));
        assert!(r.svg.contains(">PK<"));
        // Separator <line> under the header.
        assert!(r.svg.contains("<line"));
    }

    #[test]
    fn keys_and_comment_rendered() {
        let src = "erDiagram\n  CUSTOMER {\n    string name PK \"the name\"\n    int age\n  }";
        let r = render_er(src, &opts()).unwrap();
        // Key marker and comment text both present as their own cells.
        assert!(r.svg.contains(">PK<"), "PK key rendered: {}", r.svg);
        assert!(r.svg.contains(">the name<"), "comment rendered: {}", r.svg);
        // PK row is emphasized (bold) somewhere.
        assert!(r.svg.contains("font-weight=\"bold\""));
    }

    #[test]
    fn box_grows_to_fit_keys_and_comments() {
        // Same entity with vs. without keys/comment → the keyed box is wider.
        let bare = render_er("erDiagram\n  C {\n    string name\n  }", &opts()).unwrap();
        let keyed = render_er(
            "erDiagram\n  C {\n    string name PK \"a long-ish comment\"\n  }",
            &opts(),
        )
        .unwrap();
        assert!(
            keyed.width_px > bare.width_px,
            "keyed/comment box ({}) should be wider than bare ({})",
            keyed.width_px,
            bare.width_px,
        );
    }

    #[test]
    fn name_only_entity_has_no_attr_cells() {
        // An attribute-less entity renders unchanged: no separator line, no cells.
        let src = "erDiagram\n  LONE";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains(">LONE<"));
        assert!(!r.svg.contains("<line"), "no attr separator for name-only box");
        assert!(!r.svg.contains("font-weight=\"bold\""));
    }

    #[test]
    fn crows_foot_marks_drawn() {
        // `||` (exactly-one) → a double bar of perpendicular ticks; `o{`
        // (zero-or-many) → a crow's foot plus an open circle. No textual glyphs.
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER";
        let r = render_er(src, &opts()).unwrap();
        // Old textual cardinality glyphs are gone.
        assert!(!r.svg.contains(">1</text>"));
        assert!(!r.svg.contains(">0+</text>"));
        // The zero-or-many end draws an open circle.
        assert!(r.svg.contains("<circle"), "zero-cardinality end has a circle");
        // Crow's-foot / tick marks are <line> elements (beyond the relationship
        // <path>). There should be several.
        assert!(r.svg.matches("<line").count() >= 4, "expected tick/foot lines: {}", r.svg);
    }

    #[test]
    fn exactly_one_has_no_circle() {
        // `||--||` → both ends are exactly-one (double bars), no circles.
        let src = "erDiagram\n  A ||--|| B";
        let r = render_er(src, &opts()).unwrap();
        assert!(!r.svg.contains("<circle"), "exactly-one ends draw no circle");
    }

    #[test]
    fn zero_or_one_end_has_circle() {
        // `o|` → zero-or-one: an open circle marker.
        let src = "erDiagram\n  A o|--|| B";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains("<circle"));
    }

    #[test]
    fn xml_escapes_label() {
        let src = "erDiagram\n  A ||--o{ B : a & b < c";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains("a &amp; b &lt; c"));
        assert!(!r.svg.contains("a & b"));
    }

    #[test]
    fn empty_diagram_errors() {
        assert_eq!(render_er("erDiagram\n", &opts()), Err(MermaidError::Empty));
    }

    #[test]
    fn deterministic() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE : has";
        let a = render_er(src, &opts()).unwrap();
        let b = render_er(src, &opts()).unwrap();
        assert_eq!(a, b);
    }

    // ---- styling directives ----

    fn style_of<'a>(d: &'a ErDiagram, name: &str) -> &'a ElemStyle {
        &d.entities.iter().find(|e| e.name == name).expect("entity").style
    }

    #[test]
    fn classdef_and_class_apply() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  classDef big fill:#00f\n  class CUSTOMER big";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "CUSTOMER").fill, Some([0, 0, 255, 255]));
        // ORDER untouched.
        assert_eq!(style_of(&d, "ORDER").fill, None);
    }

    #[test]
    fn triple_colon_shorthand() {
        let src = "erDiagram\n  CUSTOMER:::big ||--o{ ORDER : places\n  classDef big fill:#00f";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "CUSTOMER").fill, Some([0, 0, 255, 255]));
        // Relationship recorded with bare names.
        assert_eq!(d.relationships[0].left, "CUSTOMER");
        assert_eq!(d.relationships[0].right, "ORDER");
        assert_eq!(d.relationships[0].label.as_deref(), Some("places"));
    }

    #[test]
    fn style_directive_overrides_class() {
        let src = "erDiagram\n  A ||--o{ B\n  classDef big fill:#00f\n  class A big\n  style A fill:#0f0";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "A").fill, Some([0, 255, 0, 255]));
    }

    #[test]
    fn style_override_in_rendered_svg() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  classDef big fill:#00f\n  class CUSTOMER big";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains(&rgb([0, 0, 255, 255])), "override fill present: {}", r.svg);
    }

    #[test]
    fn unstyled_entities_unchanged() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places";
        let d = parse(src).unwrap();
        for e in &d.entities {
            assert_eq!(e.style, ElemStyle::default());
        }
    }

    // ---- click / interaction ----

    #[test]
    fn click_sets_link_and_tooltip() {
        let src = "erDiagram\n CUSTOMER ||--o{ ORDER : places\n click CUSTOMER \"https://x\" \"tip\"\n";
        let d = parse(src).unwrap();
        let c = d.entities.iter().find(|e| e.name == "CUSTOMER").unwrap();
        assert_eq!(c.link.as_deref(), Some("https://x"));
        assert_eq!(c.tooltip.as_deref(), Some("tip"));
        assert!(c.callback.is_none());
        // Unknown id skipped, not fabricated.
        let d2 = parse("erDiagram\n CUSTOMER ||--o{ ORDER : places\n click GHOST \"https://y\"\n").unwrap();
        assert_eq!(d2.entities.len(), 2);
    }

    #[test]
    fn regions_carry_click_data() {
        let src = "erDiagram\n CUSTOMER ||--o{ ORDER : places\n click CUSTOMER \"https://x\" \"tip\"\n";
        let (render, regions) = render_er_with_regions(src, &opts()).unwrap();
        assert_eq!(regions.len(), 2);
        let c = regions.iter().find(|r| r.id == "CUSTOMER").unwrap();
        assert_eq!(c.link.as_deref(), Some("https://x"));
        assert_eq!(c.tooltip.as_deref(), Some("tip"));
        assert!(c.w > 0.0 && c.h > 0.0);
        assert!(c.x >= 0.0 && c.y >= 0.0);
        assert!(c.x + c.w <= render.width_px + 1.0);
        assert!(c.y + c.h <= render.height_px + 1.0);
        let o = regions.iter().find(|r| r.id == "ORDER").unwrap();
        assert!(o.link.is_none() && o.callback.is_none() && o.tooltip.is_none());
    }

    #[test]
    fn regions_without_click_and_svg_unchanged() {
        let src = "erDiagram\n CUSTOMER ||--o{ ORDER : places\n";
        let plain = render_er(src, &opts()).unwrap();
        let (with_regions, regions) = render_er_with_regions(src, &opts()).unwrap();
        assert_eq!(regions.len(), 2);
        assert!(regions.iter().all(|r| r.link.is_none()
            && r.callback.is_none()
            && r.tooltip.is_none()
            && r.w > 0.0
            && r.h > 0.0));
        assert_eq!(plain.svg, with_regions.svg);
        assert_eq!(plain.width_px, with_regions.width_px);
        assert_eq!(plain.height_px, with_regions.height_px);
    }
}
