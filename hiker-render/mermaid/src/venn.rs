//! `venn` diagram — overlapping set circles, self-laid-out (no dagre).
//!
//! Mermaid *does* ship a native venn diagram (header `venn-beta`, files under
//! `references/mermaid/packages/mermaid/src/diagrams/venn/`), but it is an
//! external/plugin diagram whose layout is delegated to `@upsetjs/venn.js`
//! (force-directed circle packing driven by per-subset `size:` weights) and
//! whose grammar is fairly involved (`set A["Label"]:size`, `union A,B:size`,
//! indented `text` nodes, `style` rules). Reproducing venn.js' numeric layout
//! is out of scope, so this module defines its **own simple, member-based
//! syntax** and a deterministic fixed-geometry layout:
//!
//! ```text
//! venn
//!     title Hobbies
//!     set "Music": Alice, Bob, Carol
//!     set "Sports": Bob, Carol, Dave
//!     set "Art": Carol, Eve
//! ```
//!
//! Header `venn` (the dispatcher in `lib.rs` routes it here; `venn-beta` is
//! also accepted), an optional `title`, and one `set "<name>": <members>` line
//! per set. Members are optional. From which sets contain a member we derive
//! its **region** (the intersection of those sets) and place it near that
//! region's centroid. We lay out **2 or 3** sets as the classic overlapping
//! arrangement; for **>3** sets we fall back to placing the circles evenly
//! around a ring (overlaps are then only approximate — a noted limitation).

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

/// A parsed set: a name and its (ordered, de-duplicated) members.
#[derive(Clone, Debug, PartialEq)]
pub struct VennSet {
    pub name: String,
    pub members: Vec<String>,
}

/// A parsed venn diagram.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Venn {
    pub title: Option<String>,
    pub sets: Vec<VennSet>,
}

/// Parse venn source into a [`Venn`].
///
/// Accepts the `venn` / `venn-beta` header, an optional `title <text>`, and
/// `set "<name>": <comma-separated members>` lines. The quotes around the name
/// and the member list are both optional (`set Music: Alice, Bob` works, as
/// does `set "Music"`). Returns [`MermaidError::Parse`] on a bad/missing header.
pub fn parse_venn(src: &str) -> Result<Venn, MermaidError> {
    let mut lines = src.lines().map(strip_comment).filter(|l| !l.trim().is_empty());

    // Header.
    let header = lines
        .next()
        .map(|l| l.trim().to_string())
        .ok_or(MermaidError::Parse("empty input".to_string()))?;
    let head_kw = header.split_whitespace().next().unwrap_or("");
    if head_kw != "venn" && head_kw != "venn-beta" {
        return Err(MermaidError::Parse(format!(
            "expected `venn` header, got {header:?}"
        )));
    }

    let mut venn = Venn::default();
    for raw in lines {
        let line = raw.trim();
        if let Some(rest) = strip_kw(line, "title") {
            let t = rest.trim();
            if !t.is_empty() {
                venn.title = Some(t.to_string());
            }
        } else if let Some(rest) = strip_kw(line, "set") {
            let (name, members) = parse_set_line(rest)?;
            venn.sets.push(VennSet { name, members });
        } else if let Some(rest) = strip_kw(line, "overlap") {
            // `overlap A & B: m1, m2` — add the listed members to each named set
            // so they land in the intersection region. (Our own extension.)
            apply_overlap(&mut venn, rest)?;
        } else {
            return Err(MermaidError::Parse(format!("unrecognized line: {line:?}")));
        }
    }
    Ok(venn)
}

/// `<rest>` of a `set` line: `"<name>": a, b, c` or `<name>: a, b, c` or just
/// `"<name>"`. Returns (name, members).
fn parse_set_line(rest: &str) -> Result<(String, Vec<String>), MermaidError> {
    let rest = rest.trim();
    let (name, after) = if let Some(stripped) = rest.strip_prefix('"') {
        // Quoted name: read to the closing quote.
        let end = stripped
            .find('"')
            .ok_or_else(|| MermaidError::Parse(format!("unterminated set name in {rest:?}")))?;
        (stripped[..end].to_string(), stripped[end + 1..].trim_start())
    } else {
        // Unquoted: name runs up to the first colon (or whole string).
        match rest.find(':') {
            Some(i) => (rest[..i].trim().to_string(), &rest[i..]),
            None => (rest.trim().to_string(), ""),
        }
    };
    if name.is_empty() {
        return Err(MermaidError::Parse(format!("empty set name in {rest:?}")));
    }
    let members = match after.trim_start().strip_prefix(':') {
        Some(list) => split_members(list),
        None => Vec::new(),
    };
    Ok((name, members))
}

/// `overlap A & B: m1, m2` — append members to each `&`-separated set name.
fn apply_overlap(venn: &mut Venn, rest: &str) -> Result<(), MermaidError> {
    let (names_part, members_part) = match rest.split_once(':') {
        Some((n, m)) => (n, m),
        None => (rest, ""),
    };
    let members = split_members(members_part);
    let names: Vec<String> = names_part
        .split('&')
        .map(|n| dequote(n.trim()).to_string())
        .filter(|n| !n.is_empty())
        .collect();
    if names.len() < 2 {
        return Err(MermaidError::Parse(format!(
            "overlap needs >=2 sets joined by `&`: {rest:?}"
        )));
    }
    for name in &names {
        match venn.sets.iter_mut().find(|s| &s.name == name) {
            Some(s) => {
                for m in &members {
                    if !s.members.contains(m) {
                        s.members.push(m.clone());
                    }
                }
            }
            None => {
                return Err(MermaidError::Parse(format!(
                    "overlap references unknown set {name:?}"
                )))
            }
        }
    }
    Ok(())
}

/// Split a comma-separated member list, trimming and dropping empties.
fn split_members(list: &str) -> Vec<String> {
    list.split(',')
        .map(|m| dequote(m.trim()).to_string())
        .filter(|m| !m.is_empty())
        .collect()
}

/// Strip surrounding double quotes if present.
fn dequote(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Remove a trailing `%%` comment.
fn strip_comment(line: &str) -> &str {
    match line.find("%%") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// If `line` starts with keyword `kw` followed by whitespace (or is exactly
/// `kw`), return the remainder; else `None`.
fn strip_kw<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    if rest.is_empty() {
        Some("")
    } else if rest.starts_with(|c: char| c.is_whitespace()) {
        Some(rest.trim_start())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Layout + draw
// ---------------------------------------------------------------------------

/// A per-set color palette (straight RGBA, alpha applied at draw time).
const PALETTE: [[u8; 3]; 6] = [
    [122, 158, 217], // blue
    [232, 126, 102], // red/orange
    [128, 196, 128], // green
    [212, 178, 90],  // gold
    [170, 130, 200], // purple
    [110, 200, 200], // teal
];

/// Translucent fill alpha for the set circles (~0.4).
const FILL_ALPHA: u8 = 102;

struct Circle {
    cx: f32,
    cy: f32,
    r: f32,
    color: [u8; 3],
}

/// Render a venn diagram to an SVG document.
pub fn render_venn(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let venn = parse_venn(src)?;
    if venn.sets.is_empty() {
        return Err(MermaidError::Empty);
    }

    let n = venn.sets.len();
    let r: f32 = 110.0;
    // Center-to-center distance giving a clear, visible overlap.
    let d: f32 = r * 1.1;

    // Compute raw circle centers (before normalizing to a positive viewBox).
    let centers: Vec<(f32, f32)> = match n {
        1 => vec![(0.0, 0.0)],
        2 => vec![(-d / 2.0, 0.0), (d / 2.0, 0.0)],
        3 => {
            // Equilateral-ish triangle: two on top, one below-center.
            let dy = d * 0.5;
            vec![(-d / 2.0, -dy * 0.6), (d / 2.0, -dy * 0.6), (0.0, dy * 1.0)]
        }
        _ => {
            // >3: place evenly around a ring. Overlaps are only approximate.
            let ring = r * 1.05;
            (0..n)
                .map(|i| {
                    let a = std::f32::consts::TAU * (i as f32) / (n as f32)
                        - std::f32::consts::FRAC_PI_2;
                    (ring * a.cos(), ring * a.sin())
                })
                .collect()
        }
    };

    let circles: Vec<Circle> = centers
        .iter()
        .enumerate()
        .map(|(i, &(cx, cy))| Circle {
            cx,
            cy,
            r,
            color: PALETTE[i % PALETTE.len()],
        })
        .collect();

    let font = opts.font_size_px;

    // Label positions for set names: just outside each circle, pushed radially
    // away from the diagram center (0,0). For a lone set, put it on top.
    let set_label_pos: Vec<(f32, f32)> = circles
        .iter()
        .map(|c| {
            let (mut ux, mut uy) = (c.cx, c.cy);
            let len = (ux * ux + uy * uy).sqrt();
            if len < 1e-3 {
                (ux, uy) = (0.0, -1.0);
            } else {
                ux /= len;
                uy /= len;
            }
            (c.cx + ux * (c.r + font * 0.8), c.cy + uy * (c.r + font * 0.8))
        })
        .collect();

    // Member placement: each member's region is the set of circles whose set
    // contains it; we place it at that region's centroid. Group members by
    // region so co-located members stack vertically instead of overlapping.
    let member_points = layout_members(&venn, &circles);

    // ---- Bounds ----
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut grow = |x: f32, y: f32| {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    };
    for c in &circles {
        grow(c.cx - c.r, c.cy - c.r);
        grow(c.cx + c.r, c.cy + c.r);
    }
    // Account for set labels (estimate width).
    for (set, &(lx, ly)) in venn.sets.iter().zip(&set_label_pos) {
        let (tw, th) = text_size(&set.name, font);
        grow(lx - tw / 2.0, ly - th / 2.0);
        grow(lx + tw / 2.0, ly + th / 2.0);
    }

    let margin = 16.0;
    let title_h = if venn.title.is_some() { font * 1.6 } else { 0.0 };

    let content_x = min_x - margin;
    let content_y = min_y - margin;
    let width = (max_x - min_x) + margin * 2.0;
    let height = (max_y - min_y) + margin * 2.0 + title_h;

    // Translate so the SVG origin is (0,0): shift everything by (-content_x,
    // -content_y + title_h).
    let ox = -content_x;
    let oy = -content_y + title_h;

    // ---- Emit SVG ----
    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.1}\" height=\"{height:.1}\" \
         viewBox=\"0 0 {width:.1} {height:.1}\">",
    ));

    // Title (centered on top).
    if let Some(title) = &venn.title {
        svg.push_str(&format!(
            "<text x=\"{x:.1}\" y=\"{y:.1}\" text-anchor=\"middle\" \
             font-family=\"{ff}\" font-size=\"{fs:.1}\" font-weight=\"bold\" \
             fill=\"{fill}\"{op}>{t}</text>",
            x = width / 2.0,
            y = font * 1.1,
            ff = escape(&opts.font_family),
            fs = font * 1.1,
            fill = rgb(opts.text_color),
            op = opacity_attr("fill-opacity", opts.text_color),
            t = escape(title),
        ));
    }

    // Circles (translucent fill so overlaps blend).
    let fill_color = [0u8, 0, 0, FILL_ALPHA];
    for c in &circles {
        let col = [c.color[0], c.color[1], c.color[2], FILL_ALPHA];
        svg.push_str(&format!(
            "<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"{r:.1}\" fill=\"{f}\"{fo} \
             stroke=\"{s}\" stroke-width=\"2\"{so}/>",
            cx = c.cx + ox,
            cy = c.cy + oy,
            r = c.r,
            f = rgb(col),
            fo = opacity_attr("fill-opacity", fill_color),
            s = rgb([c.color[0], c.color[1], c.color[2], 255]),
            so = "",
        ));
    }

    // Member labels (drawn under the set names so names stay legible).
    for (label, (px, py)) in &member_points {
        svg.push_str(&format!(
            "<text x=\"{x:.1}\" y=\"{y:.1}\" text-anchor=\"middle\" \
             dominant-baseline=\"middle\" font-family=\"{ff}\" font-size=\"{fs:.1}\" \
             fill=\"{fill}\"{op}>{t}</text>",
            x = px + ox,
            y = py + oy,
            ff = escape(&opts.font_family),
            fs = font * 0.85,
            fill = rgb(opts.text_color),
            op = opacity_attr("fill-opacity", opts.text_color),
            t = escape(label),
        ));
    }

    // Set name labels.
    for (set, &(lx, ly)) in venn.sets.iter().zip(&set_label_pos) {
        svg.push_str(&format!(
            "<text x=\"{x:.1}\" y=\"{y:.1}\" text-anchor=\"middle\" \
             dominant-baseline=\"middle\" font-family=\"{ff}\" font-size=\"{fs:.1}\" \
             font-weight=\"bold\" fill=\"{fill}\"{op}>{t}</text>",
            x = lx + ox,
            y = ly + oy,
            ff = escape(&opts.font_family),
            fs = font,
            fill = rgb(opts.text_color),
            op = opacity_attr("fill-opacity", opts.text_color),
            t = escape(&set.name),
        ));
    }

    svg.push_str("</svg>");

    Ok(MermaidRender {
        svg,
        width_px: width,
        height_px: height,
    })
}

/// Compute on-canvas positions for member labels. Each member belongs to the
/// **region** formed by the indices of the sets that contain it — the set of
/// points inside ALL those circles and OUTSIDE every other circle. We find a
/// point genuinely in that region's interior by sampling a grid over the
/// circles' bounding box (so even a thin lens gets a real interior point), then
/// stack co-located members vertically within the region's vertical extent. A
/// member is emitted **once** even if listed in several sets. Returns
/// `(label, (x, y))` in deterministic order.
fn layout_members(venn: &Venn, circles: &[Circle]) -> Vec<(String, (f32, f32))> {
    // member name -> sorted set indices that contain it (its region).
    let mut regions: Vec<(String, Vec<usize>)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (si, set) in venn.sets.iter().enumerate() {
        for m in &set.members {
            if let Some(pos) = seen.iter().position(|x| x == m) {
                regions[pos].1.push(si);
            } else {
                seen.push(m.clone());
                regions.push((m.clone(), vec![si]));
            }
        }
    }

    // Group members by identical region key so we can stack them.
    // key (sorted set indices) -> Vec<member name>, preserving first-seen order.
    let mut groups: Vec<(Vec<usize>, Vec<String>)> = Vec::new();
    for (name, idxs) in regions {
        match groups.iter_mut().find(|(k, _)| *k == idxs) {
            Some((_, names)) => names.push(name),
            None => groups.push((idxs, vec![name])),
        }
    }

    // Bounding box of all circles, for the sampling grid.
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for c in circles {
        min_x = min_x.min(c.cx - c.r);
        min_y = min_y.min(c.cy - c.r);
        max_x = max_x.max(c.cx + c.r);
        max_y = max_y.max(c.cy + c.r);
    }

    // Deterministic sampling resolution.
    const GRID: usize = 80;

    let font = 16.0_f32; // line spacing scale; actual font set at draw time.
    let line_h = font * 0.95;
    let mut out = Vec::new();
    for (idxs, names) in groups {
        // Sample the bounding box; keep points inside ALL of `idxs`' circles and
        // outside every other circle. Accumulate the centroid and track the
        // points so we can space members along the region's vertical extent at
        // the centroid x.
        let mut sum_x = 0.0f32;
        let mut sum_y = 0.0f32;
        let mut hits = 0u32;
        // Vertical span of region points near the centroid x (collected after we
        // know the centroid in a second pass would be ideal; instead we record
        // per-column samples). To stay single-pass and deterministic we record
        // all hit y's and the x range, then derive a column.
        let mut hit_ys: Vec<(f32, f32)> = Vec::new(); // (x, y) of region samples

        for gy in 0..GRID {
            for gx in 0..GRID {
                let fx = (gx as f32 + 0.5) / GRID as f32;
                let fy = (gy as f32 + 0.5) / GRID as f32;
                let px = min_x + fx * (max_x - min_x);
                let py = min_y + fy * (max_y - min_y);
                let mut ok = true;
                for (ci, c) in circles.iter().enumerate() {
                    let dx = px - c.cx;
                    let dy = py - c.cy;
                    let inside = dx * dx + dy * dy <= c.r * c.r;
                    let want_inside = idxs.contains(&ci);
                    if inside != want_inside {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    sum_x += px;
                    sum_y += py;
                    hits += 1;
                    hit_ys.push((px, py));
                }
            }
        }

        let (cx, cy) = if hits > 0 {
            (sum_x / hits as f32, sum_y / hits as f32)
        } else {
            // Empty region (geometry doesn't form this combination): fall back to
            // the centroid of the involved circle centers.
            let mut fx = 0.0f32;
            let mut fy = 0.0f32;
            for &i in &idxs {
                fx += circles[i].cx;
                fy += circles[i].cy;
            }
            let count = idxs.len().max(1) as f32;
            (fx / count, fy / count)
        };

        let total = names.len();
        if total == 1 || hits == 0 {
            // Single member (or no region samples to bound the stack): center on
            // the centroid, stacking around it for the fallback multi case.
            for (k, name) in names.iter().enumerate() {
                let y = cy + (k as f32 - (total as f32 - 1.0) / 2.0) * line_h;
                out.push((name.clone(), (cx, y)));
            }
        } else {
            // Multiple members: distribute them along the region's vertical
            // extent at the centroid x, keeping each inside the region. Find the
            // y-range of region samples in the column nearest the centroid x.
            let col_w = (max_x - min_x) / GRID as f32;
            let mut col_min_y = f32::INFINITY;
            let mut col_max_y = f32::NEG_INFINITY;
            for &(x, y) in &hit_ys {
                if (x - cx).abs() <= col_w {
                    col_min_y = col_min_y.min(y);
                    col_max_y = col_max_y.max(y);
                }
            }
            if !col_min_y.is_finite() {
                // No samples in the central column; fall back to overall y-range.
                for &(_, y) in &hit_ys {
                    col_min_y = col_min_y.min(y);
                    col_max_y = col_max_y.max(y);
                }
            }
            // Inset a little so labels don't sit on the region edge.
            let pad = ((col_max_y - col_min_y) * 0.12).min(line_h * 0.5);
            let lo = col_min_y + pad;
            let hi = col_max_y - pad;
            let span = (hi - lo).max(0.0);
            // Desired stack height; if it exceeds the region, compress to fit.
            let want = (total as f32 - 1.0) * line_h;
            let step = if want > span && total > 1 {
                span / (total as f32 - 1.0)
            } else {
                line_h
            };
            let used = (total as f32 - 1.0) * step;
            let start = cy - used / 2.0;
            for (k, name) in names.iter().enumerate() {
                let y = start + k as f32 * step;
                out.push((name.clone(), (cx, y)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    const SRC3: &str = "venn\n\
        title Hobbies\n\
        set \"Music\": Alice, Bob, Carol\n\
        set \"Sports\": Bob, Carol, Dave\n\
        set \"Art\": Carol, Eve\n";

    #[test]
    fn parses_title_and_sets_with_members() {
        let v = parse_venn(SRC3).unwrap();
        assert_eq!(v.title.as_deref(), Some("Hobbies"));
        let names: Vec<&str> = v.sets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Music", "Sports", "Art"]);
        assert_eq!(v.sets[0].members, ["Alice", "Bob", "Carol"]);
        assert_eq!(v.sets[1].members, ["Bob", "Carol", "Dave"]);
        assert_eq!(v.sets[2].members, ["Carol", "Eve"]);
    }

    #[test]
    fn parses_two_sets_unquoted_and_venn_beta_header() {
        let src = "venn-beta\nset Music: Alice, Bob\nset Sports: Bob, Carol\n";
        let v = parse_venn(src).unwrap();
        assert_eq!(v.sets.len(), 2);
        assert_eq!(v.sets[0].name, "Music");
        assert_eq!(v.sets[1].members, ["Bob", "Carol"]);
        assert!(v.title.is_none());
    }

    #[test]
    fn set_without_members_ok() {
        let src = "venn\nset \"A\"\nset \"B\"\n";
        let v = parse_venn(src).unwrap();
        assert_eq!(v.sets.len(), 2);
        assert!(v.sets[0].members.is_empty());
    }

    #[test]
    fn overlap_directive_adds_shared_members() {
        let src = "venn\nset \"A\": x\nset \"B\": y\noverlap A & B: z\n";
        let v = parse_venn(src).unwrap();
        assert!(v.sets[0].members.contains(&"z".to_string()));
        assert!(v.sets[1].members.contains(&"z".to_string()));
    }

    #[test]
    fn bad_header_is_parse_error() {
        assert!(matches!(
            parse_venn("flowchart TD\nA-->B"),
            Err(MermaidError::Parse(_))
        ));
    }

    #[test]
    fn empty_input_is_parse_error() {
        assert!(matches!(parse_venn("   \n\n"), Err(MermaidError::Parse(_))));
    }

    #[test]
    fn no_sets_is_empty_error() {
        let r = render_venn("venn\ntitle Only\n", &opts());
        assert_eq!(r.unwrap_err(), MermaidError::Empty);
    }

    #[test]
    fn renders_well_formed_svg_with_circles_and_labels() {
        let out = render_venn(SRC3, &opts()).unwrap();
        let svg = &out.svg;
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains("viewBox=\"0 0"));
        // One circle per set.
        assert_eq!(svg.matches("<circle").count(), 3);
        // Translucent fill on the circles.
        assert!(svg.contains("fill-opacity="));
        // Set labels present.
        assert!(svg.contains(">Music<"));
        assert!(svg.contains(">Sports<"));
        assert!(svg.contains(">Art<"));
        // Title present.
        assert!(svg.contains(">Hobbies<"));
        assert!(out.width_px > 0.0 && out.height_px > 0.0);
    }

    #[test]
    fn two_set_render_has_two_circles() {
        let src = "venn\nset \"A\": p\nset \"B\": q\n";
        let out = render_venn(src, &opts()).unwrap();
        assert_eq!(out.svg.matches("<circle").count(), 2);
    }

    #[test]
    fn shared_member_appears_once() {
        // "Carol" is in all three sets; it must be emitted a single time.
        let out = render_venn(SRC3, &opts()).unwrap();
        assert_eq!(out.svg.matches(">Carol<").count(), 1);
        // And a member unique to one set also appears once.
        assert_eq!(out.svg.matches(">Eve<").count(), 1);
    }

    #[test]
    fn xml_escaped() {
        let src = "venn\nset \"A & <B>\": x\n";
        let out = render_venn(src, &opts()).unwrap();
        assert!(out.svg.contains("A &amp; &lt;B&gt;"));
        assert!(!out.svg.contains("A & <B>"));
    }

    #[test]
    fn deterministic() {
        let a = render_venn(SRC3, &opts()).unwrap();
        let b = render_venn(SRC3, &opts()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn more_than_three_sets_renders_ring() {
        let src = "venn\nset A: a\nset B: b\nset C: c\nset D: d\n";
        let out = render_venn(src, &opts()).unwrap();
        assert_eq!(out.svg.matches("<circle").count(), 4);
    }
}
