//! `c4` diagram (`C4Context` / `C4Container` / `C4Component` / `C4Dynamic` /
//! `C4Deployment`).
//!
//! Self-contained: parse → build a `hiker_graph` layered (dagre) graph → lay
//! out → draw one SVG document. Like flowchart/state/requirement, each C4
//! element becomes a node and each relationship a directed edge.
//!
//! ## Supported subset
//!
//! Header (any of): `C4Context`, `C4Container`, `C4Component`, `C4Dynamic`,
//! `C4Deployment`.
//!
//! **Elements** (comma-separated, quoted args):
//! * `Person(id, "label", "desc"?)`, `Person_Ext(...)`.
//! * `System(id, "label", "desc"?)`, `System_Ext(...)`, `SystemDb(...)`,
//!   `SystemDb_Ext(...)`, `SystemQueue(...)`, `SystemQueue_Ext(...)`.
//! * `Container(id, "label", "tech"?, "desc"?)`, `ContainerDb(...)`,
//!   `ContainerQueue(...)` plus their `_Ext` variants.
//! * `Component(id, "label", "tech"?, "desc"?)`, `ComponentDb(...)`,
//!   `ComponentQueue(...)` plus their `_Ext` variants.
//!
//! The first arg is the id; the remaining quoted args are the label, the
//! (container/component-only) technology, and the description. Each element is
//! mapped to a [`ElemKind`] (Person / System / Container / Component) and an
//! `external` flag.
//!
//! **Relationships**: `Rel(from, to, "label", "tech"?)` plus the directional
//! variants `Rel_U`/`Rel_D`/`Rel_L`/`Rel_R` (a.k.a. `Rel_Up`/`Rel_Down`/…),
//! `Rel_Back`, and `BiRel(from, to, "label", "tech"?)`. `BiRel` adds the reverse
//! edge as well. The direction suffix is parsed but does not steer the layout
//! (rankdir stays Tb).
//!
//! **Boundaries**: `System_Boundary(id, "label") { … }`,
//! `Enterprise_Boundary(...)`, `Container_Boundary(...)`, and bare
//! `Boundary(...)`. Each becomes a dagre cluster (container node) enclosing the
//! elements declared inside its `{ … }` block; nesting is supported. After
//! layout the boundary's bounding rectangle is drawn as a dashed, faintly
//! filled rectangle with a `«System»` / `«Enterprise»` / `«Container»` type
//! label and the boundary name at the top-left, drawn behind the element boxes
//! (outermost boundaries first). Inner elements/relationships are still parsed
//! and laid out as usual.
//!
//! **Skipped** (noted): `UpdateElementStyle`, `UpdateRelStyle`,
//! `UpdateLayoutConfig`, tags, sprites/icons, `RelIndex`, and deployment-node
//! nesting (`Deployment_Node`/`Node`/`Node_L`/`Node_R` are treated as plain
//! container-like boxes via their id/label, not nested).

use std::collections::HashMap;
use std::fmt::Write as _;

use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb};
use crate::{MermaidError, MermaidOptions, MermaidRender};

/// The broad category of a C4 element, which drives the type line and fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ElemKind {
    Person,
    System,
    Container,
    Component,
}

impl ElemKind {
    /// The bracketed type label, e.g. `[Person]` / `[Container: tech]`.
    fn type_label(self, external: bool, tech: &str) -> String {
        let base = match (self, external) {
            (ElemKind::Person, false) => "Person",
            (ElemKind::Person, true) => "External Person",
            (ElemKind::System, false) => "Software System",
            (ElemKind::System, true) => "External System",
            (ElemKind::Container, _) => "Container",
            (ElemKind::Component, _) => "Component",
        };
        if tech.is_empty() {
            format!("[{base}]")
        } else {
            format!("[{base}: {tech}]")
        }
    }
}

/// A parsed C4 element (becomes one layout node and one drawn box).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Element {
    /// The id (first arg) used to reference this element in relationships.
    id: String,
    /// The display name (bold first line).
    label: String,
    /// The technology string (container/component only); empty otherwise.
    tech: String,
    /// The wrapped description; empty if absent.
    descr: String,
    kind: ElemKind,
    external: bool,
}

/// A directed relationship `from → to`, with an optional technology suffix.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Relationship {
    from: String,
    to: String,
    label: String,
    tech: String,
}

/// The category of a boundary, which drives its `«…»` type label and the
/// default when the optional `type` arg is omitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryKind {
    System,
    Enterprise,
    Container,
    /// Bare `Boundary(...)` — generic.
    Generic,
}

impl BoundaryKind {
    /// The `«…»` type label shown at the boundary's top-left.
    fn type_label(self) -> &'static str {
        match self {
            BoundaryKind::System => "«System»",
            BoundaryKind::Enterprise => "«Enterprise»",
            BoundaryKind::Container => "«Container»",
            BoundaryKind::Generic => "«Boundary»",
        }
    }
}

/// A parsed boundary block (becomes one dagre container node and one drawn
/// dashed rectangle).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Boundary {
    /// The id (first arg).
    id: String,
    /// The display name (defaults to the id when absent).
    label: String,
    kind: BoundaryKind,
    /// Index into `C4Diagram::boundaries` of the enclosing boundary, if nested.
    parent: Option<usize>,
    /// Ids of elements declared directly inside this boundary's `{ … }`.
    member_elems: Vec<String>,
}

/// A parsed C4 diagram.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct C4Diagram {
    elements: Vec<Element>,
    relationships: Vec<Relationship>,
    boundaries: Vec<Boundary>,
}

/// The set of accepted header keywords.
fn is_header(kw: &str) -> bool {
    matches!(
        kw,
        "C4Context" | "C4Container" | "C4Component" | "C4Dynamic" | "C4Deployment"
    )
}

/// Map an element keyword to its `(kind, external)`, if it is one. Database /
/// queue variants collapse onto their base kind (we don't draw distinct
/// cylinder/queue shapes in v1).
fn element_keyword(kw: &str) -> Option<(ElemKind, bool)> {
    Some(match kw {
        "Person" => (ElemKind::Person, false),
        "Person_Ext" => (ElemKind::Person, true),

        "System" | "SystemDb" | "SystemQueue" => (ElemKind::System, false),
        "System_Ext" | "SystemDb_Ext" | "SystemQueue_Ext" => (ElemKind::System, true),

        "Container" | "ContainerDb" | "ContainerQueue" => (ElemKind::Container, false),
        "Container_Ext" | "ContainerDb_Ext" | "ContainerQueue_Ext" => (ElemKind::Container, true),

        "Component" | "ComponentDb" | "ComponentQueue" => (ElemKind::Component, false),
        "Component_Ext" | "ComponentDb_Ext" | "ComponentQueue_Ext" => (ElemKind::Component, true),

        // Deployment nodes: treated as plain container-like boxes.
        "Deployment_Node" | "Node" | "Node_L" | "Node_R" => (ElemKind::Container, false),

        _ => return None,
    })
}

/// Whether a keyword carries a technology arg (between label and description):
/// containers, components, and deployment nodes do; persons and systems don't.
fn keyword_has_tech(kind: ElemKind) -> bool {
    matches!(kind, ElemKind::Container | ElemKind::Component)
}

/// Map a relationship keyword to `Some(is_bidirectional)`, if it is one.
fn relationship_keyword(kw: &str) -> Option<bool> {
    Some(match kw {
        "Rel" => false,
        "BiRel" => true,
        "Rel_U" | "Rel_Up" => false,
        "Rel_D" | "Rel_Down" => false,
        "Rel_L" | "Rel_Left" => false,
        "Rel_R" | "Rel_Right" => false,
        "Rel_Back" | "Rel_B" => false,
        _ => return None,
    })
}

/// Split the keyword off a statement like `Person(a, "b")`. Returns
/// `(keyword, args_str)` where `args_str` is the text inside the outer parens.
/// `None` if there's no `(...)`.
fn split_call(line: &str) -> Option<(&str, &str)> {
    let open = line.find('(')?;
    // Match the final close paren so trailing `{` (boundary openers) is excluded
    // by the caller before this is reached.
    let close = line.rfind(')')?;
    if close <= open {
        return None;
    }
    let kw = line[..open].trim();
    let args = &line[open + 1..close];
    if kw.is_empty() {
        return None;
    }
    Some((kw, args))
}

/// Split a comma-separated argument list, honoring double quotes (commas inside
/// quotes are kept). Surrounding quotes on each arg are stripped; bare args are
/// trimmed. Empty trailing/positional args are preserved as empty strings.
fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = args.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                // Keep the quote marker out of the value; quotes are stripped.
            }
            ',' if !in_quotes => {
                out.push(cur.trim().to_string());
                cur = String::new();
            }
            _ => cur.push(c),
        }
        let _ = chars.peek();
    }
    out.push(cur.trim().to_string());
    out
}

/// Word-wrap `text` to at most `max_chars` per line (greedy by words). Returns
/// the lines joined with `\n`. Long single words are left intact.
fn wrap(text: &str, max_chars: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    let max = max_chars.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.chars().count() + 1 + word.chars().count() <= max {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines.join("\n")
}

/// Description wrap width, in characters.
const WRAP_CHARS: usize = 28;

/// Parse a C4-diagram source. Errors on a missing/wrong header.
fn parse(src: &str) -> Result<C4Diagram, String> {
    let mut diag = C4Diagram::default();
    let mut seen_ids: HashMap<String, ()> = HashMap::new();
    let mut pending_header = true;
    // Stack of currently-open boundary indices (into `diag.boundaries`). The top
    // is the boundary that newly-declared elements / boundaries belong to.
    let mut boundary_stack: Vec<usize> = Vec::new();

    for raw in src.lines() {
        // Strip `%%` comments and surrounding whitespace.
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if pending_header {
            let kw = line.split_whitespace().next().unwrap_or("");
            if !is_header(kw) {
                return Err(format!("expected a C4 header, got {kw:?}"));
            }
            pending_header = false;
            continue;
        }

        // Boundary close brace(s): pop the innermost open boundary. A line may be
        // a bare `}` (possibly several) — pop one per `}`.
        if line.chars().all(|c| c == '}') {
            for _ in 0..line.chars().count() {
                boundary_stack.pop();
            }
            continue;
        }

        // Boundary opener: `<kind>_Boundary(id, "label") {`. We strip a trailing
        // `{` and remember that an opener introduced a block.
        let opens_block = line.ends_with('{');
        let line_no_brace = line.strip_suffix('{').map(str::trim_end).unwrap_or(line);

        if let Some((kw, args_str)) = split_call(line_no_brace) {
            // Boundary opener: create a boundary node, link it to its parent (the
            // current top of the stack), and push it so inner elements/boundaries
            // attach to it.
            if let Some(kind) = boundary_keyword(kw) {
                let args = split_args(args_str);
                if let Some(b) = build_boundary(kind, boundary_stack.last().copied(), &args) {
                    let idx = diag.boundaries.len();
                    diag.boundaries.push(b);
                    if opens_block {
                        boundary_stack.push(idx);
                    }
                }
                continue;
            }

            if let Some((kind, external)) = element_keyword(kw) {
                let args = split_args(args_str);
                if let Some(elem) = build_element(kind, external, &args) {
                    if seen_ids.insert(elem.id.clone(), ()).is_none() {
                        // Register membership in the enclosing boundary, if any.
                        if let Some(&bi) = boundary_stack.last() {
                            diag.boundaries[bi].member_elems.push(elem.id.clone());
                        }
                        diag.elements.push(elem);
                    }
                }
                continue;
            }

            if let Some(bidir) = relationship_keyword(kw) {
                let args = split_args(args_str);
                if let Some(rel) = build_relationship(&args) {
                    if bidir {
                        diag.relationships.push(Relationship {
                            from: rel.to.clone(),
                            to: rel.from.clone(),
                            label: rel.label.clone(),
                            tech: rel.tech.clone(),
                        });
                    }
                    diag.relationships.push(rel);
                }
                continue;
            }

            // Unknown call (UpdateElementStyle / UpdateRelStyle /
            // UpdateLayoutConfig / sprites / etc.): ignore.
            continue;
        }

        // Anything else (directives, stray tokens): ignore.
    }

    if pending_header {
        return Err("empty input / no C4 diagram header".to_string());
    }
    Ok(diag)
}

/// Map a boundary keyword to its [`BoundaryKind`], if it is one.
fn boundary_keyword(kw: &str) -> Option<BoundaryKind> {
    Some(match kw {
        "System_Boundary" => BoundaryKind::System,
        "Enterprise_Boundary" => BoundaryKind::Enterprise,
        "Container_Boundary" => BoundaryKind::Container,
        "Boundary" => BoundaryKind::Generic,
        _ => return None,
    })
}

/// Build a [`Boundary`] from a keyword's kind, its parent (the enclosing open
/// boundary) and its split args: `id, "label"`. `None` if there is no id.
fn build_boundary(kind: BoundaryKind, parent: Option<usize>, args: &[String]) -> Option<Boundary> {
    let id = args.first().map(|s| s.trim().to_string())?;
    if id.is_empty() {
        return None;
    }
    let label = args.get(1).cloned().unwrap_or_default();
    let label = if label.is_empty() { id.clone() } else { label };
    Some(Boundary {
        id,
        label,
        kind,
        parent,
        member_elems: Vec::new(),
    })
}

/// Build an [`Element`] from a keyword's kind and its split args. The first arg
/// is the id; remaining args are label, (tech,) description. `None` if there is
/// no id.
fn build_element(kind: ElemKind, external: bool, args: &[String]) -> Option<Element> {
    let id = args.first().map(|s| s.trim().to_string())?;
    if id.is_empty() {
        return None;
    }
    let label = args.get(1).cloned().unwrap_or_default();
    let (tech, descr) = if keyword_has_tech(kind) {
        let tech = args.get(2).cloned().unwrap_or_default();
        let descr = args.get(3).cloned().unwrap_or_default();
        (tech, descr)
    } else {
        let descr = args.get(2).cloned().unwrap_or_default();
        (String::new(), descr)
    };
    // A missing label falls back to the id.
    let label = if label.is_empty() { id.clone() } else { label };
    Some(Element {
        id,
        label,
        tech,
        descr: wrap(&descr, WRAP_CHARS),
        kind,
        external,
    })
}

/// Build a [`Relationship`] from split args: `from, to, label, tech?`. `None` if
/// from/to are missing.
fn build_relationship(args: &[String]) -> Option<Relationship> {
    let from = args.first().map(|s| s.trim().to_string())?;
    let to = args.get(1).map(|s| s.trim().to_string())?;
    if from.is_empty() || to.is_empty() {
        return None;
    }
    let label = args.get(2).cloned().unwrap_or_default();
    let tech = args.get(3).cloned().unwrap_or_default();
    Some(Relationship {
        from,
        to,
        label,
        tech,
    })
}

/// Boundary rectangle stroke (dashed, dark grey) and a faint themed fill.
const BOUNDARY_STROKE: [u8; 4] = [68, 68, 68, 255];
/// Boundary label / type text color.
const BOUNDARY_TEXT: [u8; 4] = [68, 68, 68, 255];

/// Vertical padding inside a box for the head/circle of a person, px.
const PERSON_HEAD_R: f32 = 9.0;
/// Per text line height factor (× font size).
const LINE_H_EM: f32 = 1.3;

/// External elements get a greyer fill to distinguish them.
const EXTERNAL_FILL: [u8; 4] = [153, 153, 153, 255];
const EXTERNAL_STROKE: [u8; 4] = [102, 102, 102, 255];
/// Person elements get a distinct (blue-ish) fill.
const PERSON_FILL: [u8; 4] = [8, 67, 124, 255];
const PERSON_STROKE: [u8; 4] = [7, 51, 99, 255];
/// Person/external text is light so it reads on the darker fill.
const LIGHT_TEXT: [u8; 4] = [255, 255, 255, 255];

/// The fill / stroke / text colors for an element box.
fn element_colors(elem: &Element, opts: &MermaidOptions) -> ([u8; 4], [u8; 4], [u8; 4]) {
    if elem.external {
        (EXTERNAL_FILL, EXTERNAL_STROKE, LIGHT_TEXT)
    } else if elem.kind == ElemKind::Person {
        (PERSON_FILL, PERSON_STROKE, LIGHT_TEXT)
    } else {
        (opts.node_fill, opts.node_stroke, opts.text_color)
    }
}

/// The stacked text lines of an element box: bold name, `[Type: tech]`, then the
/// wrapped description lines.
fn element_lines(elem: &Element) -> Vec<(String, bool)> {
    let mut lines: Vec<(String, bool)> = Vec::new();
    lines.push((elem.label.clone(), true)); // bold name
    lines.push((elem.type_label(), false));
    for l in elem.descr.split('\n') {
        if !l.is_empty() {
            lines.push((l.to_string(), false));
        }
    }
    lines
}

impl Element {
    fn type_label(&self) -> String {
        self.kind.type_label(self.external, &self.tech)
    }
}

/// Render a mermaid `c4` diagram to SVG.
pub fn render_c4(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diag = parse(src).map_err(MermaidError::Parse)?;
    if diag.elements.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let line_h = fs * LINE_H_EM;

    // id → node index (first-seen order matches dagre node indices).
    let index_of: HashMap<&str, u32> = diag
        .elements
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.as_str(), i as u32))
        .collect();

    // Size each box from its stacked lines + padding. Persons reserve a little
    // extra headroom for the head circle.
    let sizes: Vec<(f32, f32)> = diag
        .elements
        .iter()
        .map(|e| {
            let lines = element_lines(e);
            let mut max_w = 0.0_f32;
            for (txt, _) in &lines {
                let (w, _) = crate::label::measure(txt, fs);
                max_w = max_w.max(w);
            }
            let w = max_w + 2.0 * opts.node_padding_x;
            let mut h = lines.len() as f32 * line_h + 2.0 * opts.node_padding_y;
            if e.kind == ElemKind::Person {
                h += PERSON_HEAD_R * 2.0;
            }
            (w.max(60.0), h.max(40.0))
        })
        .collect();

    // Build the dagre edge list (dropping any relationship whose endpoints are
    // unknown — i.e. reference an id that was never declared as an element).
    // Also build a parallel list of edge-label box sizes so dagre reserves space
    // and positions each relationship label.
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(diag.relationships.len());
    let mut kept: Vec<usize> = Vec::with_capacity(diag.relationships.len());
    let mut label_sizes: Vec<Option<Vec2>> = Vec::with_capacity(diag.relationships.len());
    for (j, r) in diag.relationships.iter().enumerate() {
        if let (Some(&a), Some(&b)) =
            (index_of.get(r.from.as_str()), index_of.get(r.to.as_str()))
        {
            edges.push((a, b));
            kept.push(j);
            let text = relationship_text(r);
            label_sizes.push(if text.is_empty() {
                None
            } else {
                let (w, h) = crate::label::measure(&text, fs);
                Some(Vec2::new(w + 10.0, h + 6.0))
            });
        }
    }

    // Dagre node list = elements (indices `0..n`) then one synthetic container
    // node per boundary (indices `n..n+b`). Container nodes get size (0,0) — the
    // engine computes their bounding rectangle from their members.
    let n = diag.elements.len();
    let b = diag.boundaries.len();
    let mut node_sizes: Vec<Vec2> = sizes.iter().map(|&(w, h)| Vec2::new(w, h)).collect();
    node_sizes.resize(n + b, Vec2::ZERO);

    // `node_parents[i]` = the dagre index of the boundary container holding node
    // `i`, or `None` for a top-level node. Built only when there are boundaries
    // (so the no-boundary path passes `None` and is byte-for-byte unchanged).
    let node_parents: Option<Vec<Option<usize>>> = if b == 0 {
        None
    } else {
        let mut parents: Vec<Option<usize>> = vec![None; n + b];
        for (j, bd) in diag.boundaries.iter().enumerate() {
            // Each member element → this boundary's container index.
            for id in &bd.member_elems {
                if let Some(&fi) = index_of.get(id.as_str()) {
                    parents[fi as usize] = Some(n + j);
                }
            }
            // Nested boundary → its parent boundary's container index.
            if let Some(p) = bd.parent {
                parents[n + j] = Some(n + p);
            }
        }
        Some(parents)
    };

    let engine = LayeredEngine {
        rankdir: RankDir::Tb,
        ranksep: opts.rank_sep,
        nodesep: opts.node_sep,
        edgesep: 20.0,
        default_node_size: Vec2::new(120.0, 60.0),
    };
    let out = engine.layout(&GraphInput {
        node_count: n + b,
        edges: &edges,
        node_sizes: Some(&node_sizes),
        edge_label_sizes: Some(&label_sizes),
        node_parents: node_parents.as_deref(),
        directed: true,
    });

    let base_width = (out.size.x.ceil() + 1.0).max(1.0);
    let base_height = (out.size.y.ceil() + 1.0).max(1.0);

    // Boundary labels are drawn ABOVE each box's top edge (using the gap that
    // already exists above nested clusters), so children never paint over them.
    // Reserve a top margin for the outermost boundary, and widen the canvas for
    // any label that extends past the right edge.
    let label_block_h = fs * LINE_H_EM + fs + 4.0;
    let mut top_pad = 0.0f32;
    let mut right_pad = 0.0f32;
    for j in 0..diag.boundaries.len() {
        let k = n + j;
        match (out.positions.get(k), out.node_sizes.get(k)) {
            (Some(c), Some(s)) if s.x > 0.0 && s.y > 0.0 => {
                let left = c.x - s.x / 2.0;
                let top = c.y - s.y / 2.0;
                top_pad = top_pad.max(label_block_h - top);
                let lw = boundary_label_width(&diag.boundaries[j], fs);
                right_pad = right_pad.max(left + lw - base_width);
            }
            _ => {}
        }
    }
    let top_pad = top_pad.max(0.0).ceil();
    let right_pad = right_pad.max(0.0).ceil();
    let width = base_width + right_pad;
    let height = base_height + top_pad;

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\">"
    );
    // Shift all diagram content down by `top_pad` so above-box boundary labels
    // (and anything at y≈0) stay on-canvas.
    let _ = write!(svg, "<g transform=\"translate(0,{top_pad:.2})\">");

    // Boundary rectangles first — behind relationships and element boxes. Read
    // back each boundary's rect (center from `out.positions`, size from
    // `out.node_sizes`) and draw outermost-first (by nesting depth) so a nested
    // boundary's rect paints over its enclosing parent.
    let mut bidx: Vec<usize> = (0..diag.boundaries.len()).collect();
    bidx.sort_by_key(|&j| boundary_depth(&diag.boundaries, j));
    for &j in &bidx {
        let k = n + j;
        let center = match out.positions.get(k).copied() {
            Some(c) => c,
            None => continue,
        };
        let size = out.node_sizes.get(k).copied().unwrap_or(Vec2::ZERO);
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        emit_boundary(&mut svg, &diag.boundaries[j], center, size, fs, opts);
    }

    // Group relationships by unordered node pair so parallel / bidirectional
    // relationships spread their labels apart.
    let mut group_total: HashMap<(String, String), usize> = HashMap::new();
    for &orig in &kept {
        let r = &diag.relationships[orig];
        *group_total.entry(pair_key(&r.from, &r.to)).or_insert(0) += 1;
    }
    let mut group_seen: HashMap<(String, String), usize> = HashMap::new();

    // Relationship polylines first, then element boxes on top.
    for (dagre_idx, &orig) in kept.iter().enumerate() {
        let r = &diag.relationships[orig];
        let pts: Vec<(f32, f32)> = out
            .edge_routes
            .get(dagre_idx)
            .map(|route| route.iter().map(|p| (p.x, p.y)).collect())
            .unwrap_or_default();
        let key = pair_key(&r.from, &r.to);
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

    for (i, e) in diag.elements.iter().enumerate() {
        let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
        let (w, h) = sizes[i];
        emit_element(&mut svg, e, pos.x, pos.y, w, h, line_h, opts);
    }

    svg.push_str("</g></svg>");

    Ok(MermaidRender {
        svg,
        width_px: width,
        height_px: height,
    })
}

/// An order-independent key for a node pair, used to group parallel /
/// bidirectional relationships so their labels spread apart.
fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// One relationship: a polyline, an open arrowhead at the `to` end, and the
/// label (plus tech in parens) placed off the route on a light background.
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
    // Smooth curve through the route points; the open arrowhead is drawn
    // separately from the original points so it still lands on the border.
    let d = crate::svgutil::smooth_path_d(points);
    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\"{so}/>",
        d.trim_end(),
        stroke = rgb(opts.edge_stroke),
        so = opacity_attr("stroke-opacity", opts.edge_stroke),
    );

    // Open arrowhead at the destination end (last point).
    let n = points.len();
    if let Some(dir) = unit(points[n - 2], points[n - 1]) {
        draw_open_arrow(svg, points[n - 1], dir, opts);
    }

    // Label (+ tech in parens), de-collided via `edge_label_anchor`.
    let label = relationship_text(rel);
    if !label.is_empty() {
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
            // Relationship label is a single centered string: route it through
            // the rich-label renderer for markdown/math support.
            crate::label::emit(
                svg,
                &label,
                cx,
                cy,
                crate::label::Anchor::Middle,
                opts.font_size_px,
                opts.text_color,
                &opts.font_family,
            );
        }
    }
}

/// The drawn text for a relationship: the label with the tech appended in
/// parentheses (if present).
fn relationship_text(rel: &Relationship) -> String {
    match (rel.label.is_empty(), rel.tech.is_empty()) {
        (true, true) => String::new(),
        (false, true) => rel.label.clone(),
        (true, false) => format!("[{}]", rel.tech),
        (false, false) => format!("{} [{}]", rel.label, rel.tech),
    }
}

/// Draw an **open** (two-stroke V) arrowhead at `tip`, pointing along `dir`.
fn draw_open_arrow(svg: &mut String, tip: (f32, f32), dir: (f32, f32), opts: &MermaidOptions) {
    let stroke = rgb(opts.edge_stroke);
    let so = opacity_attr("stroke-opacity", opts.edge_stroke);
    let len = 10.0_f32;
    let half = 4.0_f32;
    let back = (tip.0 - dir.0 * len, tip.1 - dir.1 * len);
    let perp = (-dir.1, dir.0);
    let a = (back.0 + perp.0 * half, back.1 + perp.1 * half);
    let b = (back.0 - perp.0 * half, back.1 - perp.1 * half);
    let _ = write!(
        svg,
        "<path d=\"M{ax:.2},{ay:.2} L{tx:.2},{ty:.2} L{bx:.2},{by:.2}\" fill=\"none\" \
         stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
        ax = a.0,
        ay = a.1,
        tx = tip.0,
        ty = tip.1,
        bx = b.0,
        by = b.1,
    );
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

/// The nesting depth of boundary `j` (0 = top-level), following `parent` links.
/// Used to order drawing outermost-first.
fn boundary_depth(boundaries: &[Boundary], j: usize) -> usize {
    let mut depth = 0;
    let mut cur = boundaries[j].parent;
    while let Some(p) = cur {
        depth += 1;
        cur = boundaries[p].parent;
    }
    depth
}

/// One boundary: a dashed rounded rectangle with a faint themed fill, plus the
/// `«Type»` line and the boundary name stacked at the top-left.
fn emit_boundary(
    svg: &mut String,
    boundary: &Boundary,
    center: Vec2,
    size: Vec2,
    fs: f32,
    opts: &MermaidOptions,
) {
    let x = center.x - size.x / 2.0;
    let y = center.y - size.y / 2.0;

    // Dashed rounded rectangle, faint themed fill (background nudged so it reads
    // as a subtle tint without obscuring members).
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" rx=\"2.5\" ry=\"2.5\" \
         fill=\"{fill}\" fill-opacity=\"0.06\" stroke=\"{stroke}\"{so} stroke-width=\"1\" \
         stroke-dasharray=\"7,7\"/>",
        w = size.x,
        h = size.y,
        fill = rgb(opts.node_fill),
        stroke = rgb(BOUNDARY_STROKE),
        so = opacity_attr("stroke-opacity", BOUNDARY_STROKE),
    );

    // `«Type»` then the bold name, stacked just ABOVE the top edge (left-anchored)
    // so member boxes — which sit at the very top of the cluster — never cover
    // them. The caller reserves the headroom (top margin / sibling gap).
    let tx = x + 2.0;
    let name_y = y - fs * 0.5 - 2.0;
    let type_y = name_y - fs * LINE_H_EM;
    emit_boundary_text(svg, boundary.kind.type_label(), tx, type_y, fs, opts, None);
    emit_boundary_text(svg, &boundary.label, tx, name_y, fs, opts, Some("bold"));
}

/// Width needed for a boundary's two-line label (`«Type»` / bold name).
fn boundary_label_width(boundary: &Boundary, fs: f32) -> f32 {
    let type_w = crate::label::measure(boundary.kind.type_label(), fs).0;
    // Name is bold (renders a touch wider than the measured regular weight).
    let name_w = crate::label::measure(&boundary.label, fs).0 * 1.08;
    type_w.max(name_w) + 4.0
}

/// A left-anchored single-line `<text>` for boundary labels.
fn emit_boundary_text(
    svg: &mut String,
    label: &str,
    x: f32,
    y: f32,
    fs: f32,
    opts: &MermaidOptions,
    font_weight: Option<&str>,
) {
    if label.is_empty() {
        return;
    }
    let weight = font_weight
        .map(|w| format!(" font-weight=\"{w}\""))
        .unwrap_or_default();
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\"{weight} fill=\"{fill}\"{fo}>{txt}</text>",
        family = escape(&opts.font_family),
        fill = rgb(BOUNDARY_TEXT),
        fo = opacity_attr("fill-opacity", BOUNDARY_TEXT),
        txt = escape(label),
    );
}

/// One element box: optional person head circle, then the stacked centered
/// lines (bold name, `[Type: tech]`, wrapped description).
#[allow(clippy::too_many_arguments)]
fn emit_element(
    svg: &mut String,
    elem: &Element,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    line_h: f32,
    opts: &MermaidOptions,
) {
    let x = cx - w / 2.0;
    let y = cy - h / 2.0;
    let (fill, stroke, text_color) = element_colors(elem, opts);

    // Outer box.
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" rx=\"3\" ry=\"3\" \
         fill=\"{f}\"{fo} stroke=\"{s}\"{so} stroke-width=\"1.5\"/>",
        f = rgb(fill),
        fo = opacity_attr("fill-opacity", fill),
        s = rgb(stroke),
        so = opacity_attr("stroke-opacity", stroke),
    );

    // The stacked text lines, vertically centered within the (head-adjusted)
    // text area.
    let lines = element_lines(elem);
    let head_offset = if elem.kind == ElemKind::Person {
        // Little "head" circle on top to suggest a person.
        let head_cx = cx;
        let head_cy = y + PERSON_HEAD_R + 2.0;
        let _ = write!(
            svg,
            "<circle cx=\"{head_cx:.2}\" cy=\"{head_cy:.2}\" r=\"{r:.2}\" \
             fill=\"{f}\"{fo} stroke=\"{s}\"{so} stroke-width=\"1.5\"/>",
            r = PERSON_HEAD_R,
            f = rgb(fill),
            fo = opacity_attr("fill-opacity", fill),
            s = rgb(stroke),
            so = opacity_attr("stroke-opacity", stroke),
        );
        PERSON_HEAD_R * 2.0
    } else {
        0.0
    };

    let text_area_top = y + opts.node_padding_y + head_offset;
    let text_area_h = h - 2.0 * opts.node_padding_y - head_offset;
    let block_h = lines.len() as f32 * line_h;
    let mut ty = text_area_top + (text_area_h - block_h) / 2.0 + line_h * 0.5;
    for (txt, bold) in &lines {
        // The bold name line is a single centered string: route it through the
        // rich-label renderer so it supports markdown/math. Plain names keep the
        // bold `<text>` (label::emit has no default-bold, so it would otherwise
        // drop the weight); rich names render their own emphasis/math.
        if *bold && has_rich_markup(txt) {
            crate::label::emit(
                svg,
                txt,
                cx,
                ty,
                crate::label::Anchor::Middle,
                opts.font_size_px,
                text_color,
                &opts.font_family,
            );
        } else {
            let weight = if *bold { Some("bold") } else { None };
            emit_text(svg, txt, cx, ty, opts, text_color, None, weight);
        }
        ty += line_h;
    }
}

/// Cheap check for any rich-label marker (markdown emphasis, inline math, or an
/// explicit `<br>`); mirrors `crate::label`'s richness test so plain labels stay
/// on the byte-identical `<text>` path.
fn has_rich_markup(s: &str) -> bool {
    s.contains('*') || s.contains('_') || s.contains('$') || s.contains("<br")
}

/// A centered single-line `<text>` with optional `font-style` / `font-weight`.
#[allow(clippy::too_many_arguments)]
fn emit_text(
    svg: &mut String,
    label: &str,
    cx: f32,
    cy: f32,
    opts: &MermaidOptions,
    color: [u8; 4],
    font_style: Option<&str>,
    font_weight: Option<&str>,
) {
    if label.is_empty() {
        return;
    }
    let style = font_style
        .map(|s| format!(" font-style=\"{s}\""))
        .unwrap_or_default();
    let weight = font_weight
        .map(|w| format!(" font-weight=\"{w}\""))
        .unwrap_or_default();
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\"{style}{weight} fill=\"{fill}\"{fo}>{txt}</text>",
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
    fn parse_person_system_rel() {
        let src = "C4Context\n\
            Person(u, \"User\", \"a user\")\n\
            System(s, \"Sys\", \"the system\")\n\
            Rel(u, s, \"uses\")";
        let d = parse(src).unwrap();
        assert_eq!(d.elements.len(), 2);

        let u = &d.elements[0];
        assert_eq!(u.id, "u");
        assert_eq!(u.label, "User");
        assert_eq!(u.kind, ElemKind::Person);
        assert!(!u.external);
        assert_eq!(u.descr, "a user");

        let s = &d.elements[1];
        assert_eq!(s.id, "s");
        assert_eq!(s.label, "Sys");
        assert_eq!(s.kind, ElemKind::System);
        assert_eq!(s.descr, "the system");

        assert_eq!(d.relationships.len(), 1);
        assert_eq!(d.relationships[0].from, "u");
        assert_eq!(d.relationships[0].to, "s");
        assert_eq!(d.relationships[0].label, "uses");
    }

    #[test]
    fn parse_external_variants() {
        let src = "C4Context\n\
            Person_Ext(pe, \"Ext Person\")\n\
            System_Ext(se, \"Ext Sys\")";
        let d = parse(src).unwrap();
        assert_eq!(d.elements.len(), 2);
        assert!(d.elements[0].external && d.elements[0].kind == ElemKind::Person);
        assert!(d.elements[1].external && d.elements[1].kind == ElemKind::System);
    }

    #[test]
    fn parse_container_component_with_tech() {
        let src = "C4Container\n\
            Container(c, \"Web App\", \"Rust\", \"serves pages\")\n\
            Component(cmp, \"Handler\", \"axum\", \"routes\")\n\
            ContainerDb(db, \"DB\", \"Postgres\", \"stores data\")";
        let d = parse(src).unwrap();
        assert_eq!(d.elements.len(), 3);

        let c = &d.elements[0];
        assert_eq!(c.kind, ElemKind::Container);
        assert_eq!(c.tech, "Rust");
        assert_eq!(c.descr, "serves pages");

        let cmp = &d.elements[1];
        assert_eq!(cmp.kind, ElemKind::Component);
        assert_eq!(cmp.tech, "axum");

        let db = &d.elements[2];
        assert_eq!(db.kind, ElemKind::Container);
        assert_eq!(db.tech, "Postgres");
    }

    #[test]
    fn quoted_args_with_commas_are_safe() {
        let src = "C4Context\n\
            System(s, \"A, B and C\", \"desc, with comma\")";
        let d = parse(src).unwrap();
        assert_eq!(d.elements.len(), 1);
        assert_eq!(d.elements[0].label, "A, B and C");
        assert_eq!(d.elements[0].descr, "desc, with comma");
    }

    #[test]
    fn rel_with_tech() {
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            Rel(a, b, \"calls\", \"HTTPS\")";
        let d = parse(src).unwrap();
        assert_eq!(d.relationships.len(), 1);
        assert_eq!(d.relationships[0].label, "calls");
        assert_eq!(d.relationships[0].tech, "HTTPS");
    }

    #[test]
    fn directional_rel_variants() {
        for kw in ["Rel_U", "Rel_D", "Rel_L", "Rel_R", "Rel_Up", "Rel_Down"] {
            let src = format!(
                "C4Context\nSystem(a,\"A\")\nSystem(b,\"B\")\n{kw}(a, b, \"x\")"
            );
            let d = parse(&src).unwrap();
            assert_eq!(d.relationships.len(), 1, "kw {kw}");
            assert_eq!(d.relationships[0].from, "a");
            assert_eq!(d.relationships[0].to, "b");
        }
    }

    #[test]
    fn birel_adds_reverse_edge() {
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            BiRel(a, b, \"talks\")";
        let d = parse(src).unwrap();
        assert_eq!(d.relationships.len(), 2);
        // One a→b and one b→a, both labeled "talks".
        let mut pairs: Vec<(String, String)> = d
            .relationships
            .iter()
            .map(|r| (r.from.clone(), r.to.clone()))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                ("a".to_string(), "b".to_string()),
                ("b".to_string(), "a".to_string())
            ]
        );
        assert!(d.relationships.iter().all(|r| r.label == "talks"));
    }

    #[test]
    fn boundary_inner_elements_parsed_and_grouped() {
        let src = "C4Context\n\
            System_Boundary(b1, \"Boundary\") {\n\
            Person(u, \"User\")\n\
            System(s, \"Sys\")\n\
            }\n\
            Rel(u, s, \"uses\")";
        let d = parse(src).unwrap();
        // Inner elements present and still parsed.
        assert_eq!(d.elements.len(), 2);
        assert_eq!(d.elements[0].id, "u");
        assert_eq!(d.elements[1].id, "s");
        assert_eq!(d.relationships.len(), 1);
        // The boundary is captured with its two members.
        assert_eq!(d.boundaries.len(), 1);
        let b = &d.boundaries[0];
        assert_eq!(b.id, "b1");
        assert_eq!(b.label, "Boundary");
        assert_eq!(b.kind, BoundaryKind::System);
        assert_eq!(b.parent, None);
        assert_eq!(b.member_elems, vec!["u".to_string(), "s".to_string()]);
    }

    #[test]
    fn boundary_members_from_spec_example() {
        let src = "C4Container\n\
            System_Boundary(b1, \"My System\") {\n\
            Container(c1,\"Web\",\"\",\"\")\n\
            Container(c2,\"DB\",\"\",\"\")\n\
            }";
        let d = parse(src).unwrap();
        assert_eq!(d.boundaries.len(), 1);
        assert_eq!(d.boundaries[0].id, "b1");
        assert_eq!(d.boundaries[0].label, "My System");
        assert_eq!(
            d.boundaries[0].member_elems,
            vec!["c1".to_string(), "c2".to_string()]
        );
        assert_eq!(d.elements.len(), 2);
    }

    #[test]
    fn nested_boundaries_parsed() {
        let src = "C4Container\n\
            Enterprise_Boundary(e1, \"Ent\") {\n\
            System_Boundary(s1, \"Sys\") {\n\
            Container(c1, \"Web\")\n\
            }\n\
            Container(c2, \"Edge\")\n\
            }";
        let d = parse(src).unwrap();
        assert_eq!(d.boundaries.len(), 2);
        let e = &d.boundaries[0];
        let s = &d.boundaries[1];
        assert_eq!(e.kind, BoundaryKind::Enterprise);
        assert_eq!(e.parent, None);
        assert_eq!(e.member_elems, vec!["c2".to_string()]);
        assert_eq!(s.kind, BoundaryKind::System);
        assert_eq!(s.parent, Some(0));
        assert_eq!(s.member_elems, vec!["c1".to_string()]);
    }

    #[test]
    fn boundary_rect_encloses_member_centers() {
        let src = "C4Container\n\
            System_Boundary(b1, \"My System\") {\n\
            Container(c1, \"Web\")\n\
            Container(c2, \"DB\")\n\
            }\n\
            Container(c3, \"Outside\")\n\
            Rel(c1, c2, \"x\")";
        let r = render_c4(src, &opts()).unwrap();
        // A dashed boundary rect must be present.
        assert!(
            r.svg.contains("stroke-dasharray=\"7,7\""),
            "dashed boundary rect missing: {}",
            r.svg
        );
        // And the boundary label + type.
        assert!(r.svg.contains(">My System<"));
        assert!(r.svg.contains("«System»"));
    }

    #[test]
    fn nested_boundary_render_has_two_dashed_rects() {
        let src = "C4Container\n\
            Enterprise_Boundary(e1, \"Ent\") {\n\
            System_Boundary(s1, \"Sys\") {\n\
            Container(c1, \"Web\")\n\
            }\n\
            }";
        let r = render_c4(src, &opts()).unwrap();
        assert_eq!(
            r.svg.matches("stroke-dasharray=\"7,7\"").count(),
            2,
            "expected two dashed boundary rects: {}",
            r.svg
        );
        assert!(r.svg.contains("«Enterprise»"));
        assert!(r.svg.contains("«System»"));
    }

    #[test]
    fn boundary_drawn_before_element_boxes() {
        let src = "C4Container\n\
            System_Boundary(b1, \"My System\") {\n\
            Container(c1, \"Web\")\n\
            }";
        let r = render_c4(src, &opts()).unwrap();
        let dash = r.svg.find("stroke-dasharray=\"7,7\"").unwrap();
        // The element box is a rounded rect with rx="3"; the boundary uses
        // rx="2.5". The dashed boundary rect must precede the first element box.
        let elem_box = r.svg.find("rx=\"3\" ry=\"3\"").unwrap();
        assert!(dash < elem_box, "boundary not drawn before element box");
    }

    #[test]
    fn no_boundary_diagram_unchanged() {
        // A diagram with no boundaries must contain no dashed boundary rects and
        // render identically through the no-boundary path.
        let src = "C4Context\n\
            Person(u, \"User\")\n\
            System(s, \"Sys\")\n\
            Rel(u, s, \"uses\")";
        let r = render_c4(src, &opts()).unwrap();
        let d = parse(src).unwrap();
        assert!(d.boundaries.is_empty());
        assert!(!r.svg.contains("stroke-dasharray"));
    }

    #[test]
    fn all_c4_headers_accepted() {
        for h in [
            "C4Context",
            "C4Container",
            "C4Component",
            "C4Dynamic",
            "C4Deployment",
        ] {
            let src = format!("{h}\nSystem(s, \"S\")");
            let d = parse(&src).unwrap();
            assert_eq!(d.elements.len(), 1, "header {h}");
        }
    }

    #[test]
    fn bad_header_errors() {
        assert!(parse("graph TD\nA --> B").is_err());
    }

    #[test]
    fn no_header_errors() {
        assert!(parse("\n\n").is_err());
    }

    #[test]
    fn label_falls_back_to_id() {
        let src = "C4Context\nSystem(s)";
        let d = parse(src).unwrap();
        assert_eq!(d.elements[0].label, "s");
    }

    #[test]
    fn type_label_formatting() {
        assert_eq!(ElemKind::Person.type_label(false, ""), "[Person]");
        assert_eq!(
            ElemKind::Person.type_label(true, ""),
            "[External Person]"
        );
        assert_eq!(
            ElemKind::Container.type_label(false, "Rust"),
            "[Container: Rust]"
        );
        assert_eq!(ElemKind::System.type_label(false, ""), "[Software System]");
        assert_eq!(
            ElemKind::System.type_label(true, ""),
            "[External System]"
        );
    }

    #[test]
    fn render_wellformed_svg() {
        let src = "C4Context\n\
            Person(u, \"User\", \"a user\")\n\
            System(s, \"Sys\", \"the system\")\n\
            Rel(u, s, \"uses\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.svg.contains("xmlns="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_box_per_element_with_label_and_type() {
        let src = "C4Container\n\
            Person(u, \"User\")\n\
            Container(c, \"App\", \"Rust\", \"the app\")";
        let r = render_c4(src, &opts()).unwrap();
        // Two boxes.
        assert!(r.svg.matches("<rect").count() >= 2);
        // Labels + type lines present.
        assert!(r.svg.contains(">User<"));
        assert!(r.svg.contains(">App<"));
        assert!(r.svg.contains("[Person]"));
        assert!(r.svg.contains("[Container: Rust]"));
    }

    #[test]
    fn render_relationship_polyline_arrow_and_label() {
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            Rel(a, b, \"calls\")";
        let r = render_c4(src, &opts()).unwrap();
        // One edge polyline (fill="none" path) + one arrowhead path.
        assert_eq!(r.svg.matches("<path d=").count(), 2);
        assert!(r.svg.contains(">calls<"));
    }

    #[test]
    fn render_rel_label_includes_tech() {
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            Rel(a, b, \"calls\", \"HTTPS\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains("calls [HTTPS]"));
    }

    #[test]
    fn person_vs_external_distinct() {
        let src = "C4Context\n\
            Person(p, \"P\")\n\
            System_Ext(e, \"E\")\n\
            System(s, \"S\")";
        let r = render_c4(src, &opts()).unwrap();
        // Person uses the person fill, external uses the grey fill, normal
        // system uses the default node fill — all three distinct.
        let person = rgb(PERSON_FILL);
        let external = rgb(EXTERNAL_FILL);
        let normal = rgb(opts().node_fill);
        assert!(r.svg.contains(&format!("fill=\"{person}\"")));
        assert!(r.svg.contains(&format!("fill=\"{external}\"")));
        assert!(r.svg.contains(&format!("fill=\"{normal}\"")));
        // Person has a head circle.
        assert!(r.svg.contains("<circle"));
    }

    #[test]
    fn xml_escapes_text() {
        let src = "C4Context\n\
            System(s, \"A & B < C\", \"x > y\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains("A &amp; B &lt; C"));
        assert!(r.svg.contains("x &gt; y"));
        assert!(!r.svg.contains("A & B"));
    }

    #[test]
    fn element_name_renders_inline_math() {
        // An element name containing `$…$` renders the embedded math group
        // rather than a plain bold `<text>` with the raw dollars.
        let src = "C4Context\nSystem(s, \"Energy $x^2$\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains("<g transform"), "expected math group: {}", r.svg);
        assert!(r.svg.contains("<path"), "expected math path: {}", r.svg);
    }

    #[test]
    fn relationship_label_renders_bold_markdown() {
        // A `**bold**` relationship label renders a bold run, not literal `**`.
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            Rel(a, b, \"**calls**\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains("font-weight=\"bold\""), "expected bold run: {}", r.svg);
        assert!(!r.svg.contains("**calls**"), "raw markdown leaked: {}", r.svg);
    }

    #[test]
    fn plain_element_name_keeps_bold_text() {
        // A plain (non-rich) name must still render as a bold <text> — unchanged.
        let src = "C4Context\nSystem(s, \"Plain\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains(">Plain<"));
        assert!(r.svg.contains("font-weight=\"bold\""), "plain name lost bold: {}", r.svg);
    }

    #[test]
    fn empty_diagram_errors() {
        assert_eq!(
            render_c4("C4Context\n", &opts()),
            Err(MermaidError::Empty)
        );
    }

    #[test]
    fn deterministic() {
        let src = "C4Context\n\
            Person(u, \"User\", \"a user\")\n\
            System(s, \"Sys\")\n\
            Rel(u, s, \"uses\")\n\
            Rel(s, u, \"replies\")";
        let x = render_c4(src, &opts()).unwrap();
        let y = render_c4(src, &opts()).unwrap();
        assert_eq!(x, y);
    }

    #[test]
    fn bidirectional_labels_separated() {
        // Two relationships between the same pair → distinct label anchors.
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            Rel(a, b, \"up\")\n\
            Rel(b, a, \"down\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains(">up<"));
        assert!(r.svg.contains(">down<"));
        // The two label backgrounds must sit at distinct anchors. Dagre now
        // reserves the labels in the rank gap and orders them apart (here in x),
        // so check separation in either axis rather than y specifically.
        let pts = label_rect_centers(&r.svg);
        assert_eq!(pts.len(), 2, "two label backgrounds");
        let (dx, dy) = ((pts[0].0 - pts[1].0).abs(), (pts[0].1 - pts[1].1).abs());
        assert!(dx > 1.0 || dy > 1.0, "labels separated: {pts:?}");
    }

    /// Parse the `(x+w/2, y+h/2)` centers of each white label-background rect.
    fn label_rect_centers(svg: &str) -> Vec<(f32, f32)> {
        svg.match_indices("fill=\"rgb(255,255,255)\"")
            .filter_map(|(i, _)| {
                let rect_start = svg[..i].rfind("<rect")?;
                let seg = &svg[rect_start..i];
                let attr = |name: &str| -> Option<f32> {
                    let s = seg.find(name)? + name.len();
                    let rest = &seg[s..];
                    let end = rest.find('"')?;
                    rest[..end].parse::<f32>().ok()
                };
                Some((
                    attr(" x=\"")? + attr(" width=\"")? / 2.0,
                    attr(" y=\"")? + attr(" height=\"")? / 2.0,
                ))
            })
            .collect()
    }
}

