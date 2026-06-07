//! Sankey diagram (self-contained: parse + self-layout + draw, no dagre).
//!
//! Mermaid sankey syntax (CSV-like):
//! ```text
//! sankey-beta
//!
//! Agricultural 'waste',Bio-conversion,124.729
//! Bio-conversion,Liquid,0.597
//! "Quoted, node","Other ""quoted""",10
//! ```
//! The header line is `sankey-beta` (or bare `sankey`), case-insensitive — see
//! `references/mermaid/packages/mermaid/src/diagrams/sankey/sankeyDetector.ts`
//! (`/^\s*sankey(-beta)?/`) and `parser/sankey.jison`. Subsequent non-empty,
//! non-`%%`-comment lines are `source,target,value` rows. Fields may be bare
//! ([ -~] minus comma/quote) or double-quoted, in which case an
//! embedded quote is written `""` and commas are allowed inside. `value` is a
//! decimal number.
//!
//! ## Layout (self-laid-out flow, no dagre)
//! - Nodes are collected in first-seen order; links are weighted directed edges.
//! - **Columns (x):** each node's column = longest path from a source (a node
//!   with no incoming link). Sources are column 0; a target's column is
//!   `max(source columns) + 1`. Cycles (shouldn't occur in a valid sankey) are
//!   broken by a visited guard so layering always terminates.
//! - **Heights:** a node's flow weight = `max(sum inflow, sum outflow)`; the
//!   busiest column's total weight (+ gaps) is scaled to the plot height, and
//!   every node/ribbon uses that one px-per-unit scale.
//! - **Stacking:** within a column nodes are stacked top-to-bottom in first-seen
//!   order (v1 — no crossing minimization), separated by a small gap.
//! - **Ribbons:** each link is a translucent cubic-Bezier band from the source
//!   bar's right edge to the target bar's left edge, thickness = scaled value.
//!   Each node tracks its consumed in/out offset so stacked ribbons don't
//!   overlap along a bar edge.
//!
//! See `references/.../sankey/sankeyRenderer.ts` for the upstream d3-sankey
//! renderer this approximates (d3 uses an iterative relaxation; we use a simpler
//! deterministic longest-path layering).

use std::fmt::Write as _;

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// A weighted directed link, by node index into [`Sankey::nodes`].
#[derive(Clone, Debug, PartialEq)]
struct Link {
    source: usize,
    target: usize,
    value: f64,
}

/// A parsed sankey diagram: node names (first-seen order) and links.
#[derive(Clone, Debug, PartialEq)]
struct Sankey {
    nodes: Vec<String>,
    links: Vec<Link>,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse mermaid sankey source. Returns `Err(message)` when the header is
/// missing/malformed or a data row is malformed.
fn parse_sankey(src: &str) -> Result<Sankey, String> {
    let mut lines = src.lines();

    // Header: first non-blank, non-comment line must be `sankey`/`sankey-beta`.
    let mut saw_header = false;
    let mut header_consumed_data: Option<String> = None;
    for raw in lines.by_ref() {
        let line = strip_comment(raw);
        if line.trim().is_empty() {
            continue;
        }
        // The header keyword may be glued to data (e.g. `sankey-betaA,B,1`), as
        // the upstream lexer switches state mid-line; split it off.
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("sankey-beta") {
            saw_header = true;
            let rest = &trimmed[trimmed.len() - rest.len()..];
            if !rest.trim().is_empty() {
                header_consumed_data = Some(rest.to_string());
            }
        } else if let Some(rest) = lower.strip_prefix("sankey") {
            // Make sure it's the keyword, not a node literally named "sankey...".
            // The header is always exactly `sankey`/`sankey-beta` on its own; if
            // there's a comma before any keyword boundary it's not a header.
            saw_header = true;
            let rest = &trimmed[trimmed.len() - rest.len()..];
            if !rest.trim().is_empty() {
                header_consumed_data = Some(rest.to_string());
            }
        } else {
            return Err(format!(
                "expected `sankey-beta` header, found {:?}",
                trimmed.chars().take(40).collect::<String>()
            ));
        }
        break;
    }
    if !saw_header {
        return Err("empty input / missing `sankey-beta` header".to_string());
    }

    let mut nodes: Vec<String> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut links: Vec<Link> = Vec::new();

    let mut find_or_create = |name: &str, nodes: &mut Vec<String>| -> usize {
        if let Some(&i) = index.get(name) {
            i
        } else {
            let i = nodes.len();
            nodes.push(name.to_string());
            index.insert(name.to_string(), i);
            i
        }
    };

    let mut handle_row = |row: &str,
                          nodes: &mut Vec<String>,
                          links: &mut Vec<Link>|
     -> Result<(), String> {
        let fields = parse_csv_row(row)?;
        if fields.len() < 3 {
            return Err(format!("row needs 3 fields `source,target,value`: {row:?}"));
        }
        let source = fields[0].trim().to_string();
        let target = fields[1].trim().to_string();
        let value: f64 = fields[2]
            .trim()
            .parse()
            .map_err(|_| format!("invalid numeric value in row {row:?}: {:?}", fields[2]))?;
        let s = find_or_create(&source, nodes);
        let t = find_or_create(&target, nodes);
        links.push(Link {
            source: s,
            target: t,
            value,
        });
        Ok(())
    };

    if let Some(data) = header_consumed_data {
        handle_row(&data, &mut nodes, &mut links)?;
    }

    for raw in lines {
        let line = strip_comment(raw);
        if line.trim().is_empty() {
            continue;
        }
        handle_row(line.trim(), &mut nodes, &mut links)?;
    }

    Ok(Sankey { nodes, links })
}

/// Strip a `%%` line comment (everything from the first `%%` to EOL).
fn strip_comment(raw: &str) -> &str {
    match raw.find("%%") {
        Some(i) => &raw[..i],
        None => raw,
    }
}

/// Split a CSV-ish row into fields. Bare fields end at a comma; a field starting
/// with `"` is quoted, where `""` is a literal quote and the closing `"` ends
/// the field. Per the mermaid grammar we only need the first three fields, but
/// we return all of them.
fn parse_csv_row(row: &str) -> Result<Vec<String>, String> {
    let mut fields: Vec<String> = Vec::new();
    let chars: Vec<char> = row.chars().collect();
    let mut i = 0;
    let n = chars.len();
    loop {
        // Skip leading whitespace before a field (mermaid trims fields anyway).
        let mut field = String::new();
        if i < n && chars[i] == '"' {
            // Quoted field.
            i += 1;
            loop {
                if i >= n {
                    return Err(format!("unterminated quoted field in row {row:?}"));
                }
                if chars[i] == '"' {
                    if i + 1 < n && chars[i + 1] == '"' {
                        field.push('"');
                        i += 2;
                    } else {
                        i += 1; // closing quote
                        break;
                    }
                } else {
                    field.push(chars[i]);
                    i += 1;
                }
            }
            // Consume up to the next comma (allow trailing spaces).
            while i < n && chars[i] != ',' {
                i += 1;
            }
        } else {
            // Bare field: read until comma.
            while i < n && chars[i] != ',' {
                field.push(chars[i]);
                i += 1;
            }
        }
        fields.push(field);
        if i < n && chars[i] == ',' {
            i += 1;
            continue;
        }
        break;
    }
    Ok(fields)
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// A laid-out node bar.
#[derive(Clone, Debug)]
struct NodeBox {
    column: usize,
    x: f32,
    y: f32,
    height: f32,
    /// Running offset (px) of inflow ribbons consumed along the left edge.
    in_offset: f32,
    /// Running offset (px) of outflow ribbons consumed along the right edge.
    out_offset: f32,
}

/// Assign each node a column via longest path from a source. Returns one column
/// index per node. Cycles are broken by a visited set so this always terminates.
fn assign_columns(s: &Sankey) -> Vec<usize> {
    let n = s.nodes.len();
    let mut incoming = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for l in &s.links {
        if l.source == l.target {
            continue; // ignore self-loops for layering
        }
        incoming[l.target] += 1;
        adj[l.source].push(l.target);
    }
    let mut column = vec![0usize; n];
    // Longest-path: relax repeatedly. With a visited guard per traversal we
    // avoid infinite loops on (invalid) cyclic input.
    // Simple Bellman-Ford-style relaxation bounded by n iterations.
    for _ in 0..n.max(1) {
        let mut changed = false;
        for l in &s.links {
            if l.source == l.target {
                continue;
            }
            let want = column[l.source] + 1;
            if want > column[l.target] {
                column[l.target] = want;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let _ = incoming;
    column
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// A small deterministic categorical palette (mermaid-ish), indexed per node.
const PALETTE: [[u8; 4]; 10] = [
    [102, 153, 204, 255], // blue
    [255, 153, 102, 255], // orange
    [153, 204, 102, 255], // green
    [204, 102, 153, 255], // pink
    [153, 102, 204, 255], // purple
    [102, 204, 204, 255], // teal
    [204, 204, 102, 255], // yellow-green
    [204, 153, 102, 255], // brown
    [153, 153, 153, 255], // grey
    [102, 102, 204, 255], // indigo
];

/// The palette color (RGBA) for node index `i` (cycling). Prefers the active
/// theme's `series_palette` when set, falling back to the local [`PALETTE`].
fn palette_color(opts: &MermaidOptions, i: usize) -> [u8; 4] {
    if !opts.series_palette.is_empty() {
        opts.series_palette[i % opts.series_palette.len()]
    } else {
        PALETTE[i % PALETTE.len()]
    }
}

/// Render a mermaid sankey diagram to SVG.
pub fn render_sankey(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let sankey = parse_sankey(src).map_err(MermaidError::Parse)?;
    if sankey.links.is_empty() || sankey.nodes.is_empty() {
        return Err(MermaidError::Empty);
    }

    let n = sankey.nodes.len();
    let columns = assign_columns(&sankey);
    let num_columns = columns.iter().copied().max().unwrap_or(0) + 1;

    // Per-node flow weight = max(sum inflow, sum outflow).
    let mut inflow = vec![0f64; n];
    let mut outflow = vec![0f64; n];
    for l in &sankey.links {
        outflow[l.source] += l.value.max(0.0);
        inflow[l.target] += l.value.max(0.0);
    }
    let weight: Vec<f64> = (0..n)
        .map(|i| inflow[i].max(outflow[i]).max(0.0))
        .collect();

    // Group nodes by column (first-seen order within a column).
    let mut by_column: Vec<Vec<usize>> = vec![Vec::new(); num_columns];
    for i in 0..n {
        by_column[columns[i]].push(i);
    }

    // --- Scale: busiest column's (weights + gaps) maps to plot height. ---
    // Inter-node vertical gap: keep stacked nodes in a column clearly separated.
    let node_gap = (opts.node_sep * 0.5).max(14.0);
    // A taller plot so bars/ribbons read as bands, not slivers.
    let plot_height = 560.0_f32;
    let mut max_col_weight = 0f64;
    for col in &by_column {
        let w: f64 = col.iter().map(|&i| weight[i]).sum();
        max_col_weight = max_col_weight.max(w);
    }
    // px per flow unit. Reserve gap space in the tallest column.
    let tallest_col_len = by_column
        .iter()
        .map(|c| c.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let gaps_height = node_gap * (tallest_col_len.saturating_sub(1)) as f32;
    let scale: f32 = if max_col_weight > 0.0 {
        ((plot_height - gaps_height).max(40.0) as f64 / max_col_weight) as f32
    } else {
        1.0
    };
    // Minimum node-bar height so zero/tiny nodes are still clearly visible.
    let min_node_h = 8.0_f32;

    // --- Geometry constants. ---
    // Wider bars read better against the wider column gaps below.
    let bar_width = 22.0_f32;
    // Horizontal gap between columns. A generous run (several× the bar width)
    // gives the ribbons room to curve so they aren't steep/cramped.
    let col_gap = opts.rank_sep.max(50.0) * 2.4; // -> >= 120px between columns
    let margin = 20.0_f32;
    let font = opts.font_size_px;
    // Estimate label widths to size left/right margins.
    let mut max_label_w = 0f32;
    for name in &sankey.nodes {
        let (w, _) = text_size(name, font);
        max_label_w = max_label_w.max(w);
    }
    let label_pad = 10.0_f32;
    // Left margin holds labels for column-0 source nodes (drawn left of bar).
    let left_label_space = max_label_w + label_pad;
    // Right margin holds labels for the last column (drawn right of bar).
    let right_label_space = max_label_w + label_pad;

    let col_step = bar_width + col_gap;
    let plot_left = margin + left_label_space;

    // Place nodes per column, stacked vertically.
    let mut boxes: Vec<NodeBox> = (0..n)
        .map(|_| NodeBox {
            column: 0,
            x: 0.0,
            y: 0.0,
            height: 0.0,
            in_offset: 0.0,
            out_offset: 0.0,
        })
        .collect();

    let mut plot_bottom = margin + plot_height;
    for (c, col) in by_column.iter().enumerate() {
        let x = plot_left + c as f32 * col_step;
        let mut y = margin;
        for &i in col {
            let h = (weight[i] as f32 * scale).max(min_node_h);
            boxes[i] = NodeBox {
                column: c,
                x,
                y,
                height: h,
                in_offset: 0.0,
                out_offset: 0.0,
            };
            y += h + node_gap;
        }
        plot_bottom = plot_bottom.max(y - node_gap);
    }

    let width = plot_left
        + (num_columns.saturating_sub(1)) as f32 * col_step
        + bar_width
        + right_label_space
        + margin;
    let height = plot_bottom + margin;

    // --- Emit SVG. ---
    let mut svg = String::with_capacity(1024 + sankey.links.len() * 256);
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.2}\" height=\"{height:.2}\" \
         viewBox=\"0 0 {width:.2} {height:.2}\">",
    );

    // Ribbons first (under the bars), in link order for determinism.
    for l in &sankey.links {
        let sb_x;
        let sb;
        let tb;
        {
            sb = (boxes[l.source].x, boxes[l.source].y, boxes[l.source].height);
            tb = (boxes[l.target].x, boxes[l.target].y, boxes[l.target].height);
            sb_x = sb.0 + bar_width;
        }
        let thickness = (l.value.max(0.0) as f32 * scale).max(0.5);

        // Source side: right edge of source bar, stacking via out_offset.
        let sy0 = boxes[l.source].y + boxes[l.source].out_offset;
        let sy1 = sy0 + thickness;
        boxes[l.source].out_offset += thickness;

        // Target side: left edge of target bar, stacking via in_offset.
        let ty0 = boxes[l.target].y + boxes[l.target].in_offset;
        let ty1 = ty0 + thickness;
        boxes[l.target].in_offset += thickness;

        let x0 = sb_x;
        let x1 = tb.0;
        // Pull each control point partway toward the midpoint so the ribbon
        // leaves/enters each bar nearly horizontally, then sweeps across the
        // (now generous) horizontal run. cx0 sits ~60% toward the middle from
        // the source, cx1 ~60% from the target — flatter, more legible curves.
        let cx0 = x0 + (x1 - x0) * 0.55;
        let cx1 = x1 - (x1 - x0) * 0.55;

        let color = palette_color(opts, l.source);
        let mut fill_alpha = color;
        fill_alpha[3] = 102; // ~0.4

        // Band: top edge (x0,sy0)→cubic→(x1,ty0); right edge down to (x1,ty1);
        // bottom edge cubic back to (x0,sy1); close.
        let _ = write!(
            svg,
            "<path d=\"M{x0:.2},{sy0:.2} C{cx0:.2},{sy0:.2} {cx1:.2},{ty0:.2} {x1:.2},{ty0:.2} \
             L{x1:.2},{ty1:.2} C{cx1:.2},{ty1:.2} {cx0:.2},{sy1:.2} {x0:.2},{sy1:.2} Z\" \
             fill=\"{}\"{}/>",
            rgb(fill_alpha),
            opacity_attr("fill-opacity", fill_alpha),
        );
    }

    // Node bars + labels on top.
    for i in 0..n {
        let b = &boxes[i];
        let color = palette_color(opts, i);
        let _ = write!(
            svg,
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"{}\"{} \
             stroke=\"{}\"{}/>",
            b.x,
            b.y,
            bar_width,
            b.height,
            rgb(color),
            opacity_attr("fill-opacity", color),
            rgb(opts.node_stroke),
            opacity_attr("stroke-opacity", opts.node_stroke),
        );

        // Label: sources (first column) to the left, everything else to the
        // right of the bar. Vertically centered on the bar.
        let cy = b.y + b.height * 0.5 + font * 0.32;
        let (tx, anchor) = if b.column == 0 {
            (b.x - label_pad, "end")
        } else {
            (b.x + bar_width + label_pad, "start")
        };
        let _ = write!(
            svg,
            "<text x=\"{:.2}\" y=\"{:.2}\" font-family=\"{}\" font-size=\"{:.2}\" \
             text-anchor=\"{}\" fill=\"{}\"{}>{}</text>",
            tx,
            cy,
            escape(&opts.font_family),
            font,
            anchor,
            rgb(opts.text_color),
            opacity_attr("fill-opacity", opts.text_color),
            escape(&sankey.nodes[i]),
        );
    }

    svg.push_str("</svg>");

    Ok(MermaidRender {
        svg,
        width_px: width,
        height_px: height,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    #[test]
    fn parses_basic_rows() {
        let src = "sankey-beta\na,b,5\nb,c,3\n";
        let s = parse_sankey(src).unwrap();
        assert_eq!(s.nodes, vec!["a", "b", "c"]); // first-seen order
        assert_eq!(s.links.len(), 2);
        assert_eq!(s.links[0].source, 0);
        assert_eq!(s.links[0].target, 1);
        assert_eq!(s.links[0].value, 5.0);
        assert_eq!(s.links[1].source, 1);
        assert_eq!(s.links[1].value, 3.0);
    }

    #[test]
    fn parses_quoted_node_with_escaped_quote() {
        // "a,b" is one node containing a comma; "x""y" contains a literal quote.
        let src = "sankey-beta\n\"a,b\",\"x\"\"y\",2.5\n";
        let s = parse_sankey(src).unwrap();
        assert_eq!(s.nodes, vec!["a,b", "x\"y"]);
        assert_eq!(s.links.len(), 1);
        assert_eq!(s.links[0].value, 2.5);
    }

    #[test]
    fn accepts_bare_sankey_header() {
        let src = "sankey\nA,B,1\n";
        let s = parse_sankey(src).unwrap();
        assert_eq!(s.nodes, vec!["A", "B"]);
        assert_eq!(s.links.len(), 1);
    }

    #[test]
    fn header_is_case_insensitive() {
        let src = "SANKEY-BETA\nA,B,1\n";
        assert!(parse_sankey(src).is_ok());
    }

    #[test]
    fn ignores_blank_lines_and_comments() {
        let src = "sankey-beta\n\n%% a comment\nA,B,1\n   \nB,C,2 %% trailing\n";
        let s = parse_sankey(src).unwrap();
        assert_eq!(s.nodes, vec!["A", "B", "C"]);
        assert_eq!(s.links.len(), 2);
        assert_eq!(s.links[1].value, 2.0);
    }

    #[test]
    fn bad_header_errors() {
        assert!(matches!(
            render_sankey("graph TD\nA-->B\n", &opts()),
            Err(MermaidError::Parse(_))
        ));
    }

    #[test]
    fn empty_errors() {
        // Header present but no links → Empty.
        assert_eq!(render_sankey("sankey-beta\n", &opts()), Err(MermaidError::Empty));
    }

    #[test]
    fn invalid_value_errors() {
        assert!(matches!(
            parse_sankey("sankey-beta\nA,B,notanumber\n"),
            Err(_)
        ));
    }

    #[test]
    fn column_assignment_chain() {
        // a -> b -> c should be columns 0, 1, 2.
        let s = parse_sankey("sankey-beta\na,b,1\nb,c,1\n").unwrap();
        let cols = assign_columns(&s);
        assert_eq!(cols[0], 0); // a
        assert_eq!(cols[1], 1); // b
        assert_eq!(cols[2], 2); // c
    }

    #[test]
    fn column_assignment_longest_path() {
        // a->c, a->b, b->c : c must be column 2 (longest path a->b->c), not 1.
        // First-seen index order: a=0, c=1, b=2.
        let s = parse_sankey("sankey-beta\na,c,1\na,b,1\nb,c,1\n").unwrap();
        assert_eq!(s.nodes, vec!["a", "c", "b"]);
        let cols = assign_columns(&s);
        assert_eq!(cols[0], 0); // a
        assert_eq!(cols[2], 1); // b
        assert_eq!(cols[1], 2); // c (longest path wins over the direct a->c)
    }

    #[test]
    fn render_is_well_formed() {
        let src = "sankey-beta\na,b,5\nb,c,3\na,c,2\n";
        let r = render_sankey(src, &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
        // One <rect> per node (3 nodes).
        assert_eq!(r.svg.matches("<rect").count(), 3);
        // One ribbon <path> per link (3 links).
        assert_eq!(r.svg.matches("<path").count(), 3);
        // Node labels present.
        assert!(r.svg.contains(">a</text>"));
        assert!(r.svg.contains(">b</text>"));
        assert!(r.svg.contains(">c</text>"));
        // Translucent ribbons.
        assert!(r.svg.contains("fill-opacity"));
    }

    #[test]
    fn render_xml_escapes_labels() {
        let src = "sankey-beta\n\"a & <b>\",c,1\n";
        let r = render_sankey(src, &opts()).unwrap();
        assert!(r.svg.contains("a &amp; &lt;b&gt;"));
        assert!(!r.svg.contains("<b>"));
    }

    #[test]
    fn render_is_deterministic() {
        let src = "sankey-beta\na,b,5\nb,c,3\na,c,2\n";
        let r1 = render_sankey(src, &opts()).unwrap();
        let r2 = render_sankey(src, &opts()).unwrap();
        assert_eq!(r1.svg, r2.svg);
        assert_eq!(r1.width_px, r2.width_px);
        assert_eq!(r1.height_px, r2.height_px);
    }

    #[test]
    fn ribbon_count_matches_links_on_larger_graph() {
        let src = "sankey-beta\n\
            Agricultural 'waste',Bio-conversion,124.729\n\
            Bio-conversion,Liquid,0.597\n\
            Bio-conversion,Losses,26.862\n\
            Bio-conversion,Solid,280.322\n\
            Bio-conversion,Gas,81.144\n";
        let s = parse_sankey(src).unwrap();
        assert_eq!(s.nodes.len(), 6);
        assert_eq!(s.links.len(), 5);
        let r = render_sankey(src, &opts()).unwrap();
        assert_eq!(r.svg.matches("<path").count(), 5);
        assert_eq!(r.svg.matches("<rect").count(), 6);
    }
}
