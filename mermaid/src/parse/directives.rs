//! Flowchart styling directives: `classDef`/`class`/`style`/`linkStyle`/`click`
//! collection during parsing and the two-pass resolve onto nodes/edges, plus the
//! shared CSS-ish color/width/prop parsing. Kept separate from the node/edge
//! token grammar so styling evolves on its own.

use std::collections::HashMap;

use crate::model::{ElemStyle, FlowChart, FlowNode, NodeShape};

/// Directive state collected during parsing, resolved onto nodes/edges at the
/// end of `parse_flowchart` (two-pass: classDefs may be defined after the
/// `class`/`:::` statements that reference them).
#[derive(Default)]
pub(super) struct Directives {
    /// Named `classDef` styles.
    pub(super) class_defs: HashMap<String, ElemStyle>,
    /// `(node id, class name)` assignments from `class A,B name` and `A:::name`.
    pub(super) class_assignments: Vec<(String, String)>,
    /// Inline `style <id> ...` overrides applied directly to a node.
    pub(super) node_inline: Vec<(String, ElemStyle)>,
    /// `linkStyle <n> ...` overrides, keyed by 0-based edge index.
    pub(super) edge_inline: Vec<(usize, ElemStyle)>,
    /// `linkStyle default ...` overrides applied to every edge.
    pub(super) edge_default: Vec<ElemStyle>,
    /// `click <id> ...` interaction directives, resolved onto nodes at the end.
    /// `(node id, link, callback, tooltip)`; the node is auto-created if absent.
    pub(super) clicks: Vec<ClickDirective>,
}

/// One parsed `click` directive (interaction data for a node).
pub(super) struct ClickDirective {
    pub(super) id: String,
    pub(super) link: Option<String>,
    pub(super) callback: Option<String>,
    pub(super) tooltip: Option<String>,
}

/// Try to parse `stmt` as a styling directive, recording it into `dir`. Returns
/// `true` if it was a (recognized) directive keyword line and should not be
/// treated as a node/edge statement.
pub(super) fn parse_directive(stmt: &str, dir: &mut Directives) -> bool {
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
pub(super) fn resolve_styles(chart: &mut FlowChart, dir: &Directives) {
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
pub(crate) fn merge_style(dst: &mut ElemStyle, src: &ElemStyle) {
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
    if src.opacity.is_some() {
        dst.opacity = src.opacity;
    }
    if src.font_weight.is_some() {
        dst.font_weight = src.font_weight.clone();
    }
    if src.font_style.is_some() {
        dst.font_style = src.font_style.clone();
    }
    if src.text_decoration.is_some() {
        dst.text_decoration = src.text_decoration.clone();
    }
    if src.font_size.is_some() {
        dst.font_size = src.font_size;
    }
}

/// Parse a comma-separated `prop:val,prop:val,...` list into an [`ElemStyle`].
/// Unknown props or unparseable colors are skipped leniently.
pub(crate) fn parse_style_props(props: &str) -> ElemStyle {
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
            "opacity" => {
                if let Ok(o) = val.parse::<f32>() {
                    style.opacity = Some(o.clamp(0.0, 1.0));
                }
            }
            "font-weight" => {
                if !val.is_empty() {
                    style.font_weight = Some(val.to_string());
                }
            }
            "font-style" => {
                if !val.is_empty() {
                    style.font_style = Some(val.to_string());
                }
            }
            "text-decoration" => {
                if !val.is_empty() {
                    style.text_decoration = Some(val.to_string());
                }
            }
            "font-size" => {
                if let Some(s) = parse_width(val) {
                    style.font_size = Some(s);
                }
            }
            _ => {}
        }
    }
    style
}

/// Parse a stroke width like `2px` / `4` / `1.5` into an f32 (px).
pub(super) fn parse_width(val: &str) -> Option<f32> {
    let v = val.trim().trim_end_matches("px").trim();
    v.parse::<f32>().ok()
}

/// Parse a CSS-ish color into straight RGBA: `#rgb`, `#rrggbb`, `#rrggbbaa`,
/// `rgb(r,g,b)`, `rgba(r,g,b,a)`, or a small set of named colors. Returns `None`
/// on anything unrecognized so the caller can skip the prop.
pub(crate) fn parse_color(s: &str) -> Option<[u8; 4]> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_existing_props() {
        let s = parse_style_props("fill:#f00,stroke:#00f,stroke-width:3px,color:#0f0,stroke-dasharray:5 5");
        assert_eq!(s.fill, Some([255, 0, 0, 255]));
        assert_eq!(s.stroke, Some([0, 0, 255, 255]));
        assert_eq!(s.stroke_width, Some(3.0));
        assert_eq!(s.text_color, Some([0, 255, 0, 255]));
        assert!(s.dashed);
    }

    #[test]
    fn parses_opacity() {
        assert_eq!(parse_style_props("opacity:0.4").opacity, Some(0.4));
        // Out-of-range opacity is clamped to 0..1.
        assert_eq!(parse_style_props("opacity:2.0").opacity, Some(1.0));
    }

    #[test]
    fn parses_font_weight() {
        assert_eq!(
            parse_style_props("font-weight:bold").font_weight.as_deref(),
            Some("bold")
        );
    }

    #[test]
    fn parses_font_style() {
        assert_eq!(
            parse_style_props("font-style:italic").font_style.as_deref(),
            Some("italic")
        );
    }

    #[test]
    fn parses_text_decoration() {
        assert_eq!(
            parse_style_props("text-decoration:underline").text_decoration.as_deref(),
            Some("underline")
        );
    }

    #[test]
    fn parses_font_size_strips_px() {
        assert_eq!(parse_style_props("font-size:20px").font_size, Some(20.0));
        assert_eq!(parse_style_props("font-size:18").font_size, Some(18.0));
    }

    #[test]
    fn unknown_prop_ignored() {
        let s = parse_style_props("flibberty:wobble,font-weight:bold");
        assert_eq!(s.font_weight.as_deref(), Some("bold"));
        // Nothing else set by the bogus prop.
        assert!(s.fill.is_none() && s.opacity.is_none());
    }

    #[test]
    fn merge_overrides_new_props() {
        let mut dst = ElemStyle {
            font_weight: Some("normal".to_string()),
            opacity: Some(1.0),
            ..Default::default()
        };
        let src = ElemStyle {
            font_weight: Some("bold".to_string()),
            opacity: Some(0.5),
            font_size: Some(24.0),
            ..Default::default()
        };
        merge_style(&mut dst, &src);
        assert_eq!(dst.font_weight.as_deref(), Some("bold"));
        assert_eq!(dst.opacity, Some(0.5));
        assert_eq!(dst.font_size, Some(24.0));
    }
}

