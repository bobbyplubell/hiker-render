//! ER-diagram parser: `erDiagram` source → [`super::model::ErDiagram`]. Handles
//! relationships with cardinality tokens, entity attribute blocks, styling
//! directives (`classDef`/`class`/`style`/`:::`), and `click` interaction.

use std::collections::HashMap;

use crate::model::ElemStyle;

use super::model;

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
    fn resolve(&self, entities: &mut [model::Entity]) {
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

/// Parse an ER diagram source. Errors on a missing/wrong header.
pub(super) fn parse(src: &str) -> Result<model::ErDiagram, String> {
    let mut diag = model::ErDiagram::default();
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
    diag: &mut model::ErDiagram,
    index_of: &mut HashMap<String, usize>,
) -> usize {
    if let Some(&i) = index_of.get(name) {
        return i;
    }
    let i = diag.entities.len();
    index_of.insert(name.to_string(), i);
    diag.entities.push(model::Entity {
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
fn parse_attribute(line: &str) -> Option<model::Attribute> {
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

    Some(model::Attribute {
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
fn parse_relationship(line: &str) -> Option<model::Relationship> {
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

    Some(model::Relationship {
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
fn parse_card(tok: &str, left: bool) -> Option<model::Cardinality> {
    match tok {
        "||" => Some(model::Cardinality::ExactlyOne),
        "|{" | "}|" => Some(model::Cardinality::OneOrMore),
        "o{" | "}o" => Some(model::Cardinality::ZeroOrMore),
        "o|" | "|o" => Some(model::Cardinality::ZeroOrOne),
        _ => {
            let _ = left;
            None
        }
    }
}
