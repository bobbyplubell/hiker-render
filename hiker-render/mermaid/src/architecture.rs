//! `architecture` diagram (`architecture-beta`).
//!
//! Self-contained: parse → grid self-layout → draw one SVG document.
//!
//! ## Confirmed header
//!
//! `architecture-beta` (also the bare `architecture` alias accepted by the
//! dispatcher). The first non-blank, non-`%%`-comment line must be the header.
//!
//! ## Supported subset (from the Langium grammar)
//!
//! * **group** — `group <id>(<icon>)?[<Title>]? (in <parentGroup>)?`. Drawn as a
//!   labeled rounded boundary rect behind its members (faint themed fill).
//!   Nested groups (`in`) are parsed; the parent's box is grown to enclose the
//!   child's members.
//! * **service** — `service <id>(<icon>)?[<Title>]? (in <group>)?` or
//!   `service <id>"<iconText>"[<Title>]? …`. Drawn as a rounded box with its
//!   Title (falling back to the id). The `(icon)` name is rendered as small grey
//!   subtext under the title; real icons are **not** drawn (NOTE).
//! * **junction** — `junction <id> (in <group>)?`. A tiny dot node that edges can
//!   route through.
//! * **edge** — `<idA>{group}?:<side> [<]--[>] <side>:<idB>{group}?`, where each
//!   `<side>` is one of `L|R|T|B`. The optional `<`/`>` mark arrowheads on the
//!   left/right end (directional). The `{group}` modifier is parsed (and tinily
//!   nudges the endpoint toward the group edge) but otherwise treated like a
//!   normal endpoint. The `- title -` labelled-arrow form is parsed and the
//!   title drawn at the edge midpoint.
//! * **align** — `align (row|column) <id> <id> …`. Parsed; members are pinned to
//!   a shared row (same grid y) / column (same grid x) during layout so they
//!   spread instead of colliding.
//!
//! ## Layout (NOT dagre)
//!
//! A simple deterministic grid. Services are placed on an integer cell grid: the
//! first service of each connected component anchors at the origin, then each
//! edge places the neighbour relative to the already-placed endpoint using the
//! requested port sides — `R`/`L` ⇒ horizontal step, `T`/`B` ⇒ vertical step.
//! Collisions are resolved by scanning outward for a free cell. Services with no
//! placement (isolated, no honoured edge) are appended in a row. `align`
//! directives override a member's row/column to a shared value. Cells are then
//! mapped to pixels by uniform column-width / row-height (max box size + sep),
//! and groups get boundary rects sized to enclose their members. Edges route
//! orthogonally between the requested port points (an L-shaped 3-point poly when
//! the sides imply a corner, else a straight line).
//!
//! Simplifications (NOTE): no force relaxation, no real icons, no
//! grid-constraint solver — a readable grouped placement honouring port-side
//! direction is the bar.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

/// A port side on a service box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    L,
    R,
    T,
    B,
}

impl Side {
    fn parse(c: char) -> Option<Side> {
        match c {
            'L' => Some(Side::L),
            'R' => Some(Side::R),
            'T' => Some(Side::T),
            'B' => Some(Side::B),
            _ => None,
        }
    }

    /// Grid step (dx, dy) for a neighbour reached by leaving through this side.
    fn step(self) -> (i32, i32) {
        match self {
            Side::L => (-1, 0),
            Side::R => (1, 0),
            Side::T => (0, -1),
            Side::B => (0, 1),
        }
    }
}

/// A service or junction node.
#[derive(Clone, Debug, PartialEq)]
pub struct Service {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    pub group: Option<String>,
    /// `true` for a `junction` (drawn as a small dot, not a box).
    pub junction: bool,
}

/// A group/boundary box.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    pub parent: Option<String>,
}

/// An edge between two services, with a port side at each end.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub lhs: String,
    pub lhs_side: Side,
    pub lhs_group: bool,
    pub lhs_arrow: bool,
    pub rhs: String,
    pub rhs_side: Side,
    pub rhs_group: bool,
    pub rhs_arrow: bool,
    pub title: String,
}

/// A parsed architecture diagram.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Architecture {
    pub groups: Vec<Group>,
    pub services: Vec<Service>,
    pub edges: Vec<Edge>,
    /// `(is_row, members)` — `align row|column a b c …`.
    pub aligns: Vec<(bool, Vec<String>)>,
}

fn is_header(kw: &str) -> bool {
    kw == "architecture-beta" || kw == "architecture"
}

/// Extract a leading `(...)` icon token from `rest`, returning (icon, remainder).
fn take_icon(rest: &str) -> (Option<String>, &str) {
    let rest = rest.trim_start();
    if let Some(stripped) = rest.strip_prefix('(') {
        if let Some(end) = stripped.find(')') {
            let icon = stripped[..end].trim().to_string();
            return (Some(icon), &stripped[end + 1..]);
        }
    }
    (None, rest)
}

/// Extract a leading `"..."` iconText token, returning (text, remainder).
fn take_quoted(rest: &str) -> (Option<String>, &str) {
    let rest = rest.trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        if let Some(end) = stripped.find('"') {
            return (Some(stripped[..end].to_string()), &stripped[end + 1..]);
        }
    }
    (None, rest)
}

/// Extract a leading `[...]` title token, returning (title, remainder). Strips a
/// surrounding pair of quotes inside the brackets.
fn take_title(rest: &str) -> (Option<String>, &str) {
    let rest = rest.trim_start();
    if let Some(stripped) = rest.strip_prefix('[') {
        if let Some(end) = stripped.find(']') {
            let mut t = stripped[..end].trim();
            if t.len() >= 2
                && ((t.starts_with('"') && t.ends_with('"'))
                    || (t.starts_with('\'') && t.ends_with('\'')))
            {
                t = &t[1..t.len() - 1];
            }
            return (Some(t.to_string()), &stripped[end + 1..]);
        }
    }
    (None, rest)
}

/// Extract a trailing `in <id>` group reference from `rest`.
fn take_in(rest: &str) -> Option<String> {
    let rest = rest.trim();
    let mut it = rest.split_whitespace();
    if it.next() == Some("in") {
        return it.next().map(|s| s.to_string());
    }
    None
}

/// Parse `architecture-beta` source. Returns the diagram or a parse-error string.
pub fn parse(src: &str) -> Result<Architecture, String> {
    let mut lines = src.lines();
    // Header: first non-blank, non-comment line.
    let mut header_ok = false;
    for raw in lines.by_ref() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let kw = line.split_whitespace().next().unwrap_or("");
        if is_header(kw) {
            header_ok = true;
            break;
        }
        return Err(format!("architecture: expected `architecture-beta` header, got {kw:?}"));
    }
    if !header_ok {
        return Err("architecture: missing `architecture-beta` header".to_string());
    }

    let mut diag = Architecture::default();
    for raw in lines {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (kw, rest) = match line.split_once(char::is_whitespace) {
            Some((k, r)) => (k, r.trim()),
            None => (line, ""),
        };
        match kw {
            "title" | "accTitle" | "accDescr" => { /* accessibility: ignored */ }
            "group" => {
                let (id, rest) = split_id(rest);
                if id.is_empty() {
                    return Err(format!("architecture: group missing id: {line:?}"));
                }
                let (icon, rest) = take_icon(rest);
                let (title, rest) = take_title(rest);
                let parent = take_in(rest);
                diag.groups.push(Group {
                    id: id.to_string(),
                    title: title.unwrap_or_else(|| id.to_string()),
                    icon,
                    parent,
                });
            }
            "service" => {
                let (id, rest) = split_id(rest);
                if id.is_empty() {
                    return Err(format!("architecture: service missing id: {line:?}"));
                }
                // icon may be `(name)` or a quoted iconText.
                let (icon, rest) = {
                    let (q, r) = take_quoted(rest);
                    if q.is_some() {
                        (q, r)
                    } else {
                        take_icon(rest)
                    }
                };
                let (title, rest) = take_title(rest);
                let group = take_in(rest);
                diag.services.push(Service {
                    id: id.to_string(),
                    title: title.unwrap_or_else(|| id.to_string()),
                    icon,
                    group,
                    junction: false,
                });
            }
            "junction" => {
                let (id, rest) = split_id(rest);
                if id.is_empty() {
                    return Err(format!("architecture: junction missing id: {line:?}"));
                }
                let group = take_in(rest);
                diag.services.push(Service {
                    id: id.to_string(),
                    title: String::new(),
                    icon: None,
                    group,
                    junction: true,
                });
            }
            "align" => {
                let mut it = rest.split_whitespace();
                let axis = it.next().unwrap_or("");
                let is_row = axis == "row";
                let members: Vec<String> = it.map(|s| s.to_string()).collect();
                if members.len() >= 2 {
                    diag.aligns.push((is_row, members));
                }
            }
            _ => {
                // Anything else must be an edge: <id>{group}?:<S> [<]--[>] <S>:<id>{group}?
                match parse_edge(line) {
                    Some(e) => diag.edges.push(e),
                    None => return Err(format!("architecture: unrecognized statement: {line:?}")),
                }
            }
        }
    }
    Ok(diag)
}

/// Split a leading identifier (`[\w-]+`) off `rest`, returning (id, remainder).
fn split_id(rest: &str) -> (&str, &str) {
    let rest = rest.trim_start();
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    (&rest[..end], &rest[end..])
}

/// Parse one endpoint of an edge: `<id>{group}?:<S>` (left) or `<S>:<id>{group}?`
/// (right). `side_first` selects the right-hand form. Returns (id, side, group?).
fn parse_endpoint(s: &str, side_first: bool) -> Option<(String, Side, bool)> {
    let s = s.trim();
    if side_first {
        // <S>:<id>{group}?
        let (side_c, rest) = s.split_once(':')?;
        let side = Side::parse(side_c.trim().chars().next()?)?;
        let rest = rest.trim();
        let (id, group) = strip_group(rest);
        if id.is_empty() {
            return None;
        }
        Some((id.to_string(), side, group))
    } else {
        // <id>{group}?:<S>
        let (head, side_c) = s.rsplit_once(':')?;
        let side = Side::parse(side_c.trim().chars().next()?)?;
        let (id, group) = strip_group(head.trim());
        if id.is_empty() {
            return None;
        }
        Some((id.to_string(), side, group))
    }
}

/// Strip a trailing/leading `{group}` modifier, returning (id, had_group).
fn strip_group(s: &str) -> (&str, bool) {
    if let Some(rest) = s.strip_suffix("{group}") {
        (rest.trim(), true)
    } else {
        (s, false)
    }
}

/// Parse a full edge line, or `None` if it is not an edge.
fn parse_edge(line: &str) -> Option<Edge> {
    // Find the central connector. The grammar is `LeftPort '--' RightPort` or
    // `LeftPort '-' TITLE '-' RightPort`, with optional `<`/`>` arrow-into marks
    // adjacent to the dashes. We split on the first run of dashes.
    let bytes = line.as_bytes();
    let dash_start = find_dash_run(bytes)?;
    let (mut left, mut right_and_after) = (&line[..dash_start.0], &line[dash_start.1..]);

    // Arrow-into marks: `<` at the end of left, `>` at the start of right.
    let lhs_arrow = left.trim_end().ends_with('<');
    if lhs_arrow {
        let t = left.trim_end();
        left = &t[..t.len() - 1];
    }

    // The connector may carry a `- title -` between two single-dash runs. If the
    // middle isn't a port boundary, treat embedded text as the title.
    let mut title = String::new();
    let rhs_arrow;
    let right;
    {
        let r = right_and_after.trim_start();
        // Possible forms after the first dash run:
        //   ">"? RightPort                       (plain `--`)
        //   TITLE "-" ">"? RightPort             (`- title -`)
        let r2 = r;
        // Detect a `title -` segment: text up to the next `-`.
        if let Some(dash2) = r2.find('-') {
            // Heuristic: if there's another dash and the segment before it is not
            // a valid right endpoint, it's a title.
            let candidate_title = &r2[..dash2];
            if parse_endpoint(strip_into(candidate_title).0, true).is_none()
                && !candidate_title.trim().is_empty()
            {
                title = candidate_title.trim().to_string();
                right_and_after = &r2[dash2 + 1..];
            }
        }
        let r3 = right_and_after.trim_start();
        let (r4, has_arrow) = strip_into(r3);
        rhs_arrow = has_arrow;
        right = r4;
    }

    let (lhs, lhs_side, lhs_group) = parse_endpoint(left, false)?;
    let (rhs, rhs_side, rhs_group) = parse_endpoint(right, true)?;
    Some(Edge {
        lhs,
        lhs_side,
        lhs_group,
        lhs_arrow,
        rhs,
        rhs_side,
        rhs_group,
        rhs_arrow,
        title,
    })
}

/// Strip a leading `>` arrow-into mark, returning (rest, had_arrow).
fn strip_into(s: &str) -> (&str, bool) {
    let t = s.trim_start();
    if let Some(rest) = t.strip_prefix('>') {
        (rest.trim_start(), true)
    } else {
        (t, false)
    }
}

/// Find the first run of `-` (length ≥ 1) that is preceded by a `:port` (so we
/// don't trip on dashes inside ids). Returns (start, end) byte offsets of the run.
fn find_dash_run(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'-' {
            // Require a ':' somewhere before this dash on the line (the left port).
            if bytes[..i].iter().any(|&b| b == b':') {
                let start = i;
                while i < bytes.len() && bytes[i] == b'-' {
                    i += 1;
                }
                return Some((start, i));
            }
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// A placed node with grid cell + pixel rect.
#[derive(Clone, Debug)]
struct Placed {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Assign integer grid cells to services using edge port sides for direction.
fn assign_cells(diag: &Architecture, index: &HashMap<&str, usize>) -> Vec<(i32, i32)> {
    let n = diag.services.len();
    let mut cells: Vec<Option<(i32, i32)>> = vec![None; n];
    let mut occupied: HashMap<(i32, i32), usize> = HashMap::new();

    let place = |cells: &mut Vec<Option<(i32, i32)>>,
                 occupied: &mut HashMap<(i32, i32), usize>,
                 idx: usize,
                 want: (i32, i32)| {
        // Scan outward from `want` for a free cell (deterministic spiral-ish).
        if !occupied.contains_key(&want) {
            cells[idx] = Some(want);
            occupied.insert(want, idx);
            return;
        }
        for r in 1..1000 {
            for d in &[(r, 0), (-r, 0), (0, r), (0, -r), (r, r), (-r, -r), (r, -r), (-r, r)] {
                let c = (want.0 + d.0, want.1 + d.1);
                if !occupied.contains_key(&c) {
                    cells[idx] = Some(c);
                    occupied.insert(c, idx);
                    return;
                }
            }
        }
    };

    // Process edges in order, placing endpoints relative to each other.
    for e in &diag.edges {
        let (Some(&a), Some(&b)) = (index.get(e.lhs.as_str()), index.get(e.rhs.as_str())) else {
            continue;
        };
        match (cells[a], cells[b]) {
            (None, None) => {
                place(&mut cells, &mut occupied, a, (0, 0));
                let base = cells[a].unwrap();
                let step = e.lhs_side.step();
                place(&mut cells, &mut occupied, b, (base.0 + step.0, base.1 + step.1));
            }
            (Some(base), None) => {
                let step = e.lhs_side.step();
                place(&mut cells, &mut occupied, b, (base.0 + step.0, base.1 + step.1));
            }
            (None, Some(base)) => {
                // Place `a` on the opposite side of `b` per rhs side.
                let step = e.rhs_side.step();
                place(&mut cells, &mut occupied, a, (base.0 + step.0, base.1 + step.1));
            }
            (Some(_), Some(_)) => {}
        }
    }

    // Place any still-unplaced services (isolated): append in a row below.
    let max_y = cells.iter().filter_map(|c| c.map(|c| c.1)).max().unwrap_or(-1);
    let mut next_x = 0;
    for i in 0..n {
        if cells[i].is_none() {
            place(&mut cells, &mut occupied, i, (next_x, max_y + 1));
            next_x += 1;
        }
    }

    // Apply align directives: pin members to a shared row/column. Use the min of
    // current members' coords on the shared axis, and spread along the other.
    for (is_row, members) in &diag.aligns {
        let idxs: Vec<usize> = members.iter().filter_map(|m| index.get(m.as_str()).copied()).collect();
        if idxs.len() < 2 {
            continue;
        }
        if *is_row {
            // Shared y; spread x.
            let y = idxs.iter().filter_map(|&i| cells[i]).map(|c| c.1).min().unwrap_or(0);
            for (k, &i) in idxs.iter().enumerate() {
                cells[i] = Some((k as i32, y));
            }
        } else {
            // Shared x; spread y.
            let x = idxs.iter().filter_map(|&i| cells[i]).map(|c| c.0).min().unwrap_or(0);
            for (k, &i) in idxs.iter().enumerate() {
                cells[i] = Some((x, k as i32));
            }
        }
    }

    cells.into_iter().map(|c| c.unwrap_or((0, 0))).collect()
}

/// Anchor point on a placed box for a given port side.
fn port_point(p: &Placed, side: Side) -> (f32, f32) {
    match side {
        Side::L => (p.x, p.y + p.h / 2.0),
        Side::R => (p.x + p.w, p.y + p.h / 2.0),
        Side::T => (p.x + p.w / 2.0, p.y),
        Side::B => (p.x + p.w / 2.0, p.y + p.h),
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

const PAD: f32 = 24.0;
const JUNCTION_R: f32 = 6.0;

/// Render an `architecture-beta` diagram to an SVG document.
pub fn render_architecture(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diag = parse(src).map_err(MermaidError::Parse)?;
    if diag.services.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let index: HashMap<&str, usize> =
        diag.services.iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect();

    // Box sizes from title (+ icon subtext).
    let sizes: Vec<(f32, f32)> = diag
        .services
        .iter()
        .map(|s| {
            if s.junction {
                return (JUNCTION_R * 2.0, JUNCTION_R * 2.0);
            }
            let (tw, th) = text_size(&s.title, fs);
            let mut w = tw;
            let mut h = th;
            if s.icon.is_some() {
                let (iw, ih) = text_size(s.icon.as_deref().unwrap_or(""), fs * 0.75);
                w = w.max(iw);
                h += ih;
            }
            (w + 2.0 * opts.node_padding_x, h + 2.0 * opts.node_padding_y)
        })
        .collect();

    // Grid cells → pixels via uniform col/row pitch.
    let cells = assign_cells(&diag, &index);
    let cell_w = sizes.iter().map(|s| s.0).fold(60.0_f32, f32::max) + opts.node_sep;
    let cell_h = sizes.iter().map(|s| s.1).fold(40.0_f32, f32::max) + opts.rank_sep;
    let min_cx = cells.iter().map(|c| c.0).min().unwrap_or(0);
    let min_cy = cells.iter().map(|c| c.1).min().unwrap_or(0);

    let mut placed: Vec<Placed> = Vec::with_capacity(diag.services.len());
    for (i, &(cx, cy)) in cells.iter().enumerate() {
        let (w, h) = sizes[i];
        // Center the box within its cell.
        let cell_x = PAD + (cx - min_cx) as f32 * cell_w;
        let cell_y = PAD + (cy - min_cy) as f32 * cell_h;
        let x = cell_x + (cell_w - opts.node_sep - w).max(0.0) / 2.0;
        let y = cell_y + (cell_h - opts.rank_sep - h).max(0.0) / 2.0;
        placed.push(Placed { x, y, w, h });
    }

    // Group boundary rects: bounding box of member services, padded. Process
    // children before parents so a parent encloses its child boxes too.
    let group_idx: HashMap<&str, usize> =
        diag.groups.iter().enumerate().map(|(i, g)| (g.id.as_str(), i)).collect();
    // member service indices per group id
    let mut group_members: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, s) in diag.services.iter().enumerate() {
        if let Some(g) = &s.group {
            group_members.entry(g.clone()).or_default().push(i);
        }
    }

    let gpad = 14.0_f32;
    let gtitle_h = fs * 1.4;
    // Compute each group's rect. Iterate to let parents absorb child rects.
    let mut grects: HashMap<String, (f32, f32, f32, f32)> = HashMap::new();
    // Order groups so children come before parents (topological-ish by depth).
    let depth = |gid: &str| -> usize {
        let mut d = 0;
        let mut cur = Some(gid.to_string());
        let mut guard = 0;
        while let Some(c) = cur {
            guard += 1;
            if guard > 64 {
                break;
            }
            match group_idx.get(c.as_str()).and_then(|&gi| diag.groups[gi].parent.clone()) {
                Some(p) => {
                    d += 1;
                    cur = Some(p);
                }
                None => break,
            }
        }
        d
    };
    let mut order: Vec<usize> = (0..diag.groups.len()).collect();
    order.sort_by_key(|&gi| std::cmp::Reverse(depth(&diag.groups[gi].id)));

    for &gi in &order {
        let g = &diag.groups[gi];
        let mut minx = f32::MAX;
        let mut miny = f32::MAX;
        let mut maxx = f32::MIN;
        let mut maxy = f32::MIN;
        if let Some(members) = group_members.get(&g.id) {
            for &m in members {
                let p = &placed[m];
                minx = minx.min(p.x);
                miny = miny.min(p.y);
                maxx = maxx.max(p.x + p.w);
                maxy = maxy.max(p.y + p.h);
            }
        }
        // Absorb child-group rects.
        for cg in &diag.groups {
            if cg.parent.as_deref() == Some(g.id.as_str()) {
                if let Some(&(rx, ry, rw, rh)) = grects.get(&cg.id) {
                    minx = minx.min(rx);
                    miny = miny.min(ry);
                    maxx = maxx.max(rx + rw);
                    maxy = maxy.max(ry + rh);
                }
            }
        }
        if minx > maxx {
            // Empty group: skip (no rect).
            continue;
        }
        let rx = minx - gpad;
        let ry = miny - gpad - gtitle_h;
        let rw = (maxx - minx) + 2.0 * gpad;
        let rh = (maxy - miny) + 2.0 * gpad + gtitle_h;
        grects.insert(g.id.clone(), (rx, ry, rw, rh));
    }

    // Overall canvas bounds (include group rects).
    let mut max_x = 0.0_f32;
    let mut max_y = 0.0_f32;
    let mut min_x2 = f32::MAX;
    let mut min_y2 = f32::MAX;
    for p in &placed {
        max_x = max_x.max(p.x + p.w);
        max_y = max_y.max(p.y + p.h);
        min_x2 = min_x2.min(p.x);
        min_y2 = min_y2.min(p.y);
    }
    for &(rx, ry, rw, rh) in grects.values() {
        max_x = max_x.max(rx + rw);
        max_y = max_y.max(ry + rh);
        min_x2 = min_x2.min(rx);
        min_y2 = min_y2.min(ry);
    }
    // Shift so everything sits at >= PAD.
    let shift_x = PAD - min_x2.min(PAD);
    let shift_y = PAD - min_y2.min(PAD);
    for p in &mut placed {
        p.x += shift_x;
        p.y += shift_y;
    }
    let grects: HashMap<String, (f32, f32, f32, f32)> = grects
        .into_iter()
        .map(|(k, (rx, ry, rw, rh))| (k, (rx + shift_x, ry + shift_y, rw, rh)))
        .collect();
    max_x += shift_x;
    max_y += shift_y;

    let width = (max_x + PAD).ceil().max(1.0);
    let height = (max_y + PAD).ceil().max(1.0);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\">"
    );

    // 1) Group boundary rects (behind everything).
    let group_fill = lighten(opts.node_fill);
    for &gi in order.iter().rev() {
        let g = &diag.groups[gi];
        let Some(&(rx, ry, rw, rh)) = grects.get(&g.id) else {
            continue;
        };
        let _ = write!(
            svg,
            "<rect x=\"{rx:.1}\" y=\"{ry:.1}\" width=\"{rw:.1}\" height=\"{rh:.1}\" rx=\"8\" \
             fill=\"{f}\" fill-opacity=\"0.35\" stroke=\"{s}\"{so} stroke-width=\"1\" \
             stroke-dasharray=\"4 3\"/>",
            f = rgb(group_fill),
            s = rgb(opts.node_stroke),
            so = opacity_attr("stroke-opacity", opts.node_stroke),
        );
        // Group title.
        let _ = write!(
            svg,
            "<text x=\"{tx:.1}\" y=\"{ty:.1}\" font-family=\"{ff}\" font-size=\"{fz:.1}\" \
             font-weight=\"bold\" fill=\"{tc}\"{to}>{label}</text>",
            tx = rx + 8.0,
            ty = ry + gtitle_h * 0.75,
            ff = escape(&opts.font_family),
            fz = fs * 0.95,
            tc = rgb(opts.text_color),
            to = opacity_attr("fill", opts.text_color),
            label = escape(&g.title),
        );
    }

    // 2) Edges (between services).
    for e in &diag.edges {
        let (Some(&a), Some(&b)) = (index.get(e.lhs.as_str()), index.get(e.rhs.as_str())) else {
            continue;
        };
        let pa = &placed[a];
        let pb = &placed[b];
        let mut p0 = port_point(pa, e.lhs_side);
        let mut p1 = port_point(pb, e.rhs_side);
        // Nudge group-modifier endpoints slightly outward (toward group edge).
        if e.lhs_group {
            p0 = nudge(p0, e.lhs_side, 6.0);
        }
        if e.rhs_group {
            p1 = nudge(p1, e.rhs_side, 6.0);
        }
        let poly = route(p0, e.lhs_side, p1, e.rhs_side);

        let _ = write!(
            svg,
            "<polyline points=\"{pts}\" fill=\"none\" stroke=\"{s}\"{so} stroke-width=\"1.5\"/>",
            pts = poly
                .iter()
                .map(|p| format!("{:.1},{:.1}", p.0, p.1))
                .collect::<Vec<_>>()
                .join(" "),
            s = rgb(opts.edge_stroke),
            so = opacity_attr("stroke-opacity", opts.edge_stroke),
        );
        // Arrowheads.
        if e.rhs_arrow {
            arrowhead(&mut svg, &poly, true, e.rhs_side, opts);
        }
        if e.lhs_arrow {
            arrowhead(&mut svg, &poly, false, e.lhs_side, opts);
        }
        // Edge title at midpoint.
        if !e.title.is_empty() {
            let mid = poly[poly.len() / 2];
            let (lw, lh) = text_size(&e.title, fs * 0.85);
            let _ = write!(
                svg,
                "<rect x=\"{rx:.1}\" y=\"{ry:.1}\" width=\"{rw:.1}\" height=\"{rh:.1}\" \
                 fill=\"{bg}\" fill-opacity=\"0.85\"/>",
                rx = mid.0 - lw / 2.0 - 2.0,
                ry = mid.1 - lh / 2.0 - 1.0,
                rw = lw + 4.0,
                rh = lh + 2.0,
                bg = rgb(opts.background),
            );
            let _ = write!(
                svg,
                "<text x=\"{x:.1}\" y=\"{y:.1}\" text-anchor=\"middle\" \
                 font-family=\"{ff}\" font-size=\"{fz:.1}\" fill=\"{tc}\"{to}>{t}</text>",
                x = mid.0,
                y = mid.1 + fs * 0.3,
                ff = escape(&opts.font_family),
                fz = fs * 0.85,
                tc = rgb(opts.text_color),
                to = opacity_attr("fill", opts.text_color),
                t = escape(&e.title),
            );
        }
    }

    // 3) Service boxes.
    for (i, s) in diag.services.iter().enumerate() {
        let p = &placed[i];
        if s.junction {
            let _ = write!(
                svg,
                "<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"{r}\" fill=\"{f}\" stroke=\"{st}\"{so}/>",
                cx = p.x + p.w / 2.0,
                cy = p.y + p.h / 2.0,
                r = JUNCTION_R,
                f = rgb(opts.node_fill),
                st = rgb(opts.node_stroke),
                so = opacity_attr("stroke-opacity", opts.node_stroke),
            );
            continue;
        }
        let _ = write!(
            svg,
            "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{h:.1}\" rx=\"6\" \
             fill=\"{f}\"{fo} stroke=\"{st}\"{so} stroke-width=\"1\"/>",
            x = p.x,
            y = p.y,
            w = p.w,
            h = p.h,
            f = rgb(opts.node_fill),
            fo = opacity_attr("fill-opacity", opts.node_fill),
            st = rgb(opts.node_stroke),
            so = opacity_attr("stroke-opacity", opts.node_stroke),
        );
        // Title.
        let has_icon = s.icon.is_some();
        let title_y = if has_icon {
            p.y + opts.node_padding_y + fs
        } else {
            p.y + p.h / 2.0 + fs * 0.32
        };
        let _ = write!(
            svg,
            "<text x=\"{x:.1}\" y=\"{y:.1}\" text-anchor=\"middle\" font-family=\"{ff}\" \
             font-size=\"{fz:.1}\" fill=\"{tc}\"{to}>{t}</text>",
            x = p.x + p.w / 2.0,
            y = title_y,
            ff = escape(&opts.font_family),
            fz = fs,
            tc = rgb(opts.text_color),
            to = opacity_attr("fill", opts.text_color),
            t = escape(&s.title),
        );
        // Icon name as small grey subtext (no real icon drawn).
        if let Some(icon) = &s.icon {
            let _ = write!(
                svg,
                "<text x=\"{x:.1}\" y=\"{y:.1}\" text-anchor=\"middle\" font-family=\"{ff}\" \
                 font-size=\"{fz:.1}\" fill=\"{tc}\" fill-opacity=\"0.55\">{t}</text>",
                x = p.x + p.w / 2.0,
                y = p.y + p.h - opts.node_padding_y,
                ff = escape(&opts.font_family),
                fz = fs * 0.75,
                tc = rgb(opts.text_color),
                t = escape(icon),
            );
        }
    }

    let _ = write!(svg, "</svg>");

    Ok(MermaidRender { svg, width_px: width, height_px: height })
}

/// Lighten a color toward white (for faint group fills).
fn lighten(c: [u8; 4]) -> [u8; 4] {
    let mix = |v: u8| -> u8 { (v as u16 + (255 - v as u16) * 6 / 10) as u8 };
    [mix(c[0]), mix(c[1]), mix(c[2]), c[3]]
}

/// Move a point outward along a side direction.
fn nudge(p: (f32, f32), side: Side, d: f32) -> (f32, f32) {
    let (dx, dy) = side.step();
    (p.0 + dx as f32 * d, p.1 + dy as f32 * d)
}

/// Orthogonal route between two port points given their sides. Produces an
/// L-shaped 3-point poly when the sides imply a corner, else a 2-point line.
fn route(p0: (f32, f32), s0: Side, p1: (f32, f32), s1: Side) -> Vec<(f32, f32)> {
    let horiz0 = matches!(s0, Side::L | Side::R);
    let horiz1 = matches!(s1, Side::L | Side::R);
    if horiz0 && horiz1 {
        // Both horizontal ports: step out, meet at mid-x.
        let midx = (p0.0 + p1.0) / 2.0;
        vec![p0, (midx, p0.1), (midx, p1.1), p1]
    } else if !horiz0 && !horiz1 {
        let midy = (p0.1 + p1.1) / 2.0;
        vec![p0, (p0.0, midy), (p1.0, midy), p1]
    } else if horiz0 {
        // Left horizontal, right vertical → corner at (p1.x, p0.y).
        vec![p0, (p1.0, p0.1), p1]
    } else {
        vec![p0, (p0.0, p1.1), p1]
    }
}

/// Draw a small filled arrowhead at the `end` (true = last point) of the poly,
/// pointing along the entering segment.
fn arrowhead(svg: &mut String, poly: &[(f32, f32)], at_end: bool, side: Side, opts: &MermaidOptions) {
    if poly.len() < 2 {
        return;
    }
    let tip = if at_end { poly[poly.len() - 1] } else { poly[0] };
    // Direction points INTO the box: opposite of the port side's outward step.
    let (sx, sy) = side.step();
    let dir = (-sx as f32, -sy as f32);
    let len = 9.0_f32;
    let wid = 5.0_f32;
    let back = (tip.0 - dir.0 * len, tip.1 - dir.1 * len);
    let perp = (-dir.1, dir.0);
    let l = (back.0 + perp.0 * wid, back.1 + perp.1 * wid);
    let r = (back.0 - perp.0 * wid, back.1 - perp.1 * wid);
    let _ = write!(
        svg,
        "<polygon points=\"{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}\" fill=\"{f}\"{fo}/>",
        tip.0, tip.1, l.0, l.1, r.0, r.1,
        f = rgb(opts.edge_stroke),
        fo = opacity_attr("fill-opacity", opts.edge_stroke),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    #[test]
    fn parse_groups_services_and_edge() {
        let src = "architecture-beta
            group api(cloud)[API]
            service db(database)[Database] in api
            service server(server)[Server] in api
            db:L -- R:server
        ";
        let d = parse(src).unwrap();
        assert_eq!(d.groups.len(), 1);
        assert_eq!(d.groups[0].id, "api");
        assert_eq!(d.groups[0].title, "API");
        assert_eq!(d.groups[0].icon.as_deref(), Some("cloud"));

        assert_eq!(d.services.len(), 2);
        assert_eq!(d.services[0].id, "db");
        assert_eq!(d.services[0].title, "Database");
        assert_eq!(d.services[0].group.as_deref(), Some("api"));
        assert_eq!(d.services[1].id, "server");
        assert_eq!(d.services[1].group.as_deref(), Some("api"));

        assert_eq!(d.edges.len(), 1);
        let e = &d.edges[0];
        assert_eq!(e.lhs, "db");
        assert_eq!(e.lhs_side, Side::L);
        assert_eq!(e.rhs, "server");
        assert_eq!(e.rhs_side, Side::R);
        assert!(!e.lhs_arrow && !e.rhs_arrow);
    }

    #[test]
    fn parse_arrowed_edge_and_group_modifier() {
        let src = "architecture-beta
            service server(server)[Server] in g1
            service subnet(server)[Subnet] in g2
            server{group}:B --> T:subnet{group}
        ";
        let d = parse(src).unwrap();
        assert_eq!(d.edges.len(), 1);
        let e = &d.edges[0];
        assert_eq!(e.lhs, "server");
        assert_eq!(e.lhs_side, Side::B);
        assert!(e.lhs_group);
        assert_eq!(e.rhs, "subnet");
        assert_eq!(e.rhs_side, Side::T);
        assert!(e.rhs_group);
        assert!(e.rhs_arrow);
        assert!(!e.lhs_arrow);
    }

    #[test]
    fn parse_junction_and_nested_group() {
        let src = "architecture-beta
            group outer(cloud)[Outer]
            group inner(cloud)[Inner] in outer
            service a(server)[A] in inner
            junction j in inner
        ";
        let d = parse(src).unwrap();
        assert_eq!(d.groups.len(), 2);
        assert_eq!(d.groups[1].id, "inner");
        assert_eq!(d.groups[1].parent.as_deref(), Some("outer"));
        // junction is a service with junction=true.
        let j = d.services.iter().find(|s| s.id == "j").unwrap();
        assert!(j.junction);
        assert_eq!(j.group.as_deref(), Some("inner"));
    }

    #[test]
    fn parse_align_directive() {
        let src = "architecture-beta
            service a(server)[A]
            service b(server)[B]
            service c(server)[C]
            align column a b c
        ";
        let d = parse(src).unwrap();
        assert_eq!(d.aligns.len(), 1);
        assert!(!d.aligns[0].0); // column → is_row=false
        assert_eq!(d.aligns[0].1, vec!["a", "b", "c"]);
    }

    #[test]
    fn bad_header_errors() {
        assert!(matches!(parse("flowchart TD\n a-->b"), Err(_)));
    }

    #[test]
    fn empty_diagram_is_empty_error() {
        let r = render_architecture("architecture-beta\n", &opts());
        assert_eq!(r, Err(MermaidError::Empty));
    }

    #[test]
    fn render_wellformed_svg() {
        let src = "architecture-beta
            group api(cloud)[API]
            service db(database)[Database] in api
            service server(server)[Server] in api
            db:L -- R:server
        ";
        let r = render_architecture(src, &opts()).unwrap();
        assert!(r.svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(r.svg.ends_with("</svg>"));
        assert!(r.svg.contains("viewBox=\"0 0"));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_box_per_service_with_title() {
        let src = "architecture-beta
            service db(database)[Database]
            service server(server)[Server]
            db:R -- L:server
        ";
        let r = render_architecture(src, &opts()).unwrap();
        // Two service boxes → two <rect ... rx="6".
        let boxes = r.svg.matches("rx=\"6\"").count();
        assert_eq!(boxes, 2, "one rounded box per service");
        assert!(r.svg.contains(">Database</text>"));
        assert!(r.svg.contains(">Server</text>"));
    }

    #[test]
    fn render_group_boundary_rect() {
        let src = "architecture-beta
            group api(cloud)[API]
            service db(database)[Database] in api
            service server(server)[Server] in api
            db:R -- L:server
        ";
        let r = render_architecture(src, &opts()).unwrap();
        // Group rect uses a dashed boundary.
        assert!(r.svg.contains("stroke-dasharray=\"4 3\""));
        assert!(r.svg.contains(">API</text>"));
    }

    #[test]
    fn render_one_line_per_edge() {
        let src = "architecture-beta
            service a(server)[A]
            service b(server)[B]
            service c(server)[C]
            a:R -- L:b
            b:R -- L:c
        ";
        let r = render_architecture(src, &opts()).unwrap();
        let lines = r.svg.matches("<polyline").count();
        assert_eq!(lines, 2, "one polyline per edge");
    }

    #[test]
    fn render_arrowhead_for_directional_edge() {
        let src = "architecture-beta
            service a(server)[A]
            service b(server)[B]
            a:R --> L:b
        ";
        let r = render_architecture(src, &opts()).unwrap();
        assert_eq!(r.svg.matches("<polygon").count(), 1, "one arrowhead");
    }

    #[test]
    fn render_xml_escapes_titles() {
        let src = "architecture-beta
            service a(server)[A & <B>]
        ";
        let r = render_architecture(src, &opts()).unwrap();
        assert!(r.svg.contains("A &amp; &lt;B&gt;"));
        assert!(!r.svg.contains("A & <B>"));
    }

    #[test]
    fn render_is_deterministic() {
        let src = "architecture-beta
            group api(cloud)[API]
            service db(database)[Database] in api
            service server(server)[Server] in api
            db:L -- R:server
        ";
        let a = render_architecture(src, &opts()).unwrap();
        let b = render_architecture(src, &opts()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn title_falls_back_to_id() {
        let src = "architecture-beta
            service lonely(server)
        ";
        let d = parse(src).unwrap();
        assert_eq!(d.services[0].title, "lonely");
    }
}
