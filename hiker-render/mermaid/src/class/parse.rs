//! Class-diagram parser: `classDiagram` source → [`super::model::ClassDiagram`].
//! Handles classes (block and member-line forms), relationships with UML
//! markers, generics, annotations/stereotypes, notes, styling directives
//! (`classDef`/`class`/`cssClass`/`style`/`:::`), and `click` interaction.

use crate::model::ElemStyle;

use super::model;

/// Directive state collected during parsing, resolved onto classes at the end.
#[derive(Default)]
struct Directives {
    /// Named `classDef` styles.
    class_defs: std::collections::HashMap<String, ElemStyle>,
    /// `(element id, class name)` assignments from `class A,B name`,
    /// `cssClass "A" name`, and `A:::name`.
    class_assignments: Vec<(String, String)>,
    /// Inline `style <id> ...` overrides applied directly to an element.
    inline: Vec<(String, ElemStyle)>,
}

impl Directives {
    /// Try to parse `line` as a styling directive. Returns `true` if it was a
    /// recognized directive keyword line (and should not be parsed further).
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
                // class <id1>,<id2>,... <className>
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
            "cssClass" => {
                // cssClass "<id>" <className>  (mermaid class-diagram form)
                let rest = line[kw.len()..].trim_start();
                if let Some(sp) = rest.rfind(char::is_whitespace) {
                    let ids = rest[..sp].trim().trim_matches('"');
                    let class_name = rest[sp..].trim();
                    if !class_name.is_empty() {
                        for id in ids.split(',') {
                            let id = id.trim().trim_matches('"');
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
                // style <id> <prop:val,...>
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

    /// Record an `id:::className` shorthand assignment.
    fn add_shorthand(&mut self, id: &str, class_name: &str) {
        if !id.is_empty() && !class_name.is_empty() {
            self.class_assignments
                .push((id.to_string(), class_name.to_string()));
        }
    }

    /// Resolve the collected directives onto each class's `style`: classDef-via-
    /// `class` first, then inline `style` overrides on top.
    fn resolve(&self, classes: &mut [model::Class]) {
        for (id, class_name) in &self.class_assignments {
            if let Some(cs) = self.class_defs.get(class_name) {
                if let Some(c) = classes.iter_mut().find(|c| c.name == *id) {
                    merge_style(&mut c.style, cs);
                }
            }
        }
        for (id, st) in &self.inline {
            if let Some(c) = classes.iter_mut().find(|c| c.name == *id) {
                merge_style(&mut c.style, st);
            }
        }
    }
}

/// Merge `src` into `dst` field-by-field: any set field in `src` wins.
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

/// Parse a `prop:val,prop:val,...` list into an [`ElemStyle`]. Unknown props or
/// unparseable colors are skipped leniently.
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
                if let Some(w) = val.trim_end_matches("px").trim().parse::<f32>().ok() {
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

/// Parse a CSS-ish color into straight RGBA. Returns `None` on anything
/// unrecognized so the caller can skip the prop.
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

/// Parse-time builders that upsert classes/members/annotations/clicks onto the
/// [`model::ClassDiagram`] as statements are read.
impl model::ClassDiagram {
    /// Index of `name`, inserting an empty class if not present (auto-create).
    /// The display name defaults to the id; use [`ensure_named`] to set a
    /// distinct (generic) display.
    fn ensure(&mut self, name: &str) -> usize {
        self.ensure_named(name, name)
    }

    /// Like [`ensure`], but sets `display_name` for a freshly created class (or
    /// upgrades a placeholder whose display still equals its id, e.g. one
    /// auto-created from a relationship before the `List~int~` definition).
    fn ensure_named(&mut self, name: &str, display: &str) -> usize {
        if let Some(i) = self.classes.iter().position(|c| c.name == name) {
            let c = &mut self.classes[i];
            if c.display_name == c.name && display != name {
                c.display_name = display.to_string();
            }
            return i;
        }
        self.classes.push(model::Class {
            name: name.to_string(),
            display_name: display.to_string(),
            ..model::Class::default()
        });
        self.classes.len() - 1
    }

    /// Record a stereotype/annotation (without `<<` `>>`) on a class.
    fn annotate(&mut self, class: &str, annotation: &str) {
        let i = self.ensure(class);
        self.classes[i].annotation = Some(annotation.trim().to_string());
    }

    fn add_member(&mut self, class: &str, raw: &str) {
        let m = parse_member(raw);
        let i = self.ensure(class);
        if m.is_method {
            self.classes[i].methods.push(m);
        } else {
            self.classes[i].attributes.push(m);
        }
    }

    /// Attach a parsed `click` directive's link/callback/tooltip to the named
    /// class. Unknown ids are skipped (classes are declared explicitly, so we
    /// do not fabricate one). The id may carry a generic suffix; match the base.
    fn apply_click(&mut self, c: &ClickDirective) {
        let base = strip_generic(&c.id);
        let Some(i) = self.classes.iter().position(|cl| cl.name == base) else {
            return;
        };
        let cl = &mut self.classes[i];
        if c.link.is_some() {
            cl.link = c.link.clone();
        }
        if c.callback.is_some() {
            cl.callback = c.callback.clone();
        }
        if c.tooltip.is_some() {
            cl.tooltip = c.tooltip.clone();
        }
    }
}

// ── Parse ───────────────────────────────────────────────────────────────────

/// Strip a trailing generic suffix like `~T~` / `~K, V~` from a bare class name,
/// returning the base id used for relationship matching.
fn strip_generic(name: &str) -> &str {
    match name.find('~') {
        Some(i) => name[..i].trim(),
        None => name.trim(),
    }
}

/// Split a class name into `(base_id, display)`. A generic suffix `~K, V~`
/// becomes `<K, V>` in the display; the base id has it removed. Mermaid uses the
/// first two `~`-delimited segments: `List~int~` → base `List`, generic `int`.
pub(super) fn split_generic(name: &str) -> (String, String) {
    let name = name.trim();
    match name.split_once('~') {
        Some((base, rest)) => {
            let base = base.trim();
            // The generic body runs to the closing `~` (if any).
            let inner = rest.split_once('~').map(|(g, _)| g).unwrap_or(rest).trim();
            let display = if inner.is_empty() {
                base.to_string()
            } else {
                format!("{base}<{inner}>")
            };
            (base.to_string(), display)
        }
        None => (name.to_string(), name.to_string()),
    }
}

/// Convert any `~…~` generic markers in arbitrary member text to `<…>` for
/// display (e.g. `+List~int~ items` → `+List<int> items`). Pairs of `~` become
/// `<`/`>`; an unpaired trailing `~` is left as-is.
fn generics_to_angles(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut open = false;
    for ch in text.chars() {
        if ch == '~' {
            out.push(if open { '>' } else { '<' });
            open = !open;
        } else {
            out.push(ch);
        }
    }
    // Unbalanced: a lone `~` was turned into `<`; restore it to a literal `~`.
    if open {
        if let Some(pos) = out.rfind('<') {
            out.replace_range(pos..pos + 1, "~");
        }
    }
    out
}

/// Parse one member text into a [`Member`], classifying method vs attribute and
/// converting any generic `~…~` to `<…>` for display.
pub(super) fn parse_member(raw: &str) -> model::Member {
    let text = generics_to_angles(raw.trim());
    // A method ends with a `)` somewhere (has a parameter list). Attributes have
    // no parentheses.
    let is_method = text.contains('(') && text.contains(')');
    model::Member { text, is_method }
}

/// Parse a relationship line into the two endpoint names, marker, and label.
/// Returns `None` if no relationship token is present.
fn parse_relation_line(line: &str) -> Option<model::Relation> {
    // Split off a trailing `: label` (after the relationship). Cardinality
    // labels in quotes are ignored.
    let (rel_part, label) = match line.split_once(':') {
        Some((l, r)) => {
            let t = r.trim();
            (l.trim(), if t.is_empty() { None } else { Some(t.to_string()) })
        }
        None => (line.trim(), None),
    };

    let (tok, marker, marker_at_to, dashed) = match_relation_earliest(rel_part)?;
    let idx = rel_part.find(tok)?;
    let left = rel_part[..idx].trim();
    let right = rel_part[idx + tok.len()..].trim();
    let left = strip_cardinality(left);
    let right = strip_cardinality(right);
    let left = strip_generic(left);
    let right = strip_generic(right);
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some(model::Relation {
        from: left.to_string(),
        to: right.to_string(),
        marker,
        marker_at_to,
        dashed,
        label,
    })
}

/// Drop a trailing/leading quoted cardinality like `"1"` / `"0..*"` from an
/// endpoint token, returning the bare class name.
fn strip_cardinality(s: &str) -> &str {
    let s = s.trim();
    // Endpoint may look like `Foo "1"` or `"*" Bar`. Remove quoted runs.
    if let Some(q) = s.find('"') {
        // Take whichever side of the quotes is the (unquoted) identifier.
        let before = s[..q].trim();
        if !before.is_empty() {
            return before;
        }
        // Quote leads; the name is after the closing quote.
        if let Some(end) = s[q + 1..].find('"') {
            return s[q + 1 + end + 1..].trim();
        }
    }
    s
}

/// Scan all relationship tokens and return the one whose match starts earliest
/// in `s` (ties broken by longest token), to avoid `--` shadowing `--|>` etc.
fn match_relation_earliest(s: &str) -> Option<(&'static str, model::RelMarker, bool, bool)> {
    const TOKENS: &[(&str, model::RelMarker, bool, bool)] = &[
        ("..|>", model::RelMarker::Triangle, true, true),
        ("<|..", model::RelMarker::Triangle, false, true),
        ("..>", model::RelMarker::Arrow, true, true),
        ("<..", model::RelMarker::Arrow, false, true),
        ("--|>", model::RelMarker::Triangle, true, false),
        ("<|--", model::RelMarker::Triangle, false, false),
        ("-->", model::RelMarker::Arrow, true, false),
        ("<--", model::RelMarker::Arrow, false, false),
        ("--*", model::RelMarker::DiamondFilled, true, false),
        ("*--", model::RelMarker::DiamondFilled, false, false),
        ("--o", model::RelMarker::DiamondHollow, true, false),
        ("o--", model::RelMarker::DiamondHollow, false, false),
        ("..", model::RelMarker::None, true, true),
        ("--", model::RelMarker::None, true, false),
    ];
    let mut best: Option<(usize, &'static str, model::RelMarker, bool, bool)> = None;
    for &(tok, marker, at_to, dashed) in TOKENS {
        if let Some(pos) = s.find(tok) {
            let take = match best {
                None => true,
                Some((bpos, btok, ..)) => pos < bpos || (pos == bpos && tok.len() > btok.len()),
            };
            if take {
                best = Some((pos, tok, marker, at_to, dashed));
            }
        }
    }
    best.map(|(_, tok, m, at, d)| (tok, m, at, d))
}

/// Parse a line that is *only* an annotation `<<interface>>`, returning the
/// inner text. Returns `None` if the line has trailing text after `>>` (which is
/// the standalone-with-target form handled separately).
fn parse_annotation(line: &str) -> Option<&str> {
    let line = line.trim();
    let inner = line.strip_prefix("<<")?;
    let (ann, rest) = inner.split_once(">>")?;
    if rest.trim().is_empty() {
        Some(ann.trim())
    } else {
        None
    }
}

/// Parse the standalone form `<<interface>> Shape`, returning
/// `(annotation, target_class)`. Returns `None` if there is no target.
fn parse_standalone_annotation(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    let inner = line.strip_prefix("<<")?;
    let (ann, rest) = inner.split_once(">>")?;
    let target = rest.trim();
    if target.is_empty() {
        None
    } else {
        Some((ann.trim(), target))
    }
}

/// Parse a `note for <Class> "text"` or floating `note "text"` line.
fn parse_note(line: &str) -> Option<model::Note> {
    let rest = line.trim().strip_prefix("note")?.trim_start();
    let (for_class, body) = if let Some(after) = rest.strip_prefix("for ") {
        // `for <Class> "text"` — the class id runs up to the opening quote (or
        // to the end if unquoted).
        let after = after.trim_start();
        match after.find('"') {
            Some(q) => (Some(after[..q].trim().to_string()), &after[q..]),
            None => (Some(after.trim().to_string()), ""),
        }
    } else {
        (None, rest)
    };
    let text = unquote(body.trim());
    if let Some(c) = &for_class {
        if c.is_empty() {
            return None;
        }
    }
    Some(model::Note {
        text,
        for_class: for_class.map(|c| strip_generic(&c).to_string()),
    })
}

/// Strip a single pair of surrounding double quotes, if present.
fn unquote(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s)
        .to_string()
}

// ── Click directives ──────────────────────────────────────────────────────────
//
// Same quote-aware grammar as the flowchart `click` parser (`parse.rs`), shared
// across the interactive diagram types:
// - `<id> "<url>" ["<tooltip>"]`             → link (+ tooltip)
// - `<id> href "<url>" ["<tooltip>"]`        → link (+ tooltip)
// - `<id> call <name>(<args>) ["<tooltip>"]` → callback = name (args dropped)
// - `<id> callback` / `<id> <name>` (bareword) → callback = the word

/// A parsed `click`/`link`/`callback` directive body.
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

/// Parse a `click` directive body (everything after the `click`/`link`/
/// `callback` keyword). Returns `None` if no id is present.
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

/// Parse `classDiagram` source. Errors if the header is wrong.
pub fn parse(src: &str) -> Result<model::ClassDiagram, String> {
    let mut lines = src.lines().map(|l| l.split("%%").next().unwrap_or("")).peekable();

    // Header: first non-blank line must start with `classDiagram`.
    let header = loop {
        match lines.next() {
            Some(l) if l.trim().is_empty() => continue,
            Some(l) => break l.trim().to_string(),
            None => return Err("empty input".to_string()),
        }
    };
    if !header.starts_with("classDiagram") {
        return Err(format!("expected `classDiagram` header, got {header:?}"));
    }

    let mut diagram = model::ClassDiagram::default();
    let mut directives = Directives::default();
    // When inside a `class X {` block, the class we are appending members to.
    let mut open_block: Option<String> = None;

    for raw in lines {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        // Inside a `{ ... }` block: each line is a member until `}`.
        if let Some(cls) = open_block.clone() {
            if line == "}" || line.starts_with('}') {
                open_block = None;
                continue;
            }
            // An in-body annotation line `<<interface>>` sets the class
            // stereotype instead of a member.
            if let Some(ann) = parse_annotation(line) {
                diagram.annotate(&cls, ann);
                continue;
            }
            diagram.add_member(&cls, line);
            continue;
        }

        // Styling directives: `classDef`, `class A,B name`, `cssClass`, `style`.
        // `class Name { ... }` definitions are handled below, so only treat a
        // `class …` line as a directive when it is *not* a class definition
        // (i.e. it carries a trailing class-name token and no `{`).
        if line.starts_with("classDef")
            || line.starts_with("cssClass")
            || line.starts_with("style ")
        {
            directives.try_parse(line);
            continue;
        }

        // `note for <Class> "text"` / `note "text"`.
        if line == "note" || line.starts_with("note ") {
            if let Some(note) = parse_note(line) {
                if let Some(c) = &note.for_class {
                    diagram.ensure(c);
                }
                diagram.notes.push(note);
            }
            continue;
        }

        // Standalone annotation: `<<interface>> Shape` (or `<<interface>>` alone,
        // which has no target and is ignored).
        if line.starts_with("<<") {
            if let Some((ann, target)) = parse_standalone_annotation(line) {
                let target = strip_generic(target);
                if !target.is_empty() {
                    diagram.annotate(target, ann);
                }
            }
            continue;
        }

        // Interaction directives: `click <id> ...`, `link <id> ...`,
        // `callback <id> ...`. All share the same click grammar (see
        // [`parse_click`]). Unknown ids are skipped — class diagrams declare
        // classes explicitly, so we never fabricate one from a `click`.
        if line.starts_with("click ")
            || line.starts_with("link ")
            || line.starts_with("callback ")
        {
            let rest = line.split_once(char::is_whitespace).map(|(_, r)| r).unwrap_or("");
            if let Some(c) = parse_click(rest) {
                diagram.apply_click(&c);
            }
            continue;
        }

        // Skip standalone directives we don't model.
        if line.starts_with("direction") || line.starts_with("namespace") {
            continue;
        }

        // A `class Name {` or `class Name` definition.
        if let Some(rest) = line.strip_prefix("class ") {
            let rest = rest.trim();
            if let Some(brace) = rest.find('{') {
                let (name, display) = split_generic(rest[..brace].trim());
                let name = name.as_str();
                let after = rest[brace + 1..].trim();
                let i = diagram.ensure_named(name, &display);
                let _ = i;
                // Members may follow on the same line, separated, but mermaid
                // normally puts them on their own lines. If the brace closes on
                // this line too, handle inline members.
                if let Some(close) = after.find('}') {
                    let inner = after[..close].trim();
                    if !inner.is_empty() {
                        for part in inner.split(';') {
                            let p = part.trim();
                            if !p.is_empty() {
                                diagram.add_member(name, p);
                            }
                        }
                    }
                } else {
                    open_block = Some(name.to_string());
                    if !after.is_empty() {
                        diagram.add_member(name, after);
                    }
                }
            } else if !rest.contains(":::")
                && !rest.contains('~')
                && rest.split_whitespace().count() >= 2
            {
                // `class <id1>,<id2>,... <className>` — a style-assignment
                // directive, not a definition (two whitespace-delimited groups,
                // no `:::`, no generic, no body). The classes themselves are
                // assumed defined elsewhere (assignment alone does not create
                // them). A generic like `class Map~K, V~` keeps a space inside
                // its `~…~`, so it is excluded here and handled as a definition.
                directives.try_parse(line);
            } else {
                // `class Name` (no body). Also handles `class Name:::cssClass`
                // and generics `class List~int~`.
                let (name_part, css) = match rest.split_once(":::") {
                    Some((n, c)) => (n, Some(c.trim())),
                    None => (rest, None),
                };
                let (name, display) = split_generic(name_part.trim());
                if !name.is_empty() {
                    diagram.ensure_named(&name, &display);
                    if let Some(css) = css.filter(|c| !c.is_empty()) {
                        directives.add_shorthand(&name, css);
                    }
                }
            }
            continue;
        }

        // A member line: `ClassName : +int age`.
        // But only if there's no relationship token (relationships may also have
        // a `:` for labels). Detect a relationship token first.
        // Peel any `<id>:::<className>` shorthands out of the line first (they
        // contain `:` which would otherwise confuse label/member splitting),
        // recording the class assignment for each.
        let line_owned = strip_inline_classes(line, &mut directives);
        let line = line_owned.as_str();

        if match_relation_earliest(line).is_some() {
            if let Some(rel) = parse_relation_line(line) {
                diagram.ensure(&rel.from);
                diagram.ensure(&rel.to);
                diagram.relations.push(rel);
                continue;
            }
        }

        if let Some((lhs, rhs)) = line.split_once(':') {
            let (cls, display) = split_generic(lhs.trim());
            let member = rhs.trim();
            if !cls.is_empty() && !member.is_empty() {
                diagram.ensure_named(&cls, &display);
                diagram.add_member(&cls, member);
            }
            continue;
        }

        // Bare class reference like `ClassName` or `List~int~` on its own line
        // (any `:::css` shorthand was already peeled by `strip_inline_classes`).
        let (bare, display) = split_generic(line);
        if !bare.is_empty() && bare.chars().all(|c| c.is_alphanumeric() || c == '_') {
            diagram.ensure_named(&bare, &display);
        }
        // Otherwise ignore unrecognized line.
    }

    directives.resolve(&mut diagram.classes);
    Ok(diagram)
}

/// Remove every `<id>:::<className>` shorthand from `line`, recording each
/// `(id, className)` assignment into `dir`. The `id` is the run of id-chars
/// immediately preceding `:::`; the `className` runs to the next non-id char.
/// Returns the line with the `:::className` portions deleted (id left in place).
fn strip_inline_classes(line: &str, dir: &mut Directives) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(pos) = rest.find(":::") {
        // The id is the trailing id-chars of the text before `:::`.
        let before = &rest[..pos];
        let id_start = before
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map(|i| i + 1)
            .unwrap_or(0);
        let id = before[id_start..].trim();
        // The className is the id-chars right after `:::`.
        let after = &rest[pos + 3..];
        let name_len = after
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let class_name = &after[..name_len];
        if !id.is_empty() && !class_name.is_empty() {
            dir.add_shorthand(id, class_name);
        }
        // Emit everything up to (and including) the id, drop `:::className`.
        out.push_str(before);
        rest = &after[name_len..];
    }
    out.push_str(rest);
    out
}
