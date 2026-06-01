//! `classDiagram` — UML class diagrams.
//!
//! Self-contained `parse → size → layout → draw` for the mermaid class diagram
//! subset. Classes become multi-compartment boxes (name / attributes / methods)
//! laid out with the [`hiker_graph`] layered (dagre) engine; relationships become
//! routed polylines with the appropriate UML end marker (inheritance triangle,
//! association/dependency arrow, aggregation/composition diamond), dashed for
//! `..` (dependency / realization) lines.
//!
//! Supported extras: generics `~T~` (e.g. `List~int~` renders `List<int>`,
//! but relationships match the base `List` id), annotations / stereotypes
//! `<<interface>>` (rendered «interface» in italics above the class name, both
//! the in-body and standalone forms), and `note`s (`note for Class "text"` and
//! floating `note "text"`).
//!
//! Skipped (noted, not parsed specially): namespaces and cardinality/
//! multiplicity labels.

use std::fmt::Write as _;

use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb, text_size, LINE_HEIGHT_EM};
use crate::{HitRegion, MermaidError, MermaidOptions, MermaidRender};

use crate::model::ElemStyle;
use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

// ── Styling directives (classDef / class / cssClass / style / :::) ────────────
//
// A small, self-contained re-implementation of the flowchart styling parser
// (`parse.rs`), mirroring its syntax/semantics: same prop names (`fill`,
// `stroke`, `stroke-width`, `color`, `stroke-dasharray`), same color formats
// (`#rgb` / `#rrggbb` / `#rrggbbaa` / `rgb()` / `rgba()` / 16 named colors), and
// the same two-pass resolve (classDef-via-`class` first, inline `style` on top).

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
    fn resolve(&self, classes: &mut [Class]) {
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

// ── Model ───────────────────────────────────────────────────────────────────

/// A class member — an attribute (no parens) or a method (ends in `(...)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// Full displayed text, including any visibility sigil (`+ - # ~`).
    pub text: String,
    /// `true` if this is a method (had `(...)`), `false` for an attribute.
    pub is_method: bool,
}

/// A parsed class: a name plus its attribute and method compartments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Class {
    /// The bare id used for relationship matching (generic suffix stripped).
    pub name: String,
    /// The displayed name in the name compartment. Differs from `name` when the
    /// class carries a generic suffix (e.g. id `List`, display `List<int>`).
    pub display_name: String,
    /// Stereotype/annotation, without the `<<` `>>` (e.g. `interface`). Rendered
    /// «interface» in italics above the class name. Last one wins.
    pub annotation: Option<String>,
    pub attributes: Vec<Member>,
    pub methods: Vec<Member>,
    /// Per-class style overrides (from `classDef`/`class`/`cssClass`/`style`/`:::`).
    pub style: ElemStyle,
    /// `click` interaction data: open URL (`link`), host callback name
    /// (`callback`), and hover `tooltip`. All `None` unless a `click`/`link`/
    /// `callback` directive targeted this class.
    pub link: Option<String>,
    pub callback: Option<String>,
    pub tooltip: Option<String>,
}

/// A note rectangle: either attached to a class (`note for Class "text"`) or
/// floating (`note "text"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// Note body text.
    pub text: String,
    /// The class id this note is attached to, if any (floating note → `None`).
    pub for_class: Option<String>,
}

/// Which UML marker a relationship carries and at which end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelMarker {
    /// `<|` hollow triangle (inheritance / realization).
    Triangle,
    /// `o` hollow diamond (aggregation).
    DiamondHollow,
    /// `*` filled diamond (composition).
    DiamondFilled,
    /// `>`/`<` open arrow (association / dependency).
    Arrow,
    /// Plain link, no marker.
    None,
}

/// A relationship between two classes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    /// Index/name of the left class (source as written).
    pub from: String,
    /// Index/name of the right class (target as written).
    pub to: String,
    /// The marker and the end it sits at.
    pub marker: RelMarker,
    /// `true` if the marker is at the `to` end, `false` if at the `from` end.
    pub marker_at_to: bool,
    /// Dashed line (`..`), e.g. dependency / realization.
    pub dashed: bool,
    /// Optional `: label`.
    pub label: Option<String>,
}

/// A parsed class diagram. `classes` is in first-seen order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClassDiagram {
    pub classes: Vec<Class>,
    pub relations: Vec<Relation>,
    pub notes: Vec<Note>,
}

impl ClassDiagram {
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
        self.classes.push(Class {
            name: name.to_string(),
            display_name: display.to_string(),
            ..Class::default()
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
fn split_generic(name: &str) -> (String, String) {
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
fn parse_member(raw: &str) -> Member {
    let text = generics_to_angles(raw.trim());
    // A method ends with a `)` somewhere (has a parameter list). Attributes have
    // no parentheses.
    let is_method = text.contains('(') && text.contains(')');
    Member { text, is_method }
}

/// Parse a relationship line into the two endpoint names, marker, and label.
/// Returns `None` if no relationship token is present.
fn parse_relation_line(line: &str) -> Option<Relation> {
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
    Some(Relation {
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
fn match_relation_earliest(s: &str) -> Option<(&'static str, RelMarker, bool, bool)> {
    const TOKENS: &[(&str, RelMarker, bool, bool)] = &[
        ("..|>", RelMarker::Triangle, true, true),
        ("<|..", RelMarker::Triangle, false, true),
        ("..>", RelMarker::Arrow, true, true),
        ("<..", RelMarker::Arrow, false, true),
        ("--|>", RelMarker::Triangle, true, false),
        ("<|--", RelMarker::Triangle, false, false),
        ("-->", RelMarker::Arrow, true, false),
        ("<--", RelMarker::Arrow, false, false),
        ("--*", RelMarker::DiamondFilled, true, false),
        ("*--", RelMarker::DiamondFilled, false, false),
        ("--o", RelMarker::DiamondHollow, true, false),
        ("o--", RelMarker::DiamondHollow, false, false),
        ("..", RelMarker::None, true, true),
        ("--", RelMarker::None, true, false),
    ];
    let mut best: Option<(usize, &'static str, RelMarker, bool, bool)> = None;
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
fn parse_note(line: &str) -> Option<Note> {
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
    Some(Note {
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
pub fn parse(src: &str) -> Result<ClassDiagram, String> {
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

    let mut diagram = ClassDiagram::default();
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

// ── Sizing ──────────────────────────────────────────────────────────────────

/// Geometry for one class box: total size + the y-offsets of compartment
/// dividers, measured from the box top.
struct BoxGeom {
    w: f32,
    h: f32,
    /// Height of the name band.
    name_h: f32,
    /// Height of the attribute band.
    attr_h: f32,
}

/// A blank compartment still gets a short band, like UML.
fn band_height(n: usize, line_h: f32, pad_y: f32) -> f32 {
    if n == 0 {
        // Empty band: half a line plus padding.
        line_h * 0.5 + pad_y
    } else {
        n as f32 * line_h + pad_y
    }
}

/// Compute the 3-compartment box geometry for a class.
fn box_geom(c: &Class, opts: &MermaidOptions) -> BoxGeom {
    let fs = opts.font_size_px;
    let line_h = fs * LINE_HEIGHT_EM;
    let pad_x = opts.node_padding_x;
    let pad_y = opts.node_padding_y;

    // Width: widest of the (display) name, any annotation, and every member.
    let mut max_w = text_size(&c.display_name, fs).0;
    if let Some(ann) = &c.annotation {
        max_w = max_w.max(text_size(&format!("«{ann}»"), fs).0);
    }
    for m in c.attributes.iter().chain(c.methods.iter()) {
        max_w = max_w.max(text_size(&m.text, fs).0);
    }
    let w = max_w + 2.0 * pad_x;

    // The name band gets a second line when a stereotype is present.
    let name_lines = if c.annotation.is_some() { 2.0 } else { 1.0 };
    let name_h = name_lines * line_h + pad_y;
    let attr_h = band_height(c.attributes.len(), line_h, pad_y);
    let method_h = band_height(c.methods.len(), line_h, pad_y);
    let h = name_h + attr_h + method_h;

    BoxGeom { w, h, name_h, attr_h }
}

// ── Layout ──────────────────────────────────────────────────────────────────

/// A class positioned by the layout engine.
struct Positioned {
    cx: f32,
    cy: f32,
    geom: BoxGeom,
    class_idx: usize,
}

/// A routed relationship.
struct RoutedRel {
    points: Vec<(f32, f32)>,
    rel_idx: usize,
    /// Position within its parallel group (unordered endpoint pair) and the
    /// group size, used to spread overlapping edge labels.
    label_index: usize,
    label_count: usize,
    /// Dagre's reserved label center, when it positioned one for this edge.
    dagre_label: Option<Vec2>,
}

struct Layout {
    boxes: Vec<Positioned>,
    rels: Vec<RoutedRel>,
    width: f32,
    height: f32,
}

fn layout(diagram: &ClassDiagram, opts: &MermaidOptions) -> Layout {
    let geoms: Vec<BoxGeom> = diagram.classes.iter().map(|c| box_geom(c, opts)).collect();
    let node_sizes: Vec<Vec2> = geoms.iter().map(|g| Vec2::new(g.w, g.h)).collect();

    // index_of for relationship endpoints.
    let mut index_of: std::collections::HashMap<&str, u32> =
        std::collections::HashMap::with_capacity(diagram.classes.len());
    for (i, c) in diagram.classes.iter().enumerate() {
        index_of.entry(c.name.as_str()).or_insert(i as u32);
    }

    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(diagram.relations.len());
    let mut kept: Vec<usize> = Vec::with_capacity(diagram.relations.len());
    // Per-edge label box size (aligned to `edges`) so dagre reserves a gap and
    // positions the label there; None for unlabeled relationships.
    let mut label_sizes: Vec<Option<Vec2>> = Vec::with_capacity(diagram.relations.len());
    for (j, r) in diagram.relations.iter().enumerate() {
        if let (Some(&a), Some(&b)) =
            (index_of.get(r.from.as_str()), index_of.get(r.to.as_str()))
        {
            edges.push((a, b));
            kept.push(j);
            label_sizes.push(
                r.label
                    .as_deref()
                    .filter(|l| !l.is_empty())
                    .map(|l| {
                        let (w, h) = text_size(l, opts.font_size_px);
                        Vec2::new(w + 10.0, h + 6.0)
                    }),
            );
        }
    }

    let engine = LayeredEngine {
        rankdir: RankDir::Tb,
        ranksep: opts.rank_sep,
        nodesep: opts.node_sep,
        edgesep: 20.0,
        default_node_size: Vec2::new(80.0, 60.0),
    };

    let out = engine.layout(&GraphInput {
        node_count: diagram.classes.len(),
        edges: &edges,
        node_sizes: Some(&node_sizes),
        edge_label_sizes: Some(&label_sizes),
        node_parents: None,
        directed: true,
    });

    let mut geoms = geoms;
    let boxes: Vec<Positioned> = (0..diagram.classes.len())
        .map(|i| {
            let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
            // move geom out
            let geom = std::mem::replace(
                &mut geoms[i],
                BoxGeom { w: 0.0, h: 0.0, name_h: 0.0, attr_h: 0.0 },
            );
            Positioned { cx: pos.x, cy: pos.y, geom, class_idx: i }
        })
        .collect();

    // Group edges by unordered endpoint pair so parallel / bidirectional
    // relationships spread their labels instead of stacking at one midpoint.
    let mut pair_members: std::collections::HashMap<(u32, u32), Vec<usize>> =
        std::collections::HashMap::new();
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

    let rels: Vec<RoutedRel> = kept
        .iter()
        .enumerate()
        .map(|(dagre_idx, &orig_idx)| {
            let points: Vec<(f32, f32)> = out
                .edge_routes
                .get(dagre_idx)
                .map(|r| r.iter().map(|p| (p.x, p.y)).collect())
                .unwrap_or_default();
            let (label_index, label_count) = group[dagre_idx];
            let dagre_label = out.edge_label_positions.get(dagre_idx).copied().flatten();
            RoutedRel {
                points,
                rel_idx: orig_idx,
                label_index,
                label_count,
                dagre_label,
            }
        })
        .collect();

    Layout {
        boxes,
        rels,
        width: out.size.x,
        height: out.size.y,
    }
}

// ── Draw ────────────────────────────────────────────────────────────────────

const STROKE_W: f32 = 1.5;
/// Marker triangle / diamond / arrow length, px.
const MARK_LEN: f32 = 12.0;
const MARK_HALF: f32 = 7.0;

fn fill_attrs(color: [u8; 4]) -> (String, String) {
    (rgb(color), opacity_attr("fill-opacity", color))
}
fn stroke_attrs(color: [u8; 4]) -> (String, String) {
    (rgb(color), opacity_attr("stroke-opacity", color))
}

/// Emit one class box (rect + dividers + three text bands).
fn emit_box(svg: &mut String, b: &Positioned, class: &Class, opts: &MermaidOptions) {
    let g = &b.geom;
    let x = b.cx - g.w / 2.0;
    let y = b.cy - g.h / 2.0;
    // Per-class style overrides, falling back to the theme defaults.
    let (fill, fo) = fill_attrs(class.style.fill.unwrap_or(opts.node_fill));
    let (stroke, so) = stroke_attrs(class.style.stroke.unwrap_or(opts.node_stroke));
    let sw = class.style.stroke_width.unwrap_or(STROKE_W);

    // Outer rect.
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
        w = g.w,
        h = g.h,
    );

    // Divider lines between compartments.
    let div1 = y + g.name_h;
    let div2 = y + g.name_h + g.attr_h;
    for dy in [div1, div2] {
        let _ = write!(
            svg,
            "<line x1=\"{x1:.2}\" y1=\"{dy:.2}\" x2=\"{x2:.2}\" y2=\"{dy:.2}\" \
             stroke=\"{stroke}\"{so} stroke-width=\"{sw}\"/>",
            x1 = x,
            x2 = x + g.w,
        );
    }

    // Name compartment. When a stereotype is present it sits centered in
    // italics ABOVE the (bold) class name, UML-style; the band is two lines tall.
    let (tfill, tfo) = fill_attrs(class.style.text_color.unwrap_or(opts.text_color));
    let family = escape(&opts.font_family);
    let fs = opts.font_size_px;
    let line_h = fs * LINE_HEIGHT_EM;
    let (ann_cy, name_cy) = if class.annotation.is_some() {
        let band_mid = y + g.name_h / 2.0;
        (band_mid - line_h / 2.0, band_mid + line_h / 2.0)
    } else {
        (0.0, y + g.name_h / 2.0)
    };
    if let Some(ann) = &class.annotation {
        let _ = write!(
            svg,
            "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{fs}\" font-style=\"italic\" fill=\"{tfill}\"{tfo}>{}</text>",
            escape(&format!("«{ann}»")),
            cx = b.cx,
            cy = ann_cy,
        );
    }
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" font-weight=\"bold\" fill=\"{tfill}\"{tfo}>{}</text>",
        escape(&class.display_name),
        cx = b.cx,
        cy = name_cy,
    );

    // Attributes (left-aligned) in the middle band.
    let text_x = x + opts.node_padding_x;
    let attr_top = div1;
    emit_lines(svg, &class.attributes, text_x, attr_top, line_h, opts, &tfill, &tfo, &family, fs);

    // Methods (left-aligned) in the bottom band.
    let method_top = div2;
    emit_lines(svg, &class.methods, text_x, method_top, line_h, opts, &tfill, &tfo, &family, fs);
}

#[allow(clippy::too_many_arguments)]
fn emit_lines(
    svg: &mut String,
    members: &[Member],
    x: f32,
    band_top: f32,
    line_h: f32,
    opts: &MermaidOptions,
    fill: &str,
    fo: &str,
    family: &str,
    fs: f32,
) {
    let pad_y = opts.node_padding_y;
    for (i, m) in members.iter().enumerate() {
        let cy = band_top + pad_y / 2.0 + line_h * (i as f32 + 0.5);
        let _ = write!(
            svg,
            "<text x=\"{x:.2}\" y=\"{cy:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}>{}</text>",
            escape(&m.text),
        );
    }
}

/// Pull the marker end of a polyline back by `amount` so the marker tip lands on
/// the box border. `at_to` trims the last point, else the first.
fn pullback(pts: &mut [(f32, f32)], at_to: bool, amount: f32) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let (tip_i, prev_i) = if at_to { (n - 1, n - 2) } else { (0, 1) };
    let (tx, ty) = pts[tip_i];
    let (px, py) = pts[prev_i];
    let (dx, dy) = (tx - px, ty - py);
    let len = dx.hypot(dy);
    if len <= amount || len == 0.0 {
        return;
    }
    let t = (len - amount) / len;
    pts[tip_i] = (px + dx * t, py + dy * t);
}

/// Emit a relationship polyline + its end marker + optional label.
fn emit_relation(svg: &mut String, r: &RoutedRel, rel: &Relation, opts: &MermaidOptions) {
    if r.points.len() < 2 {
        return;
    }
    let (stroke, so) = stroke_attrs(opts.edge_stroke);

    let mut pts = r.points.clone();
    // The marker sits at the from-end when `marker_at_to` is false. dagre routes
    // source→target, i.e. points[0] is `from`, last is `to`.
    let has_marker = rel.marker != RelMarker::None;
    if has_marker {
        pullback(&mut pts, rel.marker_at_to, MARK_LEN);
    }

    // Smooth curve through the (already marker-shortened) points; the marker is
    // drawn separately from the original un-shortened points below.
    let d = crate::svgutil::smooth_path_d(&pts);
    let dash = if rel.dashed { " stroke-dasharray=\"5 4\"" } else { "" };
    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"{dash}/>",
        d.trim_end(),
    );

    // Marker: oriented along the terminal segment at the marker end.
    if has_marker {
        // Use the un-pulled-back original points for the tip & direction.
        let (tip, prev) = if rel.marker_at_to {
            (r.points[r.points.len() - 1], r.points[r.points.len() - 2])
        } else {
            (r.points[0], r.points[1])
        };
        emit_marker(svg, rel.marker, tip, prev, opts);
    }

    // Label at dagre's reserved center when available; otherwise the route
    // midpoint, nudged perpendicular for parallel groups.
    if let Some(label) = &rel.label {
        if !label.is_empty() {
            let anchor = match r.dagre_label {
                Some(p) => Some((p.x, p.y)),
                None => {
                    edge_label_anchor(&r.points, r.label_index, r.label_count, opts.font_size_px)
                }
            };
            if let Some((mx, my)) = anchor {
                emit_label(svg, label, mx, my, opts);
            }
        }
    }
}

/// Draw the UML marker polygon at `tip`, pointing from `prev → tip`.
fn emit_marker(
    svg: &mut String,
    marker: RelMarker,
    tip: (f32, f32),
    prev: (f32, f32),
    opts: &MermaidOptions,
) {
    let (dx, dy) = (tip.0 - prev.0, tip.1 - prev.1);
    let len = dx.hypot(dy);
    let (ux, uy) = if len > 0.0 { (dx / len, dy / len) } else { (1.0, 0.0) };
    // perpendicular
    let (perpx, perpy) = (-uy, ux);

    let (stroke, so) = stroke_attrs(opts.edge_stroke);
    // Base point: back along the line from the tip by MARK_LEN.
    let base = (tip.0 - ux * MARK_LEN, tip.1 - uy * MARK_LEN);
    let half = MARK_HALF;
    let b1 = (base.0 + perpx * half, base.1 + perpy * half);
    let b2 = (base.0 - perpx * half, base.1 - perpy * half);

    match marker {
        RelMarker::Triangle => {
            // Hollow triangle (inheritance / realization): tip + two base corners,
            // filled with the canvas background so it reads as hollow on any theme.
            let _ = write!(
                svg,
                "<polygon points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" \
                 fill=\"{bg}\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
                tip.0, tip.1, b1.0, b1.1, b2.0, b2.1,
                bg = rgb(opts.background),
            );
        }
        RelMarker::Arrow => {
            // Open arrow: two strokes from base corners to tip (no fill).
            let _ = write!(
                svg,
                "<polyline points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" \
                 fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
                b1.0, b1.1, tip.0, tip.1, b2.0, b2.1,
            );
        }
        RelMarker::DiamondHollow | RelMarker::DiamondFilled => {
            // Diamond: tip, side1, far corner, side2. far = base extended one more
            // MARK_LEN back.
            let far = (tip.0 - ux * 2.0 * MARK_LEN, tip.1 - uy * 2.0 * MARK_LEN);
            let fill = if marker == RelMarker::DiamondFilled {
                stroke.clone()
            } else {
                rgb(opts.background)
            };
            let _ = write!(
                svg,
                "<polygon points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" \
                 fill=\"{fill}\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
                tip.0, tip.1, b1.0, b1.1, far.0, far.1, b2.0, b2.1,
            );
        }
        RelMarker::None => {}
    }
}

fn emit_label(svg: &mut String, label: &str, cx: f32, cy: f32, opts: &MermaidOptions) {
    let fs = opts.font_size_px;
    let (w, h) = text_size(label, fs);
    let pad = 2.0;
    let bw = w + 2.0 * pad;
    let bh = h + 2.0 * pad;
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{bw:.2}\" height=\"{bh:.2}\" \
         fill=\"{bg}\" fill-opacity=\"0.85\"/>",
        x = cx - bw / 2.0,
        y = cy - bh / 2.0,
        bg = rgb(opts.background),
    );
    let (tfill, tfo) = fill_attrs(opts.text_color);
    let family = escape(&opts.font_family);
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{tfill}\"{tfo}>{}</text>",
        escape(label),
    );
}

/// Note placement: gap between a class box and its attached note, and the
/// note's inner text padding.
const NOTE_GAP: f32 = 24.0;
const NOTE_PAD: f32 = 8.0;
/// Size of the folded "dog-ear" corner.
const NOTE_FOLD: f32 = 10.0;

/// Geometry for one positioned note rectangle.
struct NoteGeom {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    text: String,
}

/// Position every note. Attached notes sit to the right of their class box,
/// aligned to the box top; floating notes stack at the top-left. All notes are
/// laid out so their coordinates stay non-negative and the caller grows the
/// canvas to include them (no shifting of existing content).
fn layout_notes(diagram: &ClassDiagram, lay: &Layout, opts: &MermaidOptions) -> Vec<NoteGeom> {
    let fs = opts.font_size_px;
    let mut out = Vec::new();
    let mut float_y = 0.0_f32;
    for note in &diagram.notes {
        let (tw, th) = text_size(&note.text, fs);
        let w = tw + 2.0 * NOTE_PAD + NOTE_FOLD;
        let h = th + 2.0 * NOTE_PAD;
        let (x, y) = match &note.for_class {
            Some(id) => {
                // Find the attached class box; place the note to its right.
                let placed = lay
                    .boxes
                    .iter()
                    .find(|b| diagram.classes[b.class_idx].name == *id)
                    .map(|b| {
                        let g = &b.geom;
                        (b.cx + g.w / 2.0 + NOTE_GAP, b.cy - g.h / 2.0)
                    });
                placed.unwrap_or_else(|| {
                    let y = float_y;
                    float_y += h + NOTE_GAP;
                    (0.0, y)
                })
            }
            None => {
                let y = float_y;
                float_y += h + NOTE_GAP;
                (0.0, y)
            }
        };
        out.push(NoteGeom { x, y, w, h, text: note.text.clone() });
    }
    out
}

/// Emit a single note: a pale rectangle with a folded top-right corner and the
/// note text.
fn emit_note(svg: &mut String, n: &NoteGeom, opts: &MermaidOptions) {
    let (x, y, w, h) = (n.x, n.y, n.w, n.h);
    // Pale fill: blend the node fill toward the background a little. Use a
    // light, semi-distinct note color derived from the theme text color at low
    // opacity over the background, but keep it deterministic & simple: a fixed
    // pale yellow-ish tone that reads as a sticky note on any theme.
    let note_fill = "#fff5ad";
    let (stroke, so) = stroke_attrs(opts.node_stroke);
    let f = NOTE_FOLD;
    // Body outline with the top-right corner folded in.
    let _ = write!(
        svg,
        "<path d=\"M{x:.2},{y:.2} L{xr:.2},{y:.2} L{xrr:.2},{yf:.2} \
         L{xrr:.2},{yb:.2} L{x:.2},{yb:.2} Z\" \
         fill=\"{note_fill}\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        xr = x + w - f,
        xrr = x + w,
        yf = y + f,
        yb = y + h,
    );
    // The fold triangle.
    let _ = write!(
        svg,
        "<path d=\"M{xr:.2},{y:.2} L{xr:.2},{yf:.2} L{xrr:.2},{yf:.2} Z\" \
         fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        xr = x + w - f,
        xrr = x + w,
        yf = y + f,
    );
    // Text, left-aligned with padding, vertically centered.
    let (tfill, tfo) = fill_attrs(opts.text_color);
    let family = escape(&opts.font_family);
    let fs = opts.font_size_px;
    let _ = write!(
        svg,
        "<text x=\"{tx:.2}\" y=\"{ty:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{tfill}\"{tfo}>{}</text>",
        escape(&n.text),
        tx = x + NOTE_PAD,
        ty = y + h / 2.0,
    );
}

/// Final canvas size including any notes (notes only ever extend right/down).
fn canvas_size(diagram: &ClassDiagram, lay: &Layout, opts: &MermaidOptions) -> (f32, f32) {
    let notes = layout_notes(diagram, lay, opts);
    let mut w = lay.width;
    let mut h = lay.height;
    for n in &notes {
        w = w.max(n.x + n.w);
        h = h.max(n.y + n.h);
    }
    ((w.ceil() + 1.0).max(1.0), (h.ceil() + 1.0).max(1.0))
}

fn draw(diagram: &ClassDiagram, lay: &Layout, opts: &MermaidOptions) -> String {
    let notes = layout_notes(diagram, lay, opts);
    let (w, h) = canvas_size(diagram, lay, opts);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Relationships under boxes.
    for r in &lay.rels {
        emit_relation(&mut svg, r, &diagram.relations[r.rel_idx], opts);
    }
    // Class boxes on top.
    for b in &lay.boxes {
        emit_box(&mut svg, b, &diagram.classes[b.class_idx], opts);
    }
    // Notes on top of everything.
    for n in &notes {
        emit_note(&mut svg, n, opts);
    }

    svg.push_str("</svg>");
    svg
}

// ── Entry point ─────────────────────────────────────────────────────────────

/// Render a mermaid `classDiagram` to SVG.
pub fn render_class(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    Ok(render_class_inner(src, opts)?.0)
}

/// Like [`render_class`], but also returns one [`HitRegion`] per class box (its
/// drawn rect plus any `click`/`link`/`callback` data), in SVG-px coords. Used
/// by `render_with_regions` to make class diagrams interactive.
pub fn render_class_with_regions(
    src: &str,
    opts: &MermaidOptions,
) -> Result<(MermaidRender, Vec<HitRegion>), MermaidError> {
    render_class_inner(src, opts)
}

/// Shared pipeline for [`render_class`] / [`render_class_with_regions`]: parse →
/// layout → draw, deriving the hit regions from the very same positioned boxes.
fn render_class_inner(
    src: &str,
    opts: &MermaidOptions,
) -> Result<(MermaidRender, Vec<HitRegion>), MermaidError> {
    let diagram = parse(src).map_err(MermaidError::Parse)?;
    if diagram.classes.is_empty() {
        return Err(MermaidError::Empty);
    }
    let lay = layout(&diagram, opts);
    let svg = draw(&diagram, &lay, opts);
    let (width_px, height_px) = canvas_size(&diagram, &lay, opts);
    let regions: Vec<HitRegion> = lay
        .boxes
        .iter()
        .map(|b| {
            let class = &diagram.classes[b.class_idx];
            HitRegion {
                id: class.name.clone(),
                x: b.cx - b.geom.w / 2.0,
                y: b.cy - b.geom.h / 2.0,
                w: b.geom.w,
                h: b.geom.h,
                link: class.link.clone(),
                callback: class.callback.clone(),
                tooltip: class.tooltip.clone(),
            }
        })
        .collect();
    Ok((
        MermaidRender {
            svg,
            width_px,
            height_px,
        },
        regions,
    ))
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    // ---- parse ----

    #[test]
    fn parse_block_class() {
        let src = "classDiagram\nclass Animal {\n+int age\n+String name\n+void eat()\n}\n";
        let d = parse(src).unwrap();
        assert_eq!(d.classes.len(), 1);
        let c = &d.classes[0];
        assert_eq!(c.name, "Animal");
        assert_eq!(c.attributes.len(), 2);
        assert_eq!(c.methods.len(), 1);
        assert_eq!(c.attributes[0].text, "+int age");
        assert_eq!(c.methods[0].text, "+void eat()");
    }

    #[test]
    fn parse_member_lines() {
        let src = "classDiagram\nAnimal : +int age\nAnimal : +void eat()\n";
        let d = parse(src).unwrap();
        assert_eq!(d.classes.len(), 1);
        let c = &d.classes[0];
        assert_eq!(c.name, "Animal");
        assert_eq!(c.attributes.len(), 1);
        assert_eq!(c.methods.len(), 1);
    }

    #[test]
    fn attribute_vs_method_classification() {
        assert!(!parse_member("+int age").is_method);
        assert!(parse_member("+void eat()").is_method);
        assert!(parse_member("-doStuff(int x)").is_method);
        assert!(!parse_member("#name").is_method);
    }

    #[test]
    fn visibility_sigils_preserved() {
        let src = "classDiagram\nFoo : +pub\nFoo : -priv\nFoo : #prot\nFoo : ~pkg\n";
        let d = parse(src).unwrap();
        let texts: Vec<&str> = d.classes[0].attributes.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, vec!["+pub", "-priv", "#prot", "~pkg"]);
    }

    #[test]
    fn auto_create_classes() {
        let src = "classDiagram\nAnimal <|-- Dog\n";
        let d = parse(src).unwrap();
        assert_eq!(d.classes.len(), 2);
        assert!(d.classes.iter().any(|c| c.name == "Animal"));
        assert!(d.classes.iter().any(|c| c.name == "Dog"));
        assert_eq!(d.relations.len(), 1);
    }

    #[test]
    fn relationship_kinds_and_markers() {
        let cases = [
            ("classDiagram\nAnimal <|-- Dog\n", RelMarker::Triangle, false, false),
            ("classDiagram\nA --> B\n", RelMarker::Arrow, true, false),
            ("classDiagram\nA -- B\n", RelMarker::None, true, false),
            ("classDiagram\nA o-- B\n", RelMarker::DiamondHollow, false, false),
            ("classDiagram\nA *-- B\n", RelMarker::DiamondFilled, false, false),
            ("classDiagram\nA ..> B\n", RelMarker::Arrow, true, true),
            ("classDiagram\nA ..|> B\n", RelMarker::Triangle, true, true),
        ];
        for (src, marker, at_to, dashed) in cases {
            let d = parse(src).unwrap();
            assert_eq!(d.relations.len(), 1, "src={src}");
            let r = &d.relations[0];
            assert_eq!(r.marker, marker, "marker src={src}");
            assert_eq!(r.marker_at_to, at_to, "at_to src={src}");
            assert_eq!(r.dashed, dashed, "dashed src={src}");
        }
    }

    #[test]
    fn relationship_label() {
        let src = "classDiagram\nA --> B : uses\n";
        let d = parse(src).unwrap();
        assert_eq!(d.relations[0].label.as_deref(), Some("uses"));
    }

    #[test]
    fn dashed_detection() {
        let d = parse("classDiagram\nA ..> B\n").unwrap();
        assert!(d.relations[0].dashed);
        let d2 = parse("classDiagram\nA --> B\n").unwrap();
        assert!(!d2.relations[0].dashed);
    }

    #[test]
    fn bad_header_errors() {
        assert!(parse("flowchart TD\nA-->B").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn empty_diagram_renders_err_empty() {
        // Header but no classes.
        let r = render_class("classDiagram\n", &opts());
        assert_eq!(r, Err(MermaidError::Empty));
    }

    #[test]
    fn bad_header_render_err_parse() {
        let r = render_class("nonsense\nA-->B", &opts());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    // ---- render ----

    fn sample() -> &'static str {
        "classDiagram\n\
         class Animal {\n+int age\n+String name\n+void eat()\n}\n\
         class Dog {\n+bark()\n}\n\
         Animal <|-- Dog\n"
    }

    #[test]
    fn renders_svg_envelope() {
        let r = render_class(sample(), &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn one_box_per_class_with_compartments() {
        let r = render_class(sample(), &opts()).unwrap();
        // 2 classes → 2 outer <rect> (label backgrounds are only for edge labels;
        // none here).
        assert_eq!(r.svg.matches("<rect").count(), 2);
        // Each class has >= 2 separator lines → >= 4 total.
        assert!(r.svg.matches("<line").count() >= 4);
    }

    #[test]
    fn member_text_present() {
        let r = render_class(sample(), &opts()).unwrap();
        assert!(r.svg.contains("+int age"));
        assert!(r.svg.contains("+void eat()"));
        assert!(r.svg.contains("+bark()"));
        assert!(r.svg.contains(">Animal<"));
        assert!(r.svg.contains(">Dog<"));
    }

    #[test]
    fn one_polyline_per_relationship() {
        let r = render_class(sample(), &opts()).unwrap();
        // The relationship line is a <path fill="none">.
        assert_eq!(r.svg.matches("fill=\"none\"").count() >= 1, true);
        // Inheritance → hollow triangle polygon present.
        assert!(r.svg.contains("<polygon"));
    }

    #[test]
    fn dashed_relationship_drawn_dashed() {
        let src = "classDiagram\nclass A\nclass B\nA ..|> B\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains("stroke-dasharray"));
    }

    #[test]
    fn markers_per_kind() {
        // arrow → polyline (open), diamond → polygon, triangle → polygon.
        let arrow = render_class("classDiagram\nA --> B\n", &opts()).unwrap();
        // edge path + arrow polyline both have fill="none". Arrow is a polyline.
        assert!(arrow.svg.contains("<polyline"));

        let comp = render_class("classDiagram\nA *-- B\n", &opts()).unwrap();
        assert!(comp.svg.contains("<polygon"));
        // filled diamond uses the edge-stroke color as fill (not white).
        assert!(comp.svg.contains(&rgb(opts().edge_stroke)));
    }

    #[test]
    fn relationship_label_rendered() {
        let r = render_class("classDiagram\nA --> B : uses\n", &opts()).unwrap();
        assert!(r.svg.contains(">uses<"));
    }

    #[test]
    fn bidirectional_relationship_labels_separated() {
        // A→B and B→A both labeled: labels must land at distinct anchors.
        let src = "classDiagram\nA --> B : up\nB --> A : down\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains(">up<"));
        assert!(r.svg.contains(">down<"));

        // Read the (x, y) anchor of each label's <text> element. The two must
        // differ in at least one coordinate (the route is vertical here so the
        // perpendicular nudge moves x).
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
        let up = label_xy(&r.svg, "up");
        let down = label_xy(&r.svg, "down");
        assert!(
            (up.0 - down.0).abs() > 1.0 || (up.1 - down.1).abs() > 1.0,
            "bidirectional labels overlap: up={up:?}, down={down:?}"
        );
    }

    #[test]
    fn xml_escapes_member_text() {
        let src = "classDiagram\nFoo : +Map<K,V> data\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains("+Map&lt;K,V&gt; data"));
        assert!(!r.svg.contains("+Map<K,V>"));
    }

    #[test]
    fn empty_compartments_still_have_separators() {
        // A class with no members still gets 3 compartments / 2 separators.
        let r = render_class("classDiagram\nclass Lonely\n", &opts()).unwrap();
        assert_eq!(r.svg.matches("<line").count(), 2);
        assert_eq!(r.svg.matches("<rect").count(), 1);
    }

    #[test]
    fn deterministic() {
        let a = render_class(sample(), &opts()).unwrap();
        let b = render_class(sample(), &opts()).unwrap();
        assert_eq!(a, b);
    }

    // ---- styling directives ----

    fn style_of<'a>(d: &'a ClassDiagram, name: &str) -> &'a ElemStyle {
        &d.classes.iter().find(|c| c.name == name).expect("class").style
    }

    #[test]
    fn classdef_and_class_apply() {
        let src = "classDiagram\nclass Animal\nclass Dog\nclassDef hot fill:#f00\nclass Animal hot\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Animal").fill, Some([255, 0, 0, 255]));
        // Dog untouched.
        assert_eq!(style_of(&d, "Dog").fill, None);
    }

    #[test]
    fn classdef_defined_after_class_resolves() {
        // Two-pass: the `class` assignment references `hot` before its classDef
        // appears later in the source.
        let src = "classDiagram\nclass Animal\nclass Animal hot\nclassDef hot fill:#0f0\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Animal").fill, Some([0, 255, 0, 255]));
    }

    #[test]
    fn triple_colon_shorthand() {
        let src = "classDiagram\nAnimal:::hot <|-- Dog\nclassDef hot fill:#f00\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Animal").fill, Some([255, 0, 0, 255]));
        // Relationship still recorded with the bare name.
        assert_eq!(d.relations.len(), 1);
        assert_eq!(d.relations[0].from, "Animal");
        assert_eq!(d.relations[0].to, "Dog");
    }

    #[test]
    fn triple_colon_bare_class() {
        let src = "classDiagram\nclass Animal\nAnimal:::hot\nclassDef hot fill:#00f\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Animal").fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn cssclass_directive() {
        let src = "classDiagram\nclass Animal\nclassDef hot fill:#f00\ncssClass \"Animal\" hot\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Animal").fill, Some([255, 0, 0, 255]));
    }

    #[test]
    fn style_directive_direct_and_overrides_class() {
        let src = "classDiagram\nclass Dog\nstyle Dog fill:#0f0\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Dog").fill, Some([0, 255, 0, 255]));

        // Inline `style` wins over `class`.
        let src2 = "classDiagram\nclass Animal\nclassDef hot fill:#f00\nclass Animal hot\nstyle Animal fill:#00f\n";
        let d2 = parse(src2).unwrap();
        assert_eq!(style_of(&d2, "Animal").fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn style_override_in_rendered_svg() {
        // Animal's box rect should carry the override fill color.
        let src = "classDiagram\nclass Animal\nclass Dog\nclassDef hot fill:#f00\nclass Animal hot\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains(&rgb([255, 0, 0, 255])), "override fill present: {}", r.svg);
        // Default node fill still appears (Dog uses it).
        assert!(r.svg.contains(&rgb(opts().node_fill)));
    }

    #[test]
    fn unstyled_classes_unchanged() {
        // No directives → every class style is default.
        let d = parse(sample()).unwrap();
        for c in &d.classes {
            assert_eq!(c.style, ElemStyle::default());
        }
    }

    #[test]
    fn all_relationship_kinds_render() {
        let src = "classDiagram\n\
            A <|-- B\nA --> C\nA -- D\nA o-- E\nA *-- F\nA ..> G\nA ..|> H\n";
        let r = render_class(src, &opts()).unwrap();
        // 8 classes A..H.
        assert_eq!(r.svg.matches("<rect").count(), 8);
        assert!(r.svg.starts_with("<svg"));
    }

    // ---- generics ----

    #[test]
    fn split_generic_forms() {
        assert_eq!(split_generic("List~int~"), ("List".into(), "List<int>".into()));
        assert_eq!(split_generic("Map~K, V~"), ("Map".into(), "Map<K, V>".into()));
        assert_eq!(split_generic("Plain"), ("Plain".into(), "Plain".into()));
    }

    #[test]
    fn generic_class_id_vs_display() {
        let d = parse("classDiagram\nclass List~int~\n").unwrap();
        assert_eq!(d.classes.len(), 1);
        assert_eq!(d.classes[0].name, "List");
        assert_eq!(d.classes[0].display_name, "List<int>");
    }

    #[test]
    fn generic_class_renders_angle_brackets() {
        let r = render_class("classDiagram\nclass List~int~\n", &opts()).unwrap();
        // Display name rendered as List<int> (XML-escaped).
        assert!(r.svg.contains("List&lt;int&gt;"));
    }

    #[test]
    fn generic_relationship_links_base_class() {
        // `List~int~ --> Item` must link the `List` class id, not `List~int~`.
        let d = parse("classDiagram\nList~int~ --> Item\n").unwrap();
        assert_eq!(d.relations.len(), 1);
        assert_eq!(d.relations[0].from, "List");
        assert_eq!(d.relations[0].to, "Item");
        assert!(d.classes.iter().any(|c| c.name == "List"));
        assert!(d.classes.iter().any(|c| c.name == "Item"));
    }

    #[test]
    fn generic_definition_plus_relationship_shares_class() {
        // A `class List~int~` definition gives the display; the relationship
        // (matched on the base id `List`) links the same class.
        let src = "classDiagram\nclass List~int~\nList~int~ --> Item\n";
        let d = parse(src).unwrap();
        let list = d.classes.iter().find(|c| c.name == "List").unwrap();
        assert_eq!(list.display_name, "List<int>");
        assert_eq!(d.relations[0].from, "List");
        // Only one List class (definition + relationship endpoint merged).
        assert_eq!(d.classes.iter().filter(|c| c.name == "List").count(), 1);
    }

    #[test]
    fn generic_member_renders_angles() {
        let d = parse("classDiagram\nclass Box {\n+List~int~ items\n}\n").unwrap();
        assert_eq!(d.classes[0].attributes[0].text, "+List<int> items");
        let r = render_class("classDiagram\nclass Box {\n+List~int~ items\n}\n", &opts()).unwrap();
        assert!(r.svg.contains("+List&lt;int&gt; items"));
    }

    #[test]
    fn generic_with_space_is_definition_not_directive() {
        let d = parse("classDiagram\nclass Map~K, V~\n").unwrap();
        assert_eq!(d.classes.len(), 1);
        assert_eq!(d.classes[0].name, "Map");
        assert_eq!(d.classes[0].display_name, "Map<K, V>");
    }

    // ---- annotations / stereotypes ----

    #[test]
    fn annotation_in_body() {
        let src = "classDiagram\nclass Shape {\n<<interface>>\n+area() float\n}\n";
        let d = parse(src).unwrap();
        let shape = d.classes.iter().find(|c| c.name == "Shape").unwrap();
        assert_eq!(shape.annotation.as_deref(), Some("interface"));
        // The annotation line is NOT a member.
        assert_eq!(shape.methods.len(), 1);
        assert_eq!(shape.attributes.len(), 0);
    }

    #[test]
    fn annotation_standalone() {
        let d = parse("classDiagram\n<<interface>> Shape\n").unwrap();
        let shape = d.classes.iter().find(|c| c.name == "Shape").unwrap();
        assert_eq!(shape.annotation.as_deref(), Some("interface"));
    }

    #[test]
    fn annotation_renders_guillemets_above_name() {
        let r = render_class("classDiagram\n<<interface>> Shape\n", &opts()).unwrap();
        assert!(r.svg.contains("«interface»"));
        assert!(r.svg.contains("font-style=\"italic\""));
        assert!(r.svg.contains(">Shape<"));
    }

    #[test]
    fn annotation_in_body_renders() {
        let src = "classDiagram\nclass Shape {\n<<interface>>\n+area() float\n}\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains("«interface»"));
        assert!(r.svg.contains("+area() float"));
    }

    // ---- notes ----

    #[test]
    fn note_for_class_parsed() {
        let d = parse("classDiagram\nclass Shape\nnote for Shape \"important\"\n").unwrap();
        assert_eq!(d.notes.len(), 1);
        assert_eq!(d.notes[0].text, "important");
        assert_eq!(d.notes[0].for_class.as_deref(), Some("Shape"));
    }

    #[test]
    fn note_for_class_renders() {
        let src = "classDiagram\nclass Shape\nnote for Shape \"important\"\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains(">important<"));
        // Note body is drawn as a path (folded-corner rectangle).
        assert!(r.svg.contains("#fff5ad"));
    }

    #[test]
    fn floating_note_parsed_and_renders() {
        let src = "classDiagram\nclass A\nnote \"floating text\"\n";
        let d = parse(src).unwrap();
        assert_eq!(d.notes.len(), 1);
        assert!(d.notes[0].for_class.is_none());
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains(">floating text<"));
    }

    #[test]
    fn note_xml_escaped() {
        let src = "classDiagram\nclass A\nnote for A \"a < b & c\"\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains("a &lt; b &amp; c"));
    }

    #[test]
    fn simple_diagrams_unchanged_no_extras() {
        // A diagram with no generics/annotations/notes: display_name == name,
        // no annotation, no notes.
        let d = parse(sample()).unwrap();
        assert!(d.notes.is_empty());
        for c in &d.classes {
            assert_eq!(c.display_name, c.name);
            assert!(c.annotation.is_none());
        }
    }

    // ---- click / interaction ----

    #[test]
    fn click_sets_link_and_tooltip() {
        let src = "classDiagram\nclass Animal\nclick Animal \"https://x\" \"tip\"\n";
        let d = parse(src).unwrap();
        let a = d.classes.iter().find(|c| c.name == "Animal").unwrap();
        assert_eq!(a.link.as_deref(), Some("https://x"));
        assert_eq!(a.tooltip.as_deref(), Some("tip"));
        assert!(a.callback.is_none());
    }

    #[test]
    fn click_call_and_unknown_id() {
        // `call name(args)` → callback; unknown id is skipped, not fabricated.
        let src = "classDiagram\nclass Animal\nclick Animal call doThing() \"hi\"\nclick Ghost \"https://y\"\n";
        let d = parse(src).unwrap();
        assert_eq!(d.classes.len(), 1);
        let a = &d.classes[0];
        assert_eq!(a.callback.as_deref(), Some("doThing"));
        assert_eq!(a.tooltip.as_deref(), Some("hi"));
    }

    #[test]
    fn regions_carry_click_data() {
        let src = "classDiagram\nclass Animal\nclass Dog\nAnimal <|-- Dog\nclick Animal \"https://x\" \"tip\"\n";
        let (render, regions) = render_class_with_regions(src, &opts()).unwrap();
        assert_eq!(regions.len(), 2);
        let a = regions.iter().find(|r| r.id == "Animal").unwrap();
        assert_eq!(a.link.as_deref(), Some("https://x"));
        assert_eq!(a.tooltip.as_deref(), Some("tip"));
        assert!(a.w > 0.0 && a.h > 0.0);
        assert!(a.x >= 0.0 && a.y >= 0.0);
        assert!(a.x + a.w <= render.width_px + 1.0);
        assert!(a.y + a.h <= render.height_px + 1.0);
        // Dog has no click.
        let dog = regions.iter().find(|r| r.id == "Dog").unwrap();
        assert!(dog.link.is_none() && dog.callback.is_none() && dog.tooltip.is_none());
    }

    #[test]
    fn regions_without_click_and_svg_unchanged() {
        let src = "classDiagram\nclass Animal\nclass Dog\nAnimal <|-- Dog\n";
        let plain = render_class(src, &opts()).unwrap();
        let (with_regions, regions) = render_class_with_regions(src, &opts()).unwrap();
        // Regions per node, all click-free.
        assert_eq!(regions.len(), 2);
        assert!(regions.iter().all(|r| r.link.is_none()
            && r.callback.is_none()
            && r.tooltip.is_none()
            && r.w > 0.0
            && r.h > 0.0));
        // `render` output byte-identical.
        assert_eq!(plain.svg, with_regions.svg);
        assert_eq!(plain.width_px, with_regions.width_px);
        assert_eq!(plain.height_px, with_regions.height_px);
    }
}
