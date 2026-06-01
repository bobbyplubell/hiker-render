//! `state` diagram (`stateDiagram` / `stateDiagram-v2`).
//!
//! Self-contained: parse → build a `hiker_graph` layered (dagre) graph →
//! lay out → draw one SVG document. Supported subset:
//!
//! * states `s1` and described states `s1 : Some text` (label = text).
//! * start / end pseudo-state `[*]`: as a transition **source** it is a start
//!   (small filled circle); as a **target** it is an end (filled circle with an
//!   outer ring). A single synthetic start node and a single synthetic end node
//!   are shared across all occurrences (matching mermaid's one-start/one-end).
//! * transitions `s1 --> s2` and `s1 --> s2 : label`.
//!
//! Skipped (note in report): composite/nested `state X { ... }`, `--` /
//! concurrency, `note`, choice / fork / join.

use std::collections::HashMap;
use std::fmt::Write as _;

use hiker_graph::layered::RankDir;
use hiker_graph::{GraphInput, LayeredEngine, LayoutEngine, Vec2};

use crate::svgutil::{edge_label_anchor, escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

/// Synthetic id for the shared start pseudo-state.
const START_ID: &str = "\0start";
/// Synthetic id for the shared end pseudo-state.
const END_ID: &str = "\0end";
/// Diameter of a pseudo-state circle, px.
const PSEUDO_SIZE: f32 = 18.0;

/// A state node. `pseudo` is `None` for a real state, or `Start`/`End` for the
/// two synthetic pseudo-states.
#[derive(Clone, Debug, PartialEq)]
struct State {
    id: String,
    label: String,
    pseudo: Option<Pseudo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pseudo {
    Start,
    End,
}

/// A transition `from --> to` with an optional label.
#[derive(Clone, Debug, PartialEq)]
struct Transition {
    from: String,
    to: String,
    label: Option<String>,
}

/// Parsed state diagram.
#[derive(Clone, Debug, Default, PartialEq)]
struct StateDiagram {
    /// States in first-seen order.
    states: Vec<State>,
    transitions: Vec<Transition>,
}

/// Parse a state diagram source. Errors on a missing/wrong header.
fn parse(src: &str) -> Result<StateDiagram, String> {
    // Header: first non-blank, non-comment line must be stateDiagram[-v2].
    let mut saw_header = false;
    let mut diag = StateDiagram::default();
    let mut index_of: HashMap<String, usize> = HashMap::new();

    // Walk every line; the first meaningful one is the header.
    let mut pending_header = true;
    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if pending_header {
            let kw = line.split_whitespace().next().unwrap_or("");
            if kw != "stateDiagram" && kw != "stateDiagram-v2" {
                return Err(format!("expected `stateDiagram` header, got {kw:?}"));
            }
            saw_header = true;
            pending_header = false;
            continue;
        }
        // `direction TB` etc. — accepted but only the default Tb is used in v1.
        if line.starts_with("direction") {
            continue;
        }
        // Unsupported block/feature lines: skip quietly.
        if line.starts_with("note")
            || line.starts_with("state ")
            || line == "}"
            || line.starts_with("--")
        {
            continue;
        }

        parse_line(line, &mut diag, &mut index_of);
    }

    if !saw_header {
        return Err("empty input / no stateDiagram header".to_string());
    }
    Ok(diag)
}

/// Parse one body line: either a transition (`a --> b [: label]`) or a state
/// declaration / description (`s1` or `s1 : text`).
fn parse_line(line: &str, diag: &mut StateDiagram, index_of: &mut HashMap<String, usize>) {
    if let Some(idx) = line.find("-->") {
        let from_raw = line[..idx].trim();
        let rest = line[idx + 3..].trim();
        // Optional `: label` after the target.
        let (to_raw, label) = match rest.split_once(':') {
            Some((t, l)) => (t.trim(), Some(l.trim().to_string())),
            None => (rest, None),
        };
        if from_raw.is_empty() || to_raw.is_empty() {
            return;
        }
        let from = ensure_state(from_raw, true, diag, index_of);
        let to = ensure_state(to_raw, false, diag, index_of);
        diag.transitions.push(Transition {
            from,
            to,
            label: label.filter(|l| !l.is_empty()),
        });
        return;
    }

    // State declaration or description: `s1` or `s1 : description`.
    if let Some((id_raw, desc)) = line.split_once(':') {
        let id_raw = id_raw.trim();
        if id_raw.is_empty() {
            return;
        }
        let id = ensure_state(id_raw, false, diag, index_of);
        let desc = desc.trim();
        if !desc.is_empty() {
            if let Some(&i) = index_of.get(&id) {
                diag.states[i].label = desc.to_string();
            }
        }
    } else {
        // Bare state id.
        ensure_state(line, false, diag, index_of);
    }
}

/// Upsert a state by its raw token. `[*]` maps to the shared start pseudo-state
/// when `as_source` is true, otherwise the shared end pseudo-state. Returns the
/// canonical id used in the graph.
fn ensure_state(
    raw: &str,
    as_source: bool,
    diag: &mut StateDiagram,
    index_of: &mut HashMap<String, usize>,
) -> String {
    let (id, label, pseudo) = if raw == "[*]" {
        if as_source {
            (START_ID.to_string(), String::new(), Some(Pseudo::Start))
        } else {
            (END_ID.to_string(), String::new(), Some(Pseudo::End))
        }
    } else {
        (raw.to_string(), raw.to_string(), None)
    };

    if !index_of.contains_key(&id) {
        index_of.insert(id.clone(), diag.states.len());
        diag.states.push(State {
            id: id.clone(),
            label,
            pseudo,
        });
    }
    id
}

/// Render a mermaid `state` diagram to SVG.
pub fn render_state(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diag = parse(src).map_err(MermaidError::Parse)?;
    if diag.states.is_empty() {
        return Err(MermaidError::Empty);
    }

    // id → node index, first-seen order (matches dagre node indices).
    let index_of: HashMap<&str, u32> = diag
        .states
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i as u32))
        .collect();

    // Node sizes: real states from text + padding, pseudo-states fixed.
    let sizes: Vec<(f32, f32)> = diag
        .states
        .iter()
        .map(|s| match s.pseudo {
            Some(_) => (PSEUDO_SIZE, PSEUDO_SIZE),
            None => {
                let (tw, th) = text_size(&s.label, opts.font_size_px);
                (tw + 2.0 * opts.node_padding_x, th + 2.0 * opts.node_padding_y)
            }
        })
        .collect();

    // Build the dagre edge list; keep the mapping back to original transitions.
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(diag.transitions.len());
    let mut kept: Vec<usize> = Vec::with_capacity(diag.transitions.len());
    // Per-edge label box size (aligned to `edges`) so dagre reserves a gap and
    // positions the label there; None for unlabeled transitions.
    let mut label_sizes: Vec<Option<Vec2>> = Vec::with_capacity(diag.transitions.len());
    for (j, t) in diag.transitions.iter().enumerate() {
        if let (Some(&a), Some(&b)) =
            (index_of.get(t.from.as_str()), index_of.get(t.to.as_str()))
        {
            edges.push((a, b));
            kept.push(j);
            label_sizes.push(
                t.label
                    .as_deref()
                    .filter(|l| !l.is_empty())
                    .map(|l| {
                        let (w, h) = text_size(l, opts.font_size_px);
                        Vec2::new(w + 10.0, h + 6.0)
                    }),
            );
        }
    }

    let node_sizes: Vec<Vec2> = sizes.iter().map(|&(w, h)| Vec2::new(w, h)).collect();
    let engine = LayeredEngine {
        rankdir: RankDir::Tb,
        ranksep: opts.rank_sep,
        nodesep: opts.node_sep,
        edgesep: 20.0,
        default_node_size: Vec2::new(50.0, 50.0),
    };
    let out = engine.layout(&GraphInput {
        node_count: diag.states.len(),
        edges: &edges,
        node_sizes: Some(&node_sizes),
        edge_label_sizes: Some(&label_sizes),
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
    emit_defs(&mut svg, opts);

    // Group edges by unordered endpoint pair so bidirectional / parallel
    // transitions spread their labels instead of stacking at one midpoint.
    let mut pair_members: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
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

    // Edges first so node fills paint over the line ends.
    for (dagre_idx, &orig) in kept.iter().enumerate() {
        let route = out.edge_routes.get(dagre_idx);
        let pts: Vec<(f32, f32)> = route
            .map(|r| r.iter().map(|p| (p.x, p.y)).collect())
            .unwrap_or_default();
        let (idx, cnt) = group[dagre_idx];
        // Dagre's reserved label center, if it placed one for this edge.
        let dagre_label = out.edge_label_positions.get(dagre_idx).copied().flatten();
        emit_edge(
            &mut svg,
            &pts,
            diag.transitions[orig].label.as_deref(),
            idx,
            cnt,
            dagre_label,
            opts,
        );
    }

    // Nodes.
    for (i, s) in diag.states.iter().enumerate() {
        let pos = out.positions.get(i).copied().unwrap_or(Vec2::ZERO);
        let (w, h) = sizes[i];
        emit_node(&mut svg, s, pos.x, pos.y, w, h, opts);
    }

    svg.push_str("</svg>");

    Ok(MermaidRender {
        svg,
        width_px: width,
        height_px: height,
    })
}

/// Arrowhead marker shared by every transition.
fn emit_defs(svg: &mut String, opts: &MermaidOptions) {
    let len = 9.0_f32;
    let half = 4.0_f32;
    let _ = write!(
        svg,
        "<defs><marker id=\"state-arrow\" markerWidth=\"{len}\" markerHeight=\"{w}\" \
         refX=\"{len}\" refY=\"{half}\" orient=\"auto\" markerUnits=\"userSpaceOnUse\">\
         <path d=\"M0,0 L{len},{half} L0,{w} Z\" fill=\"{fill}\"{fo}/></marker></defs>",
        w = half * 2.0,
        fill = rgb(opts.edge_stroke),
        fo = opacity_attr("fill-opacity", opts.edge_stroke),
    );
}

/// One transition polyline (with an arrowhead and optional centered label).
/// `index`/`count` give the edge's position within its parallel group so the
/// label is nudged perpendicular to the route for bidirectional/parallel pairs.
#[allow(clippy::too_many_arguments)]
fn emit_edge(
    svg: &mut String,
    points: &[(f32, f32)],
    label: Option<&str>,
    index: usize,
    count: usize,
    dagre_label: Option<Vec2>,
    opts: &MermaidOptions,
) {
    if points.len() < 2 {
        return;
    }
    let mut pts = points.to_vec();
    pullback_end(&mut pts, 9.0);

    let mut d = String::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        let cmd = if i == 0 { 'M' } else { 'L' };
        let _ = write!(d, "{cmd}{x:.2},{y:.2} ");
    }
    let _ = write!(
        svg,
        "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\"{so} \
         marker-end=\"url(#state-arrow)\"/>",
        d.trim_end(),
        stroke = rgb(opts.edge_stroke),
        so = opacity_attr("stroke-opacity", opts.edge_stroke),
    );

    if let Some(label) = label.filter(|l| !l.is_empty()) {
        // Prefer dagre's reserved label center; fall back to the
        // perpendicular-nudged midpoint when dagre didn't place it.
        let anchor = match dagre_label {
            Some(p) => Some((p.x, p.y)),
            None => edge_label_anchor(points, index, count, opts.font_size_px),
        };
        if let Some((cx, cy)) = anchor {
            emit_label(svg, label, cx, cy, opts);
        }
    }
}

/// A state node: pseudo-states are circles, real states are rounded rects.
fn emit_node(
    svg: &mut String,
    s: &State,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    opts: &MermaidOptions,
) {
    match s.pseudo {
        Some(Pseudo::Start) => {
            // Small solid filled circle.
            let _ = write!(
                svg,
                "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" fill=\"{fill}\"{fo}/>",
                r = w / 2.0,
                fill = rgb(opts.node_stroke),
                fo = opacity_attr("fill-opacity", opts.node_stroke),
            );
        }
        Some(Pseudo::End) => {
            // Outer ring + inner solid circle.
            let _ = write!(
                svg,
                "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" fill=\"none\" \
                 stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>\
                 <circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{ri:.2}\" fill=\"{fill}\"{fo}/>",
                r = w / 2.0,
                ri = (w / 2.0 - 4.0).max(1.0),
                stroke = rgb(opts.node_stroke),
                so = opacity_attr("stroke-opacity", opts.node_stroke),
                fill = rgb(opts.node_stroke),
                fo = opacity_attr("fill-opacity", opts.node_stroke),
            );
        }
        None => {
            let x = cx - w / 2.0;
            let y = cy - h / 2.0;
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
                 rx=\"6\" ry=\"6\" fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"1.5\"/>",
                fill = rgb(opts.node_fill),
                fo = opacity_attr("fill-opacity", opts.node_fill),
                stroke = rgb(opts.node_stroke),
                so = opacity_attr("stroke-opacity", opts.node_stroke),
            );
            emit_label(svg, &s.label, cx, cy, opts);
        }
    }
}

/// Centered `<text>` (single line) at `(cx, cy)`.
fn emit_label(svg: &mut String, label: &str, cx: f32, cy: f32, opts: &MermaidOptions) {
    if label.is_empty() {
        return;
    }
    let _ = write!(
        svg,
        "<text x=\"{cx:.2}\" y=\"{cy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\"{fo}>{txt}</text>",
        family = escape(&opts.font_family),
        fs = opts.font_size_px,
        fill = rgb(opts.text_color),
        fo = opacity_attr("fill-opacity", opts.text_color),
        txt = escape(label),
    );
}

/// Shorten the polyline's last segment by `amount` px so an arrowhead tip lands
/// on the target border.
fn pullback_end(pts: &mut [(f32, f32)], amount: f32) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let (tx, ty) = pts[n - 1];
    let (px, py) = pts[n - 2];
    let (dx, dy) = (tx - px, ty - py);
    let len = dx.hypot(dy);
    if len <= amount || len == 0.0 {
        return;
    }
    let t = (len - amount) / len;
    pts[n - 1] = (px + dx * t, py + dy * t);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    #[test]
    fn parse_states_and_transitions() {
        let src = "stateDiagram-v2\n  s1 --> s2\n  s2 --> s3 : go";
        let d = parse(src).unwrap();
        assert_eq!(d.states.len(), 3);
        assert_eq!(d.states[0].id, "s1");
        assert_eq!(d.transitions.len(), 2);
        assert_eq!(d.transitions[1].label.as_deref(), Some("go"));
    }

    #[test]
    fn parse_description() {
        let src = "stateDiagram\n  s1 : First state\n  s1 --> s2";
        let d = parse(src).unwrap();
        // s1 created by the description, label set to the text.
        assert_eq!(d.states[0].id, "s1");
        assert_eq!(d.states[0].label, "First state");
    }

    #[test]
    fn start_and_end_pseudo_states() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> [*]";
        let d = parse(src).unwrap();
        // start, s1, end → three nodes.
        assert_eq!(d.states.len(), 3);
        assert_eq!(d.states[0].pseudo, Some(Pseudo::Start));
        assert_eq!(d.states[1].id, "s1");
        assert_eq!(d.states[2].pseudo, Some(Pseudo::End));
    }

    #[test]
    fn bad_header_errors() {
        assert!(parse("graph TD\n a --> b").is_err());
    }

    #[test]
    fn empty_input_errors() {
        // No header at all.
        assert!(parse("\n\n").is_err());
    }

    #[test]
    fn render_wellformed_svg() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> s2 : next\n  s2 --> [*]";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_node_and_edge_counts() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> s2\n  s2 --> [*]";
        let r = render_state(src, &opts()).unwrap();
        // Two real states → two <rect>.
        assert_eq!(r.svg.matches("<rect").count(), 2);
        // Three transitions → three edge paths (each references the arrow marker).
        assert_eq!(r.svg.matches("marker-end=\"url(#state-arrow)\"").count(), 3);
    }

    #[test]
    fn start_and_end_markers_drawn() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> [*]";
        let r = render_state(src, &opts()).unwrap();
        // Start = 1 circle, end = 2 circles → 3 <circle> total.
        assert_eq!(r.svg.matches("<circle").count(), 3);
    }

    #[test]
    fn edge_label_rendered() {
        let src = "stateDiagram-v2\n  s1 --> s2 : hello";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains(">hello<"));
    }

    #[test]
    fn xml_escapes_label() {
        let src = "stateDiagram-v2\n  s1 : a & b < c\n  s1 --> s2";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains("a &amp; b &lt; c"));
        assert!(!r.svg.contains("a & b"));
    }

    #[test]
    fn empty_diagram_errors() {
        // Header only, no states.
        assert_eq!(render_state("stateDiagram-v2\n", &opts()), Err(MermaidError::Empty));
    }

    #[test]
    fn bidirectional_labels_separated() {
        // Idle<->Running with both directions labeled: the two labels must not
        // overlap (the "stostart" bug). Both texts render, at distinct y.
        let src = "stateDiagram-v2\n  Idle --> Running : start\n  Running --> Idle : stop";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains(">start<"));
        assert!(r.svg.contains(">stop<"));

        // Read the (x, y) anchor of each label's <text> element; the two must
        // differ in at least one coordinate (perpendicular nudge).
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
        let s = label_xy(&r.svg, "start");
        let t = label_xy(&r.svg, "stop");
        assert!(
            (s.0 - t.0).abs() > 1.0 || (s.1 - t.1).abs() > 1.0,
            "bidirectional labels overlap: start={s:?}, stop={t:?}"
        );
    }

    #[test]
    fn deterministic() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> s2 : x\n  s2 --> [*]";
        let a = render_state(src, &opts()).unwrap();
        let b = render_state(src, &opts()).unwrap();
        assert_eq!(a, b);
    }
}
