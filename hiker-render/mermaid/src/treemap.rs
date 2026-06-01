//! `treemap` diagram — indentation-based hierarchy laid out with a squarified
//! treemap (Bruls/Huizing/van Wijk), no dagre. Self-contained: parse + layout +
//! draw.
//!
//! ## Syntax (confirmed against the mermaid langium grammar)
//! Header keyword is `treemap` **or** `treemap-beta`. Rows are quoted names
//! nested by **indentation width** (number of leading spaces/tabs):
//! - Branch/section: `"Name"` (optionally `"Name":::className`)
//! - Leaf: `"Name" : <number>` or `"Name" , <number>` (the value separator is
//!   `:` or `,`; numbers may contain `,`/`_` group separators which are stripped)
//!
//! `title <text>`, `accTitle:`, `accDescr` and `%%` comments are ignored, as is
//! `classDef`/`:::` styling (parsed-around, not applied). A branch's value is the
//! sum of its leaf descendants. Single/double quotes are both accepted, and an
//! unquoted bare name is tolerated for leniency.

use crate::svgutil::{escape, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

/// A parsed treemap node: a branch (has `children`, `value` is the leaf sum) or a
/// leaf (`value` set, `children` empty).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub name: String,
    /// Explicit value for a leaf; for a branch this is filled in as the sum of
    /// its leaf descendants during [`compute_values`].
    pub value: Option<f64>,
    pub children: Vec<Node>,
    /// `true` for a leaf (a name with a value), `false` for a branch/section
    /// (a name with no value). A childless *branch* is still not a leaf — this
    /// distinguishes it, matching mermaid's Leaf-vs-Section AST split.
    pub leaf: bool,
}

impl Node {
    fn is_leaf(&self) -> bool {
        self.leaf
    }
}

/// Parse result: optional title plus the forest of top-level nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct Treemap {
    pub title: Option<String>,
    pub roots: Vec<Node>,
}

/// A flat row as it comes off a source line, before hierarchy building.
struct Row {
    indent: usize,
    name: String,
    value: Option<f64>,
    is_leaf: bool,
}

/// Parse treemap source into a tree. `Err` on a missing/bad header.
pub fn parse(src: &str) -> Result<Treemap, String> {
    // First non-blank, non-comment line must be the header.
    let mut header_seen = false;
    let mut title: Option<String> = None;
    let mut rows: Vec<Row> = Vec::new();

    for raw in src.lines() {
        // Strip trailing `%%` comments; keep leading whitespace (indentation).
        let line = strip_comment(raw);
        if line.trim().is_empty() {
            continue;
        }
        let trimmed = line.trim_start();

        if !header_seen {
            let first = trimmed.trim();
            let kw = first.split_whitespace().next().unwrap_or("");
            if kw == "treemap" || kw == "treemap-beta" {
                header_seen = true;
                continue;
            }
            return Err(format!("treemap: expected `treemap` header, got {first:?}"));
        }

        // Title / accessibility directives.
        if let Some(rest) = trimmed.strip_prefix("title") {
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                let t = rest.trim();
                if !t.is_empty() {
                    title = Some(t.to_string());
                }
                continue;
            }
        }
        if trimmed.starts_with("accTitle") || trimmed.starts_with("accDescr") {
            continue;
        }
        // Skip class styling directives — parsed around, not applied.
        if trimmed.starts_with("classDef") {
            continue;
        }

        let indent = line.len() - trimmed.len();
        if let Some(row) = parse_row(indent, trimmed) {
            rows.push(row);
        }
    }

    if !header_seen {
        return Err("treemap: empty input / missing `treemap` header".to_string());
    }

    let roots = build_hierarchy(&rows);
    Ok(Treemap { title, roots })
}

/// Remove a trailing `%%` comment (mermaid's `ML_COMMENT`).
fn strip_comment(line: &str) -> &str {
    match line.find("%%") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// Parse a single data row into a [`Row`]. Returns `None` if it has no name.
fn parse_row(indent: usize, content: &str) -> Option<Row> {
    let content = content.trim();
    if content.is_empty() {
        return None;
    }
    // Pull off the quoted (or bare) name.
    let (name, rest) = take_name(content);
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    // Strip a `:::class` selector from whatever remains.
    let rest = strip_class_selector(rest);
    let rest = rest.trim();

    // A leaf is `<sep> <number>` where sep is `:` or `,`.
    let mut value = None;
    let mut is_leaf = false;
    if let Some(after) = rest
        .strip_prefix(':')
        .or_else(|| rest.strip_prefix(','))
    {
        let num = after.trim();
        if !num.is_empty() {
            if let Some(v) = parse_number(num) {
                value = Some(v);
                is_leaf = true;
            }
        }
    }

    Some(Row {
        indent,
        name: name.to_string(),
        value,
        is_leaf,
    })
}

/// Split a row's content into (name, remainder). Honors `"..."`/`'...'` quoting;
/// falls back to taking up to the first `:`/`,` for an unquoted name.
fn take_name(content: &str) -> (String, &str) {
    let bytes = content.as_bytes();
    if let Some(&q) = bytes.first() {
        if q == b'"' || q == b'\'' {
            let quote = q as char;
            if let Some(end) = content[1..].find(quote) {
                let name = content[1..1 + end].to_string();
                let rest = &content[1 + end + 1..];
                return (name, rest);
            }
        }
    }
    // Unquoted: name runs until the first `:` or `,`.
    let cut = content.find([':', ',']).unwrap_or(content.len());
    (content[..cut].to_string(), &content[cut..])
}

/// Remove a trailing `:::className` class selector, returning the prefix.
fn strip_class_selector(s: &str) -> &str {
    match s.find(":::") {
        Some(i) => &s[..i],
        None => s,
    }
}

/// Parse a mermaid number: float with `,`/`_` group separators removed.
fn parse_number(s: &str) -> Option<f64> {
    let cleaned: String = s.chars().filter(|&c| c != ',' && c != '_').collect();
    cleaned.trim().parse::<f64>().ok()
}

/// Convert flat indented rows into a forest, exactly like mermaid's
/// `buildHierarchy`: a stack of (open branch, indent); pop while the top's indent
/// >= this row's indent, then attach to the new top (or the forest root).
fn build_hierarchy(rows: &[Row]) -> Vec<Node> {
    let mut roots: Vec<Node> = Vec::new();
    // Stack holds a path of indices into the tree to the currently-open branch.
    // Each entry: (path-of-child-indices, indent).
    let mut stack: Vec<(Vec<usize>, usize)> = Vec::new();

    for row in rows {
        let node = Node {
            name: row.name.clone(),
            value: if row.is_leaf { row.value } else { None },
            children: Vec::new(),
            leaf: row.is_leaf,
        };

        while let Some(&(_, lvl)) = stack.last() {
            if lvl >= row.indent {
                stack.pop();
            } else {
                break;
            }
        }

        let new_path = if let Some((parent_path, _)) = stack.last() {
            let parent = node_at_mut(&mut roots, parent_path);
            parent.children.push(node);
            let mut p = parent_path.clone();
            p.push(parent.children.len() - 1);
            p
        } else {
            roots.push(node);
            vec![roots.len() - 1]
        };

        if !row.is_leaf {
            stack.push((new_path, row.indent));
        }
    }

    compute_values(&mut roots);
    roots
}

/// Follow a path of child indices to a mutable node reference.
fn node_at_mut<'a>(roots: &'a mut [Node], path: &[usize]) -> &'a mut Node {
    let (first, rest) = path.split_first().expect("non-empty path");
    let mut node = &mut roots[*first];
    for &idx in rest {
        node = &mut node.children[idx];
    }
    node
}

/// Fill each branch's `value` with the sum of its leaf descendants (bottom-up).
fn compute_values(nodes: &mut [Node]) -> f64 {
    let mut total = 0.0;
    for n in nodes.iter_mut() {
        if n.is_leaf() {
            total += n.value.unwrap_or(0.0);
        } else {
            let sum = compute_values(&mut n.children);
            n.value = Some(sum);
            total += sum;
        }
    }
    total
}

// ---------------------------------------------------------------------------
// Squarified treemap layout
// ---------------------------------------------------------------------------

/// An axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn area(&self) -> f64 {
        self.w * self.h
    }
}

/// Lay weights `values` out within `rect` using the squarified algorithm. Returns
/// one rect per input weight, in input order. Zero/negative weights get empty
/// rects. Total tile area ≈ `rect.area()`, each tile ∝ its weight.
pub fn squarify(values: &[f64], rect: Rect) -> Vec<Rect> {
    let n = values.len();
    let mut out = vec![
        Rect {
            x: rect.x,
            y: rect.y,
            w: 0.0,
            h: 0.0
        };
        n
    ];
    let total: f64 = values.iter().map(|v| v.max(0.0)).sum();
    if total <= 0.0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return out;
    }

    // Scale weights to areas in the rect's units.
    let scale = rect.area() / total;
    // Indices of the still-to-place children, in order; areas in px².
    let mut remaining: Vec<usize> = (0..n).filter(|&i| values[i] > 0.0).collect();
    let areas: Vec<f64> = values.iter().map(|v| v.max(0.0) * scale).collect();

    let mut free = rect;
    let mut row: Vec<usize> = Vec::new();

    while !remaining.is_empty() {
        let side = free.w.min(free.h);
        let next = remaining[0];

        if row.is_empty() {
            row.push(next);
            remaining.remove(0);
            continue;
        }

        // Would adding `next` improve (lower) the row's worst aspect ratio?
        let cur = worst(&row, &areas, side);
        let with_next = {
            let mut r = row.clone();
            r.push(next);
            worst(&r, &areas, side)
        };

        if with_next <= cur {
            row.push(next);
            remaining.remove(0);
        } else {
            free = lay_row(&row, &areas, free, &mut out);
            row.clear();
        }
    }
    if !row.is_empty() {
        lay_row(&row, &areas, free, &mut out);
    }
    out
}

/// Worst (largest) aspect ratio in a row laid along the shorter side `side`.
fn worst(row: &[usize], areas: &[f64], side: f64) -> f64 {
    if row.is_empty() || side <= 0.0 {
        return f64::INFINITY;
    }
    let s: f64 = row.iter().map(|&i| areas[i]).sum();
    if s <= 0.0 {
        return f64::INFINITY;
    }
    let mut rmax: f64 = 0.0;
    let mut rmin = f64::INFINITY;
    for &i in row {
        rmax = rmax.max(areas[i]);
        rmin = rmin.min(areas[i]);
    }
    let side2 = side * side;
    let s2 = s * s;
    (side2 * rmax / s2).max(s2 / (side2 * rmin))
}

/// Place `row` as a strip along the shorter side of `free`, write the tiles into
/// `out`, and return the remaining free rectangle.
fn lay_row(row: &[usize], areas: &[f64], free: Rect, out: &mut [Rect]) -> Rect {
    let s: f64 = row.iter().map(|&i| areas[i]).sum();
    if s <= 0.0 {
        return free;
    }
    if free.w >= free.h {
        // Vertical strip on the left, full height, width = s / height.
        let strip_w = s / free.h;
        let mut y = free.y;
        for &i in row {
            let h = areas[i] / strip_w;
            out[i] = Rect {
                x: free.x,
                y,
                w: strip_w,
                h,
            };
            y += h;
        }
        Rect {
            x: free.x + strip_w,
            y: free.y,
            w: free.w - strip_w,
            h: free.h,
        }
    } else {
        // Horizontal strip on top, full width, height = s / width.
        let strip_h = s / free.w;
        let mut x = free.x;
        for &i in row {
            let w = areas[i] / strip_h;
            out[i] = Rect {
                x,
                y: free.y,
                w,
                h: strip_h,
            };
            x += w;
        }
        Rect {
            x: free.x,
            y: free.y + strip_h,
            w: free.w,
            h: free.h - strip_h,
        }
    }
}

// ---------------------------------------------------------------------------
// Draw
// ---------------------------------------------------------------------------

/// A flattened tile to draw, produced by recursively squarifying the tree.
struct Tile {
    rect: Rect,
    name: String,
    value: Option<f64>,
    depth: usize,
    is_leaf: bool,
    /// Palette index = top-level branch index this tile descends from.
    palette: usize,
}

/// Qualitative palette (mermaid-ish pastel sections).
const PALETTE: [[u8; 4]; 10] = [
    [124, 179, 222, 255],
    [241, 162, 142, 255],
    [160, 209, 152, 255],
    [199, 161, 209, 255],
    [240, 209, 140, 255],
    [142, 200, 200, 255],
    [223, 159, 191, 255],
    [176, 196, 158, 255],
    [156, 168, 222, 255],
    [222, 196, 156, 255],
];

const PLOT_W: f64 = 600.0;
const PLOT_H: f64 = 450.0;
const MARGIN: f64 = 16.0;
const INSET: f64 = 3.0;

/// Render a mermaid `treemap` diagram to SVG.
pub fn render_treemap(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let tm = parse(src).map_err(MermaidError::Parse)?;
    if count_leaves(&tm.roots) == 0 {
        return Err(MermaidError::Empty);
    }

    let title_h = if tm.title.is_some() {
        opts.font_size_px as f64 * 1.6
    } else {
        0.0
    };

    let plot = Rect {
        x: MARGIN,
        y: MARGIN + title_h,
        w: PLOT_W,
        h: PLOT_H,
    };
    let width = PLOT_W + 2.0 * MARGIN;
    let height = PLOT_H + 2.0 * MARGIN + title_h;

    // Layout the top-level forest, then recurse.
    let mut tiles: Vec<Tile> = Vec::new();
    let header_h = (opts.font_size_px as f64 * 1.3).max(14.0);
    layout_nodes(&tm.roots, plot, 0, None, header_h, &mut tiles);

    // Draw.
    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" \
         viewBox=\"0 0 {width:.0} {height:.0}\">"
    ));

    // Title.
    if let Some(t) = &tm.title {
        let fs = opts.font_size_px as f64 * 1.2;
        svg.push_str(&format!(
            "<text x=\"{cx:.2}\" y=\"{y:.2}\" text-anchor=\"middle\" \
             font-family=\"{ff}\" font-size=\"{fs:.2}\" font-weight=\"bold\" \
             fill=\"{col}\">{label}</text>",
            cx = width / 2.0,
            y = MARGIN + fs,
            ff = escape(&opts.font_family),
            col = rgb(opts.text_color),
            label = escape(t),
        ));
    }

    // Branches first (outlines + header bands), leaves on top.
    for tile in tiles.iter().filter(|t| !t.is_leaf) {
        draw_branch(&mut svg, tile, opts, header_h);
    }
    for tile in tiles.iter().filter(|t| t.is_leaf) {
        draw_leaf(&mut svg, tile, opts);
    }

    svg.push_str("</svg>");

    Ok(MermaidRender {
        svg,
        width_px: width as f32,
        height_px: height as f32,
    })
}

fn count_leaves(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| {
            if n.is_leaf() {
                1
            } else {
                count_leaves(&n.children)
            }
        })
        .sum()
}

/// Recursively squarify a list of sibling nodes into `rect`, emitting a [`Tile`]
/// per node and recursing into branches (inset + header band).
fn layout_nodes(
    nodes: &[Node],
    rect: Rect,
    depth: usize,
    palette: Option<usize>,
    header_h: f64,
    out: &mut Vec<Tile>,
) {
    let weights: Vec<f64> = nodes.iter().map(|n| n.value.unwrap_or(0.0)).collect();
    let rects = squarify(&weights, rect);

    for (i, (node, r)) in nodes.iter().zip(rects.iter()).enumerate() {
        // Top-level nodes seed their own palette slot; descendants inherit it.
        let pal = palette.unwrap_or(i);
        out.push(Tile {
            rect: *r,
            name: node.name.clone(),
            value: node.value,
            depth,
            is_leaf: node.is_leaf(),
            palette: pal,
        });
        if !node.is_leaf() {
            // Inset for the branch border, leaving a header band at the top.
            let inner = Rect {
                x: r.x + INSET,
                y: r.y + header_h,
                w: (r.w - 2.0 * INSET).max(0.0),
                h: (r.h - header_h - INSET).max(0.0),
            };
            if inner.w > 1.0 && inner.h > 1.0 {
                layout_nodes(&node.children, inner, depth + 1, Some(pal), header_h, out);
            }
        }
    }
}

/// Mix a palette color toward white by `t` (0 = palette, 1 = white).
fn lighten(c: [u8; 4], t: f64) -> [u8; 4] {
    let mix = |v: u8| (v as f64 + (255.0 - v as f64) * t).round() as u8;
    [mix(c[0]), mix(c[1]), mix(c[2]), c[3]]
}

fn draw_branch(svg: &mut String, tile: &Tile, opts: &MermaidOptions, header_h: f64) {
    let r = tile.rect;
    if r.w <= 0.0 || r.h <= 0.0 {
        return;
    }
    let base = PALETTE[tile.palette % PALETTE.len()];
    // Branch header band tinted lighter the deeper it is.
    let fill = lighten(base, 0.35 + 0.12 * tile.depth as f64);
    let stroke = opts.node_stroke;

    svg.push_str(&format!(
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         fill=\"none\" stroke=\"{st}\" stroke-width=\"1.5\"/>",
        x = r.x,
        y = r.y,
        w = r.w,
        h = r.h,
        st = rgb(stroke),
    ));
    // Header band.
    let band_h = header_h.min(r.h);
    svg.push_str(&format!(
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         fill=\"{fl}\" stroke=\"none\"/>",
        x = r.x,
        y = r.y,
        w = r.w,
        h = band_h,
        fl = rgb(fill),
    ));

    // Branch label, clipped to the band width.
    let fs = (opts.font_size_px as f64 * 0.85).min(band_h * 0.8);
    if fs >= 6.0 {
        if let Some(label) = fit_label(&tile.name, r.w - 6.0, fs) {
            svg.push_str(&format!(
                "<text x=\"{x:.2}\" y=\"{y:.2}\" font-family=\"{ff}\" \
                 font-size=\"{fs:.2}\" font-weight=\"bold\" fill=\"{col}\">{label}</text>",
                x = r.x + 4.0,
                y = r.y + band_h * 0.5 + fs * 0.35,
                ff = escape(&opts.font_family),
                col = rgb(opts.text_color),
                label = escape(&label),
            ));
        }
    }
}

fn draw_leaf(svg: &mut String, tile: &Tile, opts: &MermaidOptions) {
    let r = tile.rect;
    if r.w <= 0.0 || r.h <= 0.0 {
        return;
    }
    let base = PALETTE[tile.palette % PALETTE.len()];
    let fill = lighten(base, (0.05 + 0.1 * tile.depth as f64).min(0.45));

    svg.push_str(&format!(
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         fill=\"{fl}\" stroke=\"{st}\" stroke-width=\"1\"/>",
        x = r.x,
        y = r.y,
        w = r.w,
        h = r.h,
        fl = rgb(fill),
        st = rgb(opts.node_stroke),
    ));

    // Label + value, centered, only if they fit.
    let fs = (opts.font_size_px as f64).min(r.h * 0.5).min(r.w * 0.4);
    if fs < 6.0 {
        return;
    }
    let cx = r.x + r.w / 2.0;
    let cy = r.y + r.h / 2.0;

    if let Some(label) = fit_label(&tile.name, r.w - 4.0, fs) {
        let has_val = tile.value.is_some() && r.h >= fs * 2.4;
        let label_y = if has_val { cy - fs * 0.1 } else { cy + fs * 0.35 };
        svg.push_str(&format!(
            "<text x=\"{cx:.2}\" y=\"{y:.2}\" text-anchor=\"middle\" \
             font-family=\"{ff}\" font-size=\"{fs:.2}\" fill=\"{col}\">{label}</text>",
            y = label_y,
            ff = escape(&opts.font_family),
            col = rgb(opts.text_color),
            label = escape(&label),
        ));
        if has_val {
            let v = fmt_value(tile.value.unwrap());
            let vfs = fs * 0.8;
            svg.push_str(&format!(
                "<text x=\"{cx:.2}\" y=\"{y:.2}\" text-anchor=\"middle\" \
                 font-family=\"{ff}\" font-size=\"{vfs:.2}\" fill=\"{col}\">{label}</text>",
                y = cy + fs,
                ff = escape(&opts.font_family),
                col = rgb(opts.text_color),
                label = escape(&v),
            ));
        }
    }
}

/// Format a value, dropping a `.0` fractional part.
fn fmt_value(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Return `label` (possibly truncated with an ellipsis) if at least one
/// character fits in `max_w`, else `None`.
fn fit_label(label: &str, max_w: f64, fs: f64) -> Option<String> {
    if max_w <= 0.0 {
        return None;
    }
    let (w, _) = text_size(label, fs as f32);
    if (w as f64) <= max_w {
        return Some(label.to_string());
    }
    // Truncate to the number of chars that fit, leaving room for an ellipsis.
    let char_w = (text_size("M", fs as f32).0 as f64).max(1.0);
    let fit = (max_w / char_w).floor() as usize;
    if fit == 0 {
        return None;
    }
    let chars: Vec<char> = label.chars().collect();
    if fit >= chars.len() {
        return Some(label.to_string());
    }
    if fit <= 1 {
        return Some(chars[0].to_string());
    }
    let mut s: String = chars[..fit - 1].iter().collect();
    s.push('…');
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    #[test]
    fn parse_header_keywords() {
        assert!(parse("treemap\n\"A\": 1").is_ok());
        assert!(parse("treemap-beta\n\"A\": 1").is_ok());
        assert!(parse("flowchart TD\nA-->B").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn parse_small_tree() {
        // Root branch with 2 leaves + a sub-branch with 1 leaf.
        let src = "treemap\n\"Root\"\n  \"A\" : 10\n  \"B\" , 20\n  \"Sub\"\n    \"C\": 30\n";
        let tm = parse(src).unwrap();
        assert_eq!(tm.roots.len(), 1);
        let root = &tm.roots[0];
        assert_eq!(root.name, "Root");
        assert_eq!(root.children.len(), 3);

        let a = &root.children[0];
        assert_eq!(a.name, "A");
        assert_eq!(a.value, Some(10.0));
        assert!(a.is_leaf());

        let b = &root.children[1];
        assert_eq!(b.value, Some(20.0));

        let sub = &root.children[2];
        assert_eq!(sub.name, "Sub");
        assert!(!sub.is_leaf());
        assert_eq!(sub.children.len(), 1);
        assert_eq!(sub.children[0].name, "C");
        assert_eq!(sub.children[0].value, Some(30.0));

        // Branch values are leaf-descendant sums.
        assert_eq!(sub.value, Some(30.0));
        assert_eq!(root.value, Some(60.0));
    }

    #[test]
    fn parse_title_and_comments() {
        let src = "treemap\n\
            title My Diagram\n\
            %% a comment\n\
            \"X\" : 5  %% trailing comment\n";
        let tm = parse(src).unwrap();
        assert_eq!(tm.title.as_deref(), Some("My Diagram"));
        assert_eq!(tm.roots.len(), 1);
        assert_eq!(tm.roots[0].value, Some(5.0));
    }

    #[test]
    fn parse_number_separators_and_quotes() {
        let tm = parse("treemap\n'Item' : 1,234.5\n").unwrap();
        assert_eq!(tm.roots[0].name, "Item");
        assert_eq!(tm.roots[0].value, Some(1234.5));
    }

    #[test]
    fn squarify_covers_rect_proportionally() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 600.0,
            h: 400.0,
        };
        let weights = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let tiles = squarify(&weights, rect);
        assert_eq!(tiles.len(), weights.len());

        let total: f64 = weights.iter().sum();
        let rect_area = rect.area();
        let tile_area: f64 = tiles.iter().map(|t| t.area()).sum();
        // Tiles cover the rectangle.
        assert!(
            (tile_area - rect_area).abs() < 1e-3,
            "tile area {tile_area} vs rect area {rect_area}"
        );
        // Each tile's area is proportional to its weight.
        for (t, w) in tiles.iter().zip(weights.iter()) {
            let expect = w / total * rect_area;
            assert!(
                (t.area() - expect).abs() < 1e-3,
                "tile area {} vs expected {expect}",
                t.area()
            );
            // Tiles stay inside the rect.
            assert!(t.x >= rect.x - 1e-6 && t.y >= rect.y - 1e-6);
            assert!(t.x + t.w <= rect.x + rect.w + 1e-6);
            assert!(t.y + t.h <= rect.y + rect.h + 1e-6);
        }
    }

    #[test]
    fn squarify_no_overlap() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        let weights = [1.0, 1.0, 1.0, 1.0];
        let tiles = squarify(&weights, rect);
        // Pairwise: no two tiles overlap (allow shared edges).
        for i in 0..tiles.len() {
            for j in (i + 1)..tiles.len() {
                let a = &tiles[i];
                let b = &tiles[j];
                let disjoint = a.x + a.w <= b.x + 1e-6
                    || b.x + b.w <= a.x + 1e-6
                    || a.y + a.h <= b.y + 1e-6
                    || b.y + b.h <= a.y + 1e-6;
                assert!(disjoint, "tiles {i} and {j} overlap");
            }
        }
    }

    #[test]
    fn render_wellformed_and_one_rect_per_leaf() {
        let src = "treemap\ntitle T\n\"Root\"\n  \"A\" : 10\n  \"B\" : 20\n  \"Sub\"\n    \"C\": 30\n";
        let out = render_treemap(src, &opts()).unwrap();
        let svg = &out.svg;
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.contains("viewBox="));
        // 3 leaves → 3 filled leaf rects (fill=rgb...). Count all rects: 3 leaves
        // + branch outline + branch header band for Root and Sub = 3 + 2*2 = 7.
        let rect_count = svg.matches("<rect").count();
        assert_eq!(rect_count, 7, "rects: {rect_count}");
        // Labels present.
        assert!(svg.contains(">A</text>"));
        assert!(svg.contains(">Root</text>"));
        assert!(svg.contains(">T</text>")); // title
        assert!(svg.contains(">30</text>")); // a value
        assert!(out.width_px > 0.0 && out.height_px > 0.0);
    }

    #[test]
    fn render_xml_escapes() {
        let src = "treemap\n\"A & <B>\" : 5\n";
        let out = render_treemap(src, &opts()).unwrap();
        assert!(out.svg.contains("A &amp; &lt;B&gt;"));
        assert!(!out.svg.contains("A & <B>"));
    }

    #[test]
    fn render_empty_and_errors() {
        // Header but no leaves → Empty.
        assert_eq!(
            render_treemap("treemap\n\"OnlyBranch\"\n", &opts()),
            Err(MermaidError::Empty)
        );
        assert_eq!(render_treemap("treemap\n", &opts()), Err(MermaidError::Empty));
        // Bad header → Parse.
        assert!(matches!(
            render_treemap("notatreemap\n", &opts()),
            Err(MermaidError::Parse(_))
        ));
    }

    #[test]
    fn render_is_deterministic() {
        let src = "treemap\n\"R\"\n  \"A\":1\n  \"B\":2\n";
        let a = render_treemap(src, &opts()).unwrap();
        let b = render_treemap(src, &opts()).unwrap();
        assert_eq!(a, b);
    }
}
