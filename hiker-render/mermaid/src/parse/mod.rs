//! Stage 1: parse mermaid flowchart source → [`FlowChart`]. No upstream deps.
//!
//! A pragmatic, hand-written (line-and-token, recursive-descent flavored)
//! parser for a well-defined SUBSET of the mermaid flowchart grammar. See
//! `references/mermaid`'s `packages/mermaid/src/diagrams/flowchart/parser/flow.jison`
//! for the full grammar. Pure std, dependency-free.
//!
//! ## Supported subset
//! - Header: `graph <dir>` / `flowchart <dir>` with `<dir>` ∈ `TB|TD|BT|LR|RL`.
//!   `TD`/`TB` → [`Direction::TopDown`]. Default [`Direction::TopDown`] if absent.
//! - Statement separators: newlines and `;`. Blank lines ignored. `%% ...`
//!   comments stripped to end of line.
//! - Node shapes: `A` / `A[..]` (Rect), `A(..)` (RoundRect), `A([..])` (Stadium),
//!   `A((..))` (Circle), `A{..}` (Diamond), `A{{..}}` (Hexagon), plus the
//!   extended brackets `A[(..)]` (Cylinder), `A[[..]]` (Subroutine),
//!   `A[/../]`/`A[\..\]` (Parallelogram), `A[/..\]`/`A[\../]` (Trapezoid),
//!   `A(((..)))` (DoubleCircle), and the mermaid-11 `A@{ shape: .., label: .. }`
//!   form. Labels may be `"quoted"`.
//! - Edges: `-->` `---` `<-->`, thick `==>`/`===`, dotted `-.->`/`-.-`, variable
//!   dash lengths, labels via `-->|text|` and `-- text -->` (and `--- text ---`),
//!   and chaining `A --> B --> C`.
//!
//! Intentionally skipped for v1 (see report): `&` multi-node refs, accessibility
//! directives, and the `o--o`/`x--x` endpoint markers.
//!
//! Policy: **lenient** — unrecognized lines are skipped rather than erroring, so
//! recovery is per-line. Node label/shape is **last-wins** when a node is
//! redefined. A node referenced before being shaped defaults to `Rect` with
//! `label == id`.

use crate::model::{Direction, FlowChart, FlowEdge, FlowNode, Subgraph};

mod directives;
mod shapes;

use directives::{parse_directive, resolve_styles, Directives};
use shapes::{clean_label, parse_edge_op, parse_node_ref, skip_ws, ParsedNode};

/// Parse mermaid flowchart source (e.g. `graph TD; A[Start] --> B{Decision}`)
/// into a [`FlowChart`]. Returns `Err(message)` on a syntax error.
pub fn parse_flowchart(src: &str) -> Result<FlowChart, String> {
    let mut chart = FlowChart {
        direction: Direction::TopDown,
        nodes: Vec::new(),
        edges: Vec::new(),
        subgraphs: Vec::new(),
    };
    // Tracks insertion index of each node id so we can update (last-wins) the
    // existing entry rather than appending a duplicate.
    let mut node_index: Vec<(String, usize)> = Vec::new();
    let mut directives = Directives::default();
    // Stack of currently-open subgraph indices (into `chart.subgraphs`), innermost
    // last. A node first seen while this is non-empty joins the innermost subgraph.
    let mut subgraph_stack: Vec<usize> = Vec::new();

    let mut header_seen = false;

    for raw_line in src.lines() {
        let line = strip_comment(raw_line);
        // A physical line may carry multiple `;`-separated statements.
        for stmt in line.split(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }

            // Header line: `graph TD`, `flowchart LR`, etc. Only the first
            // keyword line counts as a header / direction.
            if !header_seen {
                if let Some(dir) = parse_header(stmt) {
                    chart.direction = dir;
                    header_seen = true;
                    continue;
                }
                // First real statement without a header keyword: treat the
                // (absent) header as default TopDown and fall through to parse
                // it as a statement.
                header_seen = true;
            }

            // Subgraph block control: `subgraph …` opens a cluster (push), `end`
            // closes the innermost one (pop). These bracket the statements between
            // them; nodes first seen inside join the innermost open subgraph.
            if let Some((id, title)) = parse_subgraph_open(stmt) {
                let parent = subgraph_stack.last().copied();
                let idx = chart.subgraphs.len();
                chart.subgraphs.push(Subgraph {
                    id,
                    title,
                    node_ids: Vec::new(),
                    parent,
                });
                subgraph_stack.push(idx);
                continue;
            }
            if stmt == "end" {
                subgraph_stack.pop();
                continue;
            }
            // `direction LR` inside a subgraph is parsed-and-ignored (the whole
            // chart keeps its single top-level direction in this renderer).
            if stmt.split_whitespace().next() == Some("direction") {
                continue;
            }

            // Styling directives (`classDef`/`class`/`style`/`linkStyle`) are
            // collected here and resolved after all statements are parsed.
            if parse_directive(stmt, &mut directives) {
                continue;
            }

            parse_statement(
                stmt,
                &mut chart,
                &mut node_index,
                &mut directives,
                &subgraph_stack,
            );
        }
    }

    resolve_styles(&mut chart, &directives);

    Ok(chart)
}

fn strip_comment(line: &str) -> &str {
    match line.find("%%") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// If `stmt` is a header keyword line (`graph`/`flowchart [dir]`), return the
/// direction (defaulting to `TopDown` when no/unknown dir token follows).
fn parse_header(stmt: &str) -> Option<Direction> {
    let mut words = stmt.split_whitespace();
    let kw = words.next()?;
    if kw != "graph" && kw != "flowchart" {
        return None;
    }
    let dir = match words.next() {
        Some(tok) => parse_direction(tok).unwrap_or(Direction::TopDown),
        None => Direction::TopDown,
    };
    Some(dir)
}

/// If `stmt` opens a subgraph block, return its `(id, title)`. Forms:
/// - `subgraph <id>[<Title>]` — explicit id + bracketed title.
/// - `subgraph <id>` — id only; title = id.
/// - `subgraph "Title"` / `subgraph <Title>` — no brackets; the token(s) form
///   both the title and (for a single bare word) the id.
fn parse_subgraph_open(stmt: &str) -> Option<(String, String)> {
    let rest = stmt.strip_prefix("subgraph")?;
    // Must be a word boundary: `subgraph` followed by whitespace or end-of-line.
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim();
    if rest.is_empty() {
        // Anonymous subgraph: id/title derived from declaration order by caller's
        // index; use an empty title so no label is drawn.
        return Some((String::new(), String::new()));
    }

    // `<id>[<Title>]` — id is the leading id-chars before a `[`.
    if let Some(br) = rest.find('[') {
        let id = rest[..br].trim();
        if rest.ends_with(']') {
            let title = clean_label(rest[br + 1..rest.len() - 1].trim());
            return Some((id.to_string(), title));
        }
    }

    // `"Title"` — quoted title; id derived as the title text (no separate id).
    if rest.starts_with('"') {
        let title = clean_label(rest);
        return Some((title.clone(), title));
    }

    // Bare `<id>` (single token) → title = id. Multi-word bare title → id = whole
    // text, title = whole text.
    Some((rest.to_string(), rest.to_string()))
}

/// Map a direction token to a [`Direction`].
fn parse_direction(tok: &str) -> Option<Direction> {
    match tok {
        "TB" | "TD" => Some(Direction::TopDown),
        "BT" => Some(Direction::BottomUp),
        "LR" => Some(Direction::LeftRight),
        "RL" => Some(Direction::RightLeft),
        _ => None,
    }
}

/// Parse one statement (a single `;`/newline-delimited unit). Handles both
/// standalone node declarations and edge chains. Lenient: bails silently on
/// anything it can't make sense of.
fn parse_statement(
    stmt: &str,
    chart: &mut FlowChart,
    node_index: &mut Vec<(String, usize)>,
    dir: &mut Directives,
    subgraph_stack: &[usize],
) {
    let bytes = stmt.as_bytes();
    let mut pos = 0usize;

    // First node ref is required for any statement we care about.
    let first = match parse_node_ref(bytes, &mut pos, dir) {
        Some(n) => n,
        None => return,
    };
    upsert_node(chart, node_index, first.clone(), subgraph_stack);

    let mut prev_id = first.id;

    // Then zero-or-more (edge, node) pairs, supporting chaining A --> B --> C.
    loop {
        skip_ws(bytes, &mut pos);
        if pos >= bytes.len() {
            break;
        }
        let edge = match parse_edge_op(bytes, &mut pos) {
            Some(e) => e,
            None => break, // not an edge here; stop (lenient)
        };
        skip_ws(bytes, &mut pos);
        let target = match parse_node_ref(bytes, &mut pos, dir) {
            Some(n) => n,
            None => break, // edge with no target; drop it (lenient)
        };
        let target_id = target.id.clone();
        upsert_node(chart, node_index, target, subgraph_stack);

        chart.edges.push(FlowEdge {
            from: prev_id.clone(),
            to: target_id.clone(),
            label: edge.label,
            kind: edge.kind,
            arrow_start: edge.arrow_start,
            arrow_end: edge.arrow_end, style: crate::model::ElemStyle::default(),
        });
        prev_id = target_id;
    }
}

/// Insert or update (last-wins for shape/label) a node into the chart, keeping
/// first-seen ordering.
fn upsert_node(
    chart: &mut FlowChart,
    node_index: &mut Vec<(String, usize)>,
    parsed: ParsedNode,
    subgraph_stack: &[usize],
) {
    let existing = node_index.iter().find(|(id, _)| *id == parsed.id).map(|(_, i)| *i);
    let is_new = existing.is_none();
    match existing {
        Some(i) => {
            // Only override shape/label when this ref actually carried one.
            if let Some(label) = parsed.label {
                chart.nodes[i].label = label;
                chart.nodes[i].shape = parsed.shape;
            }
        }
        None => {
            let idx = chart.nodes.len();
            let label = parsed.label.unwrap_or_else(|| parsed.id.clone());
            chart.nodes.push(FlowNode {
                id: parsed.id.clone(),
                label,
                shape: parsed.shape,
                style: crate::model::ElemStyle::default(),
                link: None,
                callback: None,
                tooltip: None,
            });
            node_index.push((parsed.id.clone(), idx));
        }
    }

    // A node first seen inside a subgraph block belongs to the innermost open
    // subgraph. Membership is keyed on first-seen so a node declared in one
    // subgraph but referenced from another stays in its original subgraph.
    if is_new {
        if let Some(&sg) = subgraph_stack.last() {
            chart.subgraphs[sg].node_ids.push(parsed.id);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::directives::{parse_color, parse_width};
    use super::*;
    use crate::model::{EdgeKind, ElemStyle, NodeShape};

    fn parse(src: &str) -> FlowChart {
        parse_flowchart(src).expect("parse ok")
    }

    fn node<'a>(c: &'a FlowChart, id: &str) -> &'a FlowNode {
        c.nodes.iter().find(|n| n.id == id).expect("node present")
    }

    // ── Direction ──────────────────────────────────────────────────────────

    #[test]
    fn direction_td_and_tb_are_topdown() {
        assert_eq!(parse("graph TD\nA-->B").direction, Direction::TopDown);
        assert_eq!(parse("graph TB\nA-->B").direction, Direction::TopDown);
        assert_eq!(parse("flowchart TD\nA-->B").direction, Direction::TopDown);
    }

    #[test]
    fn direction_bt_lr_rl() {
        assert_eq!(parse("graph BT\nA-->B").direction, Direction::BottomUp);
        assert_eq!(parse("graph LR\nA-->B").direction, Direction::LeftRight);
        assert_eq!(parse("flowchart RL\nA-->B").direction, Direction::RightLeft);
    }

    #[test]
    fn direction_defaults_to_topdown_without_header() {
        let c = parse("A --> B");
        assert_eq!(c.direction, Direction::TopDown);
        assert_eq!(c.nodes.len(), 2);
        assert_eq!(c.edges.len(), 1);
    }

    #[test]
    fn header_without_dir_is_topdown() {
        assert_eq!(parse("graph\nA-->B").direction, Direction::TopDown);
    }

    // ── Node shapes ────────────────────────────────────────────────────────

    #[test]
    fn all_node_shapes() {
        let c = parse(
            "graph TD\n\
             A[rect]\n\
             B(round)\n\
             C([stad])\n\
             D((circ))\n\
             E{diam}\n\
             F{{hex}}",
        );
        assert_eq!(node(&c, "A").shape, NodeShape::Rect);
        assert_eq!(node(&c, "A").label, "rect");
        assert_eq!(node(&c, "B").shape, NodeShape::RoundRect);
        assert_eq!(node(&c, "B").label, "round");
        assert_eq!(node(&c, "C").shape, NodeShape::Stadium);
        assert_eq!(node(&c, "C").label, "stad");
        assert_eq!(node(&c, "D").shape, NodeShape::Circle);
        assert_eq!(node(&c, "D").label, "circ");
        assert_eq!(node(&c, "E").shape, NodeShape::Diamond);
        assert_eq!(node(&c, "E").label, "diam");
        assert_eq!(node(&c, "F").shape, NodeShape::Hexagon);
        assert_eq!(node(&c, "F").label, "hex");
    }

    #[test]
    fn extended_bracket_shapes() {
        let c = parse(
            "graph TD\n\
             A[(DB)]\n\
             B[[Sub]]\n\
             C[/Para/]\n\
             D[\\ParaAlt\\]\n\
             E[/Trap\\]\n\
             F[\\TrapAlt/]\n\
             G(((Dbl)))",
        );
        assert_eq!(node(&c, "A").shape, NodeShape::Cylinder);
        assert_eq!(node(&c, "A").label, "DB");
        assert_eq!(node(&c, "B").shape, NodeShape::Subroutine);
        assert_eq!(node(&c, "B").label, "Sub");
        assert_eq!(node(&c, "C").shape, NodeShape::Parallelogram);
        assert_eq!(node(&c, "C").label, "Para");
        assert_eq!(node(&c, "D").shape, NodeShape::ParallelogramAlt);
        assert_eq!(node(&c, "D").label, "ParaAlt");
        assert_eq!(node(&c, "E").shape, NodeShape::Trapezoid);
        assert_eq!(node(&c, "E").label, "Trap");
        assert_eq!(node(&c, "F").shape, NodeShape::TrapezoidAlt);
        assert_eq!(node(&c, "F").label, "TrapAlt");
        assert_eq!(node(&c, "G").shape, NodeShape::DoubleCircle);
        assert_eq!(node(&c, "G").label, "Dbl");
    }

    #[test]
    fn at_shape_syntax_sets_shape_and_label() {
        let c = parse("graph TD\nX@{ shape: cylinder, label: \"DB\" }");
        assert_eq!(node(&c, "X").shape, NodeShape::Cylinder);
        assert_eq!(node(&c, "X").label, "DB");
    }

    #[test]
    fn at_shape_aliases_and_unknown() {
        let c = parse(
            "graph TD\n\
             A@{ shape: doc }\n\
             B@{ shape: lean-l }\n\
             C@{ shape: trap-t }\n\
             D@{ shape: dbl-circ }\n\
             E@{ shape: nope }",
        );
        assert_eq!(node(&c, "A").shape, NodeShape::Document);
        assert_eq!(node(&c, "B").shape, NodeShape::ParallelogramAlt);
        assert_eq!(node(&c, "C").shape, NodeShape::TrapezoidAlt);
        assert_eq!(node(&c, "D").shape, NodeShape::DoubleCircle);
        // Unknown shape name falls back to Rect.
        assert_eq!(node(&c, "E").shape, NodeShape::Rect);
        // No label given → label is the id.
        assert_eq!(node(&c, "A").label, "A");
    }

    #[test]
    fn at_shape_label_overrides_bracket() {
        // `@{ label: }` wins over a preceding bracket label.
        let c = parse("graph TD\nA[old]@{ shape: stadium, label: new }");
        assert_eq!(node(&c, "A").shape, NodeShape::Stadium);
        assert_eq!(node(&c, "A").label, "new");
    }

    #[test]
    fn at_shape_node_participates_in_edges() {
        let c = parse("graph LR\nA@{ shape: cylinder } --> B");
        assert_eq!(node(&c, "A").shape, NodeShape::Cylinder);
        assert_eq!(c.edges.len(), 1);
        assert_eq!((c.edges[0].from.as_str(), c.edges[0].to.as_str()), ("A", "B"));
    }

    #[test]
    fn bare_node_defaults_rect_label_is_id() {
        let c = parse("graph TD\nHello");
        assert_eq!(c.nodes.len(), 1);
        assert_eq!(node(&c, "Hello").shape, NodeShape::Rect);
        assert_eq!(node(&c, "Hello").label, "Hello");
    }

    #[test]
    fn default_rect_node_from_bare_edge_endpoint() {
        let c = parse("graph TD\nA --> B");
        assert_eq!(node(&c, "A").shape, NodeShape::Rect);
        assert_eq!(node(&c, "A").label, "A");
        assert_eq!(node(&c, "B").label, "B");
    }

    #[test]
    fn quoted_labels() {
        let c = parse("graph TD\nA[\"hello world\"] --> B{\"is it?\"}");
        assert_eq!(node(&c, "A").label, "hello world");
        assert_eq!(node(&c, "B").label, "is it?");
        assert_eq!(node(&c, "B").shape, NodeShape::Diamond);
    }

    // ── Edge kinds / arrowheads ────────────────────────────────────────────

    #[test]
    fn edge_arrow_end_only() {
        let c = parse("A --> B");
        let e = &c.edges[0];
        assert_eq!(e.kind, EdgeKind::Normal);
        assert!(e.arrow_end);
        assert!(!e.arrow_start);
    }

    #[test]
    fn edge_no_arrowheads() {
        let c = parse("A --- B");
        let e = &c.edges[0];
        assert!(!e.arrow_end);
        assert!(!e.arrow_start);
        assert_eq!(e.kind, EdgeKind::Normal);
    }

    #[test]
    fn edge_bidirectional() {
        let c = parse("A <--> B");
        let e = &c.edges[0];
        assert!(e.arrow_start);
        assert!(e.arrow_end);
    }

    #[test]
    fn edge_thick() {
        let c = parse("A ==> B");
        assert_eq!(c.edges[0].kind, EdgeKind::Thick);
        assert!(c.edges[0].arrow_end);
        let c2 = parse("A === B");
        assert_eq!(c2.edges[0].kind, EdgeKind::Thick);
        assert!(!c2.edges[0].arrow_end);
    }

    #[test]
    fn edge_dotted() {
        let c = parse("A -.-> B");
        assert_eq!(c.edges[0].kind, EdgeKind::Dotted);
        assert!(c.edges[0].arrow_end);
        let c2 = parse("A -.- B");
        assert_eq!(c2.edges[0].kind, EdgeKind::Dotted);
        assert!(!c2.edges[0].arrow_end);
    }

    #[test]
    fn edge_variable_dash_length() {
        let c = parse("A ---> B");
        assert_eq!(c.edges[0].kind, EdgeKind::Normal);
        assert!(c.edges[0].arrow_end);
        let c2 = parse("A ====> B");
        assert_eq!(c2.edges[0].kind, EdgeKind::Thick);
        assert!(c2.edges[0].arrow_end);
    }

    #[test]
    fn edge_label_pipe_form() {
        let c = parse("A -->|yes| B");
        assert_eq!(c.edges[0].label.as_deref(), Some("yes"));
        assert!(c.edges[0].arrow_end);
    }

    #[test]
    fn edge_label_inline_form_arrow() {
        let c = parse("A -- maybe --> B");
        assert_eq!(c.edges[0].label.as_deref(), Some("maybe"));
        assert!(c.edges[0].arrow_end);
        assert_eq!(c.edges[0].kind, EdgeKind::Normal);
    }

    #[test]
    fn edge_label_inline_form_no_arrow() {
        let c = parse("A --- link --- B");
        assert_eq!(c.edges[0].label.as_deref(), Some("link"));
        assert!(!c.edges[0].arrow_end);
    }

    #[test]
    fn edge_label_quoted() {
        let c = parse("A -->|\"a b\"| B");
        assert_eq!(c.edges[0].label.as_deref(), Some("a b"));
    }

    // ── Node ordering / dedup ──────────────────────────────────────────────

    #[test]
    fn first_seen_ordering_and_dedup() {
        let c = parse("graph TD\nB --> A\nA --> C\nB[bee]");
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["B", "A", "C"]);
        // B's label was set last by `B[bee]` (last-wins).
        assert_eq!(node(&c, "B").label, "bee");
    }

    #[test]
    fn last_wins_shape_and_label() {
        let c = parse("graph TD\nA[first] --> B\nA{second}");
        assert_eq!(node(&c, "A").label, "second");
        assert_eq!(node(&c, "A").shape, NodeShape::Diamond);
    }

    #[test]
    fn ref_before_def_defaults_then_updates() {
        // A referenced bare first, then shaped later.
        let c = parse("graph TD\nA --> B\nA([later])");
        assert_eq!(node(&c, "A").shape, NodeShape::Stadium);
        assert_eq!(node(&c, "A").label, "later");
    }

    // ── Chaining ───────────────────────────────────────────────────────────

    #[test]
    fn chaining_produces_sequential_edges() {
        let c = parse("graph TD\nA --> B --> C");
        assert_eq!(c.nodes.len(), 3);
        assert_eq!(c.edges.len(), 2);
        assert_eq!((c.edges[0].from.as_str(), c.edges[0].to.as_str()), ("A", "B"));
        assert_eq!((c.edges[1].from.as_str(), c.edges[1].to.as_str()), ("B", "C"));
    }

    // ── Separators / comments / blank lines ────────────────────────────────

    #[test]
    fn semicolon_separated_statements() {
        let c = parse("graph TD; A --> B; B --> C");
        assert_eq!(c.nodes.len(), 3);
        assert_eq!(c.edges.len(), 2);
    }

    #[test]
    fn comments_and_blank_lines_ignored() {
        let c = parse(
            "graph TD\n\
             %% this is a comment\n\
             \n\
             A --> B %% trailing comment\n\
             \n",
        );
        assert_eq!(c.nodes.len(), 2);
        assert_eq!(c.edges.len(), 1);
        assert!(c.edges[0].label.is_none());
    }

    #[test]
    fn unknown_line_is_skipped_not_errored() {
        let c = parse("graph TD\n!!!garbage###\nA --> B");
        assert_eq!(c.edges.len(), 1);
    }

    // ── Realistic multi-line flowchart ─────────────────────────────────────

    #[test]
    fn realistic_flowchart() {
        let c = parse(
            "graph TD\n\
             A[Start] --> B{OK?}\n\
             B -->|yes| C(Done)\n\
             B -->|no| A",
        );
        assert_eq!(c.direction, Direction::TopDown);

        // Nodes in first-seen order: A, B, C
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B", "C"]);
        assert_eq!(c.nodes.len(), 3);

        assert_eq!(node(&c, "A").shape, NodeShape::Rect);
        assert_eq!(node(&c, "A").label, "Start");
        assert_eq!(node(&c, "B").shape, NodeShape::Diamond);
        assert_eq!(node(&c, "B").label, "OK?");
        assert_eq!(node(&c, "C").shape, NodeShape::RoundRect);
        assert_eq!(node(&c, "C").label, "Done");

        // Edges: A->B (no label), B->C (yes), B->A (no)
        assert_eq!(c.edges.len(), 3);

        assert_eq!((c.edges[0].from.as_str(), c.edges[0].to.as_str()), ("A", "B"));
        assert!(c.edges[0].label.is_none());
        assert!(c.edges[0].arrow_end);

        assert_eq!((c.edges[1].from.as_str(), c.edges[1].to.as_str()), ("B", "C"));
        assert_eq!(c.edges[1].label.as_deref(), Some("yes"));

        assert_eq!((c.edges[2].from.as_str(), c.edges[2].to.as_str()), ("B", "A"));
        assert_eq!(c.edges[2].label.as_deref(), Some("no"));
    }

    #[test]
    fn empty_source_yields_empty_chart() {
        let c = parse("");
        assert!(c.nodes.is_empty());
        assert!(c.edges.is_empty());
        assert_eq!(c.direction, Direction::TopDown);
    }

    // ── Styling directives ─────────────────────────────────────────────────

    #[test]
    fn color_parser_forms() {
        assert_eq!(parse_color("#f00"), Some([255, 0, 0, 255]));
        assert_eq!(parse_color("#00ff00"), Some([0, 255, 0, 255]));
        assert_eq!(parse_color("#0000ff80"), Some([0, 0, 255, 128]));
        assert_eq!(parse_color("rgb(1,2,3)"), Some([1, 2, 3, 255]));
        assert_eq!(parse_color("rgba(1,2,3,0.5)"), Some([1, 2, 3, 128]));
        assert_eq!(parse_color("red"), Some([255, 0, 0, 255]));
        assert_eq!(parse_color("RED"), Some([255, 0, 0, 255]));
        assert_eq!(parse_color("notacolor"), None);
    }

    #[test]
    fn width_parser() {
        assert_eq!(parse_width("2px"), Some(2.0));
        assert_eq!(parse_width("4"), Some(4.0));
        assert_eq!(parse_width("1.5"), Some(1.5));
    }

    #[test]
    fn classdef_and_class_apply() {
        let c = parse(
            "graph TD\n\
             A --> B\n\
             classDef hot fill:#f00,stroke:#900,stroke-width:3px\n\
             class A hot",
        );
        let a = node(&c, "A");
        assert_eq!(a.style.fill, Some([255, 0, 0, 255]));
        assert!(a.style.stroke.is_some());
        assert_eq!(a.style.stroke_width, Some(3.0));
        // B untouched.
        assert_eq!(node(&c, "B").style, ElemStyle::default());
    }

    #[test]
    fn classdef_defined_after_class_still_resolves() {
        // Two-pass: `class` references `hot` before its classDef appears.
        let c = parse(
            "graph TD\n\
             class A hot\n\
             A --> B\n\
             classDef hot fill:#0f0",
        );
        assert_eq!(node(&c, "A").style.fill, Some([0, 255, 0, 255]));
    }

    #[test]
    fn class_shorthand_triple_colon() {
        let c = parse(
            "graph TD\n\
             A:::hot --> B\n\
             classDef hot fill:#f00",
        );
        assert_eq!(node(&c, "A").style.fill, Some([255, 0, 0, 255]));
        // A is still a normal node with an edge to B.
        assert_eq!(c.edges.len(), 1);
        assert_eq!((c.edges[0].from.as_str(), c.edges[0].to.as_str()), ("A", "B"));
    }

    #[test]
    fn class_shorthand_with_shape() {
        let c = parse(
            "graph TD\n\
             A[Start]:::hot --> B\n\
             classDef hot fill:#0000ff",
        );
        assert_eq!(node(&c, "A").label, "Start");
        assert_eq!(node(&c, "A").style.fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn style_directive_direct() {
        let c = parse("graph TD\nA --> B\nstyle B fill:#0f0");
        assert_eq!(node(&c, "B").style.fill, Some([0, 255, 0, 255]));
    }

    #[test]
    fn style_overrides_class() {
        // Inline `style` wins over `class` (applied on top).
        let c = parse(
            "graph TD\n\
             A --> B\n\
             classDef hot fill:#f00\n\
             class A hot\n\
             style A fill:#00f",
        );
        assert_eq!(node(&c, "A").style.fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn linkstyle_sets_edge() {
        let c = parse("graph TD\nA --> B\nlinkStyle 0 stroke:#00f");
        assert_eq!(c.edges[0].style.stroke, Some([0, 0, 255, 255]));
    }

    #[test]
    fn linkstyle_default_and_multi_index() {
        let c = parse(
            "graph TD\n\
             A --> B\n\
             B --> C\n\
             linkStyle default stroke:#000\n\
             linkStyle 0,1 stroke-width:4px",
        );
        assert_eq!(c.edges[0].style.stroke, Some([0, 0, 0, 255]));
        assert_eq!(c.edges[1].style.stroke, Some([0, 0, 0, 255]));
        assert_eq!(c.edges[0].style.stroke_width, Some(4.0));
        assert_eq!(c.edges[1].style.stroke_width, Some(4.0));
    }

    #[test]
    fn dasharray_sets_dashed() {
        let c = parse("graph TD\nA --> B\nstyle A stroke-dasharray:5 5");
        assert!(node(&c, "A").style.dashed);
    }

    #[test]
    fn unknown_color_prop_skipped() {
        let c = parse("graph TD\nA --> B\nstyle A fill:notacolor,stroke:#f00");
        assert_eq!(node(&c, "A").style.fill, None);
        assert_eq!(node(&c, "A").style.stroke, Some([255, 0, 0, 255]));
    }

    // ── Subgraphs ──────────────────────────────────────────────────────────

    #[test]
    fn subgraph_groups_members_titled() {
        let c = parse(
            "flowchart TD\n\
             subgraph one [Group One]\n\
             A --> B\n\
             end\n\
             B --> C",
        );
        assert_eq!(c.subgraphs.len(), 1);
        let sg = &c.subgraphs[0];
        assert_eq!(sg.id, "one");
        assert_eq!(sg.title, "Group One");
        assert_eq!(sg.node_ids, vec!["A", "B"]);
        assert!(sg.parent.is_none());
        // C is top-level (declared outside the subgraph block).
        assert!(!sg.node_ids.contains(&"C".to_string()));
        // Nodes & edges still parse normally.
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B", "C"]);
        assert_eq!(c.edges.len(), 2);
    }

    #[test]
    fn subgraph_bare_id_uses_id_as_title() {
        let c = parse("flowchart TD\nsubgraph svc\nA --> B\nend");
        assert_eq!(c.subgraphs.len(), 1);
        assert_eq!(c.subgraphs[0].id, "svc");
        assert_eq!(c.subgraphs[0].title, "svc");
        assert_eq!(c.subgraphs[0].node_ids, vec!["A", "B"]);
    }

    #[test]
    fn subgraph_quoted_title() {
        let c = parse("flowchart TD\nsubgraph \"My Title\"\nA --> B\nend");
        assert_eq!(c.subgraphs.len(), 1);
        assert_eq!(c.subgraphs[0].title, "My Title");
    }

    #[test]
    fn nested_subgraphs_set_parent() {
        let c = parse(
            "flowchart TD\n\
             subgraph outer [Outer]\n\
             A --> B\n\
             subgraph inner [Inner]\n\
             C --> D\n\
             end\n\
             end",
        );
        assert_eq!(c.subgraphs.len(), 2);
        let outer = &c.subgraphs[0];
        let inner = &c.subgraphs[1];
        assert_eq!(outer.title, "Outer");
        assert_eq!(outer.node_ids, vec!["A", "B"]);
        assert!(outer.parent.is_none());
        assert_eq!(inner.title, "Inner");
        assert_eq!(inner.node_ids, vec!["C", "D"]);
        // inner's parent is the index of `outer` (0).
        assert_eq!(inner.parent, Some(0));
    }

    #[test]
    fn direction_inside_subgraph_ignored() {
        let c = parse(
            "flowchart TD\n\
             subgraph one\n\
             direction LR\n\
             A --> B\n\
             end",
        );
        // Whole-chart direction is unchanged; direction line creates no node.
        assert_eq!(c.direction, Direction::TopDown);
        assert_eq!(c.subgraphs[0].node_ids, vec!["A", "B"]);
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B"]);
    }

    #[test]
    fn no_subgraph_chart_has_no_subgraphs() {
        let c = parse("flowchart TD\nA --> B --> C");
        assert!(c.subgraphs.is_empty());
    }

    // ── Click / interaction directives ─────────────────────────────────────

    #[test]
    fn click_url_sets_link() {
        let c = parse("graph TD\nA[Start]\nclick A \"https://x\"");
        assert_eq!(node(&c, "A").link.as_deref(), Some("https://x"));
        assert!(node(&c, "A").tooltip.is_none());
        assert!(node(&c, "A").callback.is_none());
    }

    #[test]
    fn click_href_url_sets_link() {
        let c = parse("graph TD\nA[Start]\nclick A href \"https://x\"");
        assert_eq!(node(&c, "A").link.as_deref(), Some("https://x"));
    }

    #[test]
    fn click_url_and_tooltip() {
        let c = parse("graph TD\nA[Start]\nclick A \"https://x\" \"go there\"");
        assert_eq!(node(&c, "A").link.as_deref(), Some("https://x"));
        assert_eq!(node(&c, "A").tooltip.as_deref(), Some("go there"));
    }

    #[test]
    fn click_href_url_and_tooltip() {
        let c = parse("graph TD\nA[Start]\nclick A href \"u\" \"tip\"");
        assert_eq!(node(&c, "A").link.as_deref(), Some("u"));
        assert_eq!(node(&c, "A").tooltip.as_deref(), Some("tip"));
    }

    #[test]
    fn click_call_sets_callback_dropping_args() {
        let c = parse("graph TD\nA\nclick A call doThing()");
        assert_eq!(node(&c, "A").callback.as_deref(), Some("doThing"));
        assert!(node(&c, "A").link.is_none());
    }

    #[test]
    fn click_call_with_args_and_tooltip() {
        let c = parse("graph TD\nA\nclick A call doThing(1, 2) \"tip\"");
        assert_eq!(node(&c, "A").callback.as_deref(), Some("doThing"));
        assert_eq!(node(&c, "A").tooltip.as_deref(), Some("tip"));
    }

    #[test]
    fn click_bareword_callback() {
        let c = parse("graph TD\nA\nclick A myCallback");
        assert_eq!(node(&c, "A").callback.as_deref(), Some("myCallback"));
    }

    #[test]
    fn click_url_with_target_ignored() {
        let c = parse("graph TD\nA\nclick A \"https://x\" _blank");
        assert_eq!(node(&c, "A").link.as_deref(), Some("https://x"));
        // _blank is tolerated and not recorded as a callback.
        assert!(node(&c, "A").callback.is_none());
    }

    #[test]
    fn click_unknown_id_autocreates_node() {
        let c = parse("graph TD\nA --> B\nclick Z \"https://z\"");
        let z = node(&c, "Z");
        assert_eq!(z.shape, NodeShape::Rect);
        assert_eq!(z.label, "Z");
        assert_eq!(z.link.as_deref(), Some("https://z"));
    }

    #[test]
    fn click_is_not_an_edge_or_node_statement() {
        // A click line must not create phantom nodes beyond its target, nor edges.
        let c = parse("graph TD\nA --> B\nclick A \"u\"");
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B"]);
        assert_eq!(c.edges.len(), 1);
    }

    #[test]
    fn directives_do_not_create_phantom_nodes() {
        let c = parse(
            "graph TD\n\
             A --> B\n\
             classDef hot fill:#f00\n\
             class A hot",
        );
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B"]);
    }
}
