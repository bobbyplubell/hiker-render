//! `wardley` (Wardley Map) diagram — self-contained: parse + 2-axis self-layout,
//! no dagre.
//!
//! Mermaid wardley syntax (the subset we support):
//! ```text
//! wardley-beta
//! title Tea Shop Value Chain
//!
//! anchor Business [0.95, 0.63]
//! component Cup of Tea [0.79, 0.61]
//! component Tea [0.63, 0.81]
//!
//! Business -> Cup of Tea
//! Cup of Tea -> Tea
//! ```
//! The header line is `wardley-beta` (a trailing `:` is allowed) or `wardley`.
//!
//! Directives implemented:
//! - `title <text>` — optional map title, drawn centered on top.
//! - `component <Name> [<visibility>, <evolution>]` — a node. The two values are
//!   in **OnlineWardleyMaps order**: the FIRST is *visibility* (the y / value
//!   chain axis, 0 = invisible/infrastructure … 1 = visible/user-facing) and the
//!   SECOND is *evolution* (the x axis, 0 = Genesis … 1 = Commodity). The name
//!   may be a `"quoted string"` or a bare `Name With Spaces`; a trailing
//!   `label [..]`, `(inertia)` and `(build|buy|outsource|market)` decorator are
//!   tolerated and ignored.
//! - `anchor <Name> [<visibility>, <evolution>]` — like a component but flagged
//!   as a user/anchor node (drawn with a bold label).
//! - `<A> -> <B>` (also `-->`) — a dependency link from A to B, drawn as a line
//!   between the two components' circles (under the circles).
//!
//! Skipped (noted, tolerated where they appear so a map still renders):
//! `size [..]`, `evolution <stages>`, `evolve`, pipelines, notes, annotations,
//! accelerators/deaccelerators, inertia/strategy decorators, link labels and
//! flow markers (`+>`, `+<`, `+<>`, `+'text'>`).
//!
//! Layout is pure 2-axis mapping — no graph layout. A plot rectangle is drawn
//! with the x-axis (evolution) at the bottom carrying the four stage labels
//! (Genesis / Custom / Product / Commodity) separated by vertical gridlines, and
//! a y-axis ("Value Chain", rotated, with Visible/Invisible hints) up the left.
//! Each component is a small circle at `x = ox + evolution*W`,
//! `y = oy + (1-visibility)*H` (visibility inverted: higher = up), with its name
//! beside it. Links are lines between the circles, drawn first (under the dots).
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/wardley/`.

use std::fmt::Write as _;

use crate::svgutil::{escape, rgb};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model

/// A plotted node (component or anchor) with normalized coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub name: String,
    /// Value-chain axis, 0..1 (bottom→top, higher = more visible to the user).
    pub visibility: f32,
    /// Evolution axis, 0..1 (left→right: Genesis→Commodity).
    pub evolution: f32,
    /// `anchor` nodes (users/customers) are drawn with a bold label.
    pub anchor: bool,
}

/// A dependency link between two components, by name.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub from: String,
    pub to: String,
}

/// A parsed Wardley map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WardleyMap {
    pub title: Option<String>,
    pub components: Vec<Component>,
    pub links: Vec<Link>,
}

impl WardleyMap {
    fn index_of(&self, name: &str) -> Option<usize> {
        self.components.iter().position(|c| c.name == name)
    }
}

/// The four evolution stage labels along the x-axis (Genesis→Commodity).
const STAGES: [&str; 4] = ["Genesis", "Custom-Built", "Product", "Commodity"];

// ---------------------------------------------------------------------------
// Parse

/// Parse wardley source into a [`WardleyMap`]. Returns an error message on a bad
/// header.
pub fn parse_wardley(src: &str) -> Result<WardleyMap, String> {
    let mut lines = src.lines();
    // Header: first non-blank, non-comment line must start with `wardley`.
    let mut header_ok = false;
    let mut first_rest = String::new();
    for raw in lines.by_ref() {
        let line = strip_comment(raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let kw = line.split_whitespace().next().unwrap_or("").trim_end_matches(':');
        if kw == "wardley-beta" || kw == "wardley" {
            header_ok = true;
            // Allow content after the header on the same line (rare); capture rest.
            first_rest = line[kw.len()..].trim_start_matches(':').trim().to_string();
            break;
        }
        return Err(format!("wardley: expected `wardley-beta` header, got {line:?}"));
    }
    if !header_ok {
        return Err("wardley: missing `wardley-beta` header".to_string());
    }

    let mut map = WardleyMap::default();
    // If there was trailing content on the header line, process it as a statement.
    if !first_rest.is_empty() {
        parse_stmt(&first_rest, &mut map);
    }
    for raw in lines {
        let line = strip_comment(raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        parse_stmt(line, &mut map);
    }
    Ok(map)
}

/// Strip a `%%` comment (the OWM/mermaid comment marker) from a line.
fn strip_comment(line: &str) -> &str {
    match line.find("%%") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// Parse a single (trimmed, comment-free, non-empty) statement into `map`.
fn parse_stmt(line: &str, map: &mut WardleyMap) {
    // title <text>
    if let Some(rest) = strip_keyword(line, "title") {
        map.title = Some(rest.trim().to_string());
        return;
    }
    // component / anchor <Name> [v, e] ...
    if let Some(rest) = strip_keyword(line, "component") {
        if let Some(c) = parse_component(rest, false) {
            map.components.push(c);
        }
        return;
    }
    if let Some(rest) = strip_keyword(line, "anchor") {
        if let Some(c) = parse_component(rest, true) {
            map.components.push(c);
        }
        return;
    }
    // Skipped statements (tolerated): size / evolution / evolve / pipeline /
    // note / annotation(s) / accelerator / deaccelerator. Recognize their
    // keywords so we don't mistake them for links.
    for kw in [
        "size",
        "evolution",
        "evolve",
        "pipeline",
        "note",
        "annotations",
        "annotation",
        "accelerator",
        "deaccelerator",
    ] {
        if strip_keyword(line, kw).is_some() {
            return;
        }
    }
    // Closing brace of a (skipped) pipeline block.
    if line == "}" || line == "{" {
        return;
    }
    // Otherwise: a link `A -> B` (or `A --> B`).
    if let Some(link) = parse_link(line) {
        map.links.push(link);
    }
}

/// If `line` starts with the whole word `kw` (followed by whitespace or EOL),
/// return the remainder after it; else `None`.
fn strip_keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    if rest.is_empty() {
        return Some(rest);
    }
    let c = rest.chars().next().unwrap();
    if c.is_whitespace() {
        Some(rest.trim_start())
    } else {
        None
    }
}

/// Parse the tail of a `component`/`anchor` statement: `<Name> [v, e] ...`.
fn parse_component(rest: &str, anchor: bool) -> Option<Component> {
    let open = rest.find('[')?;
    let close = rest[open..].find(']')? + open;
    let name = unquote(rest[..open].trim());
    if name.is_empty() {
        return None;
    }
    let inner = &rest[open + 1..close];
    let mut nums = inner.split(',');
    let visibility = parse_num(nums.next()?)?;
    let evolution = parse_num(nums.next()?)?;
    Some(Component {
        name,
        visibility: visibility.clamp(0.0, 1.0),
        evolution: evolution.clamp(0.0, 1.0),
        anchor,
    })
}

/// Parse a link `A -> B`, `A --> B`. Returns `None` if no arrow is found.
fn parse_link(line: &str) -> Option<Link> {
    // Find the arrow token. Try the longer form first.
    let (idx, arrow_len) = find_arrow(line)?;
    let from = unquote(line[..idx].trim());
    let mut to_part = line[idx + arrow_len..].trim();
    // Drop a trailing `; label` link annotation.
    if let Some(semi) = to_part.find(';') {
        to_part = to_part[..semi].trim_end();
    }
    let to = unquote(to_part.trim());
    if from.is_empty() || to.is_empty() {
        return None;
    }
    Some(Link { from, to })
}

/// Locate the first dependency arrow (`-->` or `->`) and return its byte index
/// and length. We only support the plain `->`/`-->` dependency forms.
fn find_arrow(line: &str) -> Option<(usize, usize)> {
    if let Some(i) = line.find("-->") {
        return Some((i, 3));
    }
    if let Some(i) = line.find("->") {
        return Some((i, 2));
    }
    None
}

/// Strip surrounding double quotes from a name, if present.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parse a coordinate number (e.g. `0.79`).
fn parse_num(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok()
}

// ---------------------------------------------------------------------------
// Layout constants

const MARGIN: f32 = 24.0;
const AXIS_GUTTER: f32 = 28.0; // left gutter for the rotated y-axis label
const PLOT_W: f32 = 720.0;
const PLOT_H: f32 = 480.0;
const STROKE_W: f32 = 1.0;
const DOT_R: f32 = 6.0;

// ---------------------------------------------------------------------------
// Render

/// Render a mermaid `wardley` diagram to SVG.
pub fn render_wardley(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let map = parse_wardley(src).map_err(MermaidError::Parse)?;
    if map.components.is_empty() {
        return Err(MermaidError::Empty);
    }
    let svg = draw(&map, opts);
    Ok(svg)
}

fn draw(map: &WardleyMap, opts: &MermaidOptions) -> MermaidRender {
    let fs = opts.font_size_px;
    let title_fs = fs * 1.15;
    let title_band = if map.title.is_some() { title_fs + MARGIN * 0.5 } else { 0.0 };

    // Plot origin (top-left corner of the plot rectangle).
    let ox = MARGIN + AXIS_GUTTER;
    let oy = title_band + MARGIN;
    let w_plot = PLOT_W;
    let h_plot = PLOT_H;

    let bottom_axis_band = fs * 1.6; // room under the plot for stage labels
    let width = ox + w_plot + MARGIN;
    let height = oy + h_plot + bottom_axis_band + MARGIN;

    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Plot→pixel mapping. x: evolution 0→ox, 1→ox+W. y: visibility inverted,
    // 0→bottom, 1→top.
    let px = |e: f32| ox + e.clamp(0.0, 1.0) * w_plot;
    let py = |v: f32| oy + (1.0 - v.clamp(0.0, 1.0)) * h_plot;

    let stroke = rgb(opts.edge_stroke);

    // Outer plot border.
    let _ = write!(
        svg,
        "<rect x=\"{ox:.2}\" y=\"{oy:.2}\" width=\"{w_plot:.2}\" height=\"{h_plot:.2}\" \
         fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{STROKE_W}\"/>",
    );

    // Vertical evolution gridlines at the 1/4, 2/4, 3/4 stage boundaries.
    for i in 1..4 {
        let gx = ox + (i as f32 / 4.0) * w_plot;
        let _ = write!(
            svg,
            "<line x1=\"{gx:.2}\" y1=\"{oy:.2}\" x2=\"{gx:.2}\" y2=\"{by:.2}\" \
             stroke=\"{stroke}\" stroke-width=\"{STROKE_W}\" stroke-dasharray=\"3,3\"/>",
            by = oy + h_plot,
        );
    }

    // Stage labels centered under each quarter of the x-axis.
    let stage_y = oy + h_plot + bottom_axis_band * 0.55;
    for (i, label) in STAGES.iter().enumerate() {
        let cx = ox + (i as f32 + 0.5) / 4.0 * w_plot;
        emit_text(&mut svg, label, cx, stage_y, fs * 0.85, opts, "middle", false);
    }

    // y-axis: rotated "Value Chain" label up the left gutter, with
    // Visible (top) / Invisible (bottom) hints inside the plot at the left edge.
    let y_axis_x = MARGIN + AXIS_GUTTER * 0.4;
    emit_text_rotated(&mut svg, "Value Chain", y_axis_x, oy + h_plot / 2.0, fs * 0.9, opts);
    emit_text(&mut svg, "Visible", ox + 4.0, oy + fs * 0.8, fs * 0.7, opts, "start", false);
    emit_text(
        &mut svg,
        "Invisible",
        ox + 4.0,
        oy + h_plot - fs * 0.6,
        fs * 0.7,
        opts,
        "start",
        false,
    );

    // Links first (under the circles): a line between the two components' dots.
    for link in &map.links {
        let (Some(a), Some(b)) = (map.index_of(&link.from), map.index_of(&link.to)) else {
            continue;
        };
        let ca = &map.components[a];
        let cb = &map.components[b];
        let _ = write!(
            svg,
            "<line x1=\"{x1:.2}\" y1=\"{y1:.2}\" x2=\"{x2:.2}\" y2=\"{y2:.2}\" \
             stroke=\"{stroke}\" stroke-width=\"{STROKE_W}\"/>",
            x1 = px(ca.evolution),
            y1 = py(ca.visibility),
            x2 = px(cb.evolution),
            y2 = py(cb.visibility),
        );
    }

    // Components: a small circle, name label to the right.
    let fill = rgb(opts.node_fill);
    let node_stroke = rgb(opts.node_stroke);
    for c in &map.components {
        let cx = px(c.evolution);
        let cy = py(c.visibility);
        let _ = write!(
            svg,
            "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{DOT_R}\" fill=\"{fill}\" \
             stroke=\"{node_stroke}\" stroke-width=\"1\"/>",
        );
        let lx = cx + DOT_R + 4.0;
        emit_text(&mut svg, &c.name, lx, cy, fs * 0.85, opts, "start", c.anchor);
    }

    // Title centered on top.
    if let Some(t) = &map.title {
        let tcx = w / 2.0;
        let ty = title_band / 2.0;
        let _ = write!(
            svg,
            "<text x=\"{tcx:.2}\" y=\"{ty:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{title_fs}\" font-weight=\"bold\" fill=\"{fill}\">{txt}</text>",
            family = escape(&opts.font_family),
            fill = rgb(opts.text_color),
            txt = escape(t),
        );
    }

    svg.push_str("</svg>");

    MermaidRender { svg, width_px: w, height_px: h }
}

/// A `<text>` anchored at (x, y), vertically centered, optionally bold.
fn emit_text(
    svg: &mut String,
    text: &str,
    x: f32,
    y: f32,
    fs: f32,
    opts: &MermaidOptions,
    anchor: &str,
    bold: bool,
) {
    let weight = if bold { " font-weight=\"bold\"" } else { "" };
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"{anchor}\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\"{weight} fill=\"{fill}\">{txt}</text>",
        family = escape(&opts.font_family),
        fill = rgb(opts.text_color),
        txt = escape(text),
    );
}

/// A `<text>` rotated -90° (reading bottom-to-top), centered on (x, y).
fn emit_text_rotated(svg: &mut String, text: &str, x: f32, y: f32, fs: f32, opts: &MermaidOptions) {
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         transform=\"rotate(-90 {x:.2} {y:.2})\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\">{txt}</text>",
        family = escape(&opts.font_family),
        fill = rgb(opts.text_color),
        txt = escape(text),
    );
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    const SAMPLE: &str = "wardley-beta\n\
        title Tea Shop\n\
        \n\
        anchor Business [0.95, 0.63]\n\
        component Cup of Tea [0.79, 0.61]\n\
        component Kettle [0.43, 0.35]\n\
        \n\
        Business -> Cup of Tea\n\
        Cup of Tea -> Kettle\n";

    #[test]
    fn parses_title_components_and_links() {
        let m = parse_wardley(SAMPLE).unwrap();
        assert_eq!(m.title.as_deref(), Some("Tea Shop"));
        assert_eq!(m.components.len(), 3);

        let business = &m.components[0];
        assert_eq!(business.name, "Business");
        assert!(business.anchor);
        assert!((business.visibility - 0.95).abs() < 1e-6);
        assert!((business.evolution - 0.63).abs() < 1e-6);

        let cup = &m.components[1];
        assert_eq!(cup.name, "Cup of Tea");
        assert!(!cup.anchor);
        assert!((cup.visibility - 0.79).abs() < 1e-6);
        assert!((cup.evolution - 0.61).abs() < 1e-6);

        assert_eq!(m.links.len(), 2);
        assert_eq!(m.links[0], Link { from: "Business".into(), to: "Cup of Tea".into() });
        assert_eq!(m.links[1], Link { from: "Cup of Tea".into(), to: "Kettle".into() });
    }

    #[test]
    fn parses_quoted_names_and_double_arrow() {
        let src = "wardley-beta\n\
            component \"Mobile App\" [0.80, 0.85]\n\
            component \"API Gateway\" [0.70, 0.65]\n\
            \"Mobile App\" --> \"API Gateway\"\n";
        let m = parse_wardley(src).unwrap();
        assert_eq!(m.components[0].name, "Mobile App");
        assert_eq!(m.components[1].name, "API Gateway");
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.links[0], Link { from: "Mobile App".into(), to: "API Gateway".into() });
    }

    #[test]
    fn tolerates_decorators_and_skipped_statements() {
        let src = "wardley-beta\n\
            size [1100, 600]\n\
            evolution Genesis -> Custom -> Product -> Commodity\n\
            component Legacy [0.45, 0.40] (inertia)\n\
            component App [0.65, 0.45] label [-50, 10] (build)\n\
            Legacy -> App\n\
            evolve App 0.80\n\
            note \"hi\" [0.3, 0.4]\n";
        let m = parse_wardley(src).unwrap();
        assert_eq!(m.components.len(), 2);
        assert_eq!(m.components[0].name, "Legacy");
        assert_eq!(m.components[1].name, "App");
        assert_eq!(m.links.len(), 1);
    }

    #[test]
    fn bad_header_errors() {
        let err = parse_wardley("flowchart TD\nA-->B\n").unwrap_err();
        assert!(err.contains("wardley"), "{err}");
    }

    #[test]
    fn empty_when_no_components() {
        let r = render_wardley("wardley-beta\ntitle Just A Title\n", &opts());
        assert_eq!(r, Err(MermaidError::Empty));
    }

    #[test]
    fn renders_wellformed_svg_with_axes_and_dots() {
        let r = render_wardley(SAMPLE, &opts()).unwrap();
        let s = &r.svg;
        assert!(s.starts_with("<svg"));
        assert!(s.ends_with("</svg>"));
        assert!(s.contains("viewBox="));
        assert!(s.contains("xmlns="));

        // Axis stage labels.
        for stage in STAGES {
            assert!(s.contains(stage), "missing stage {stage}");
        }
        assert!(s.contains("Value Chain"));
        assert!(s.contains("Visible"));

        // One circle per component (3), one line per link (2 links + 3 gridlines).
        assert_eq!(s.matches("<circle").count(), 3);
        let lines = s.matches("<line").count();
        assert_eq!(lines, 2 + 3, "2 links + 3 gridlines, got {lines}");

        // Names and title present.
        assert!(s.contains("Business"));
        assert!(s.contains("Cup of Tea"));
        assert!(s.contains("Kettle"));
        assert!(s.contains("Tea Shop"));

        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn dot_y_is_inverted_by_visibility() {
        // Higher visibility → smaller y (further up). Build two components, one
        // high one low, and check their circle cy values.
        let src = "wardley-beta\n\
            component High [0.90, 0.50]\n\
            component Low [0.10, 0.50]\n";
        let r = render_wardley(src, &opts()).unwrap();
        let cys: Vec<f32> = r
            .svg
            .match_indices("<circle")
            .map(|(i, _)| {
                let seg = &r.svg[i..];
                let cy_at = seg.find("cy=\"").unwrap() + 4;
                let end = seg[cy_at..].find('"').unwrap() + cy_at;
                seg[cy_at..end].parse::<f32>().unwrap()
            })
            .collect();
        assert_eq!(cys.len(), 2);
        // First component (High visibility) must be above (smaller y) the second.
        assert!(cys[0] < cys[1], "high visibility should be higher: {cys:?}");
    }

    #[test]
    fn dot_x_follows_evolution() {
        let src = "wardley-beta\n\
            component Genesis [0.50, 0.05]\n\
            component Commodity [0.50, 0.95]\n";
        let r = render_wardley(src, &opts()).unwrap();
        let cxs: Vec<f32> = r
            .svg
            .match_indices("<circle")
            .map(|(i, _)| {
                let seg = &r.svg[i..];
                let cx_at = seg.find("cx=\"").unwrap() + 4;
                let end = seg[cx_at..].find('"').unwrap() + cx_at;
                seg[cx_at..end].parse::<f32>().unwrap()
            })
            .collect();
        assert!(cxs[0] < cxs[1], "higher evolution should be further right: {cxs:?}");
    }

    #[test]
    fn xml_escapes_names() {
        let src = "wardley-beta\n\
            title A & B <map>\n\
            component \"X & Y\" [0.5, 0.5]\n";
        let r = render_wardley(src, &opts()).unwrap();
        assert!(r.svg.contains("A &amp; B &lt;map&gt;"));
        assert!(r.svg.contains("X &amp; Y"));
        assert!(!r.svg.contains("X & Y"));
    }

    #[test]
    fn deterministic() {
        let a = render_wardley(SAMPLE, &opts()).unwrap();
        let b = render_wardley(SAMPLE, &opts()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn link_to_unknown_component_is_dropped_in_draw() {
        // A link referencing a missing node parses fine but draws no line.
        let src = "wardley-beta\n\
            component A [0.5, 0.5]\n\
            A -> Ghost\n";
        let m = parse_wardley(src).unwrap();
        assert_eq!(m.links.len(), 1);
        let r = render_wardley(src, &opts()).unwrap();
        // Only the 3 gridlines, no link line (Ghost is unknown).
        assert_eq!(r.svg.matches("<line").count(), 3);
    }
}
