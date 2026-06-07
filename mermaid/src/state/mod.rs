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
//! * composite/nested states `state X { ... }` (rendered as a labeled boundary
//!   box via the cluster API), `state f <<fork>>` / `<<join>>` (a thick bar),
//!   `state c <<choice>>` (a diamond), the `state "desc" as id` alias form, and
//!   `note left/right of S: text` / `note over S: text` / block notes.
//!
//! Skipped (note in report): `--` concurrency dividers.
//!
//! Split across submodules by stage: [`model`] (parsed types), [`parse`] (text →
//! model), [`render`] (sizing + layout + SVG drawing).

mod model;
mod parse;
mod render;

use crate::{HitRegion, MermaidError, MermaidOptions, MermaidRender};

/// Render a mermaid `state` diagram to SVG.
pub fn render_state(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    Ok(render::render(src, opts)?.0)
}

/// Like [`render_state`], but also returns one [`HitRegion`] per state node (its
/// drawn rect plus any `click` data), in SVG-px coords. Used by
/// `render_with_regions` to make state diagrams interactive.
pub fn render_state_with_regions(
    src: &str,
    opts: &MermaidOptions,
) -> Result<(MermaidRender, Vec<HitRegion>), MermaidError> {
    render::render(src, opts)
}

#[cfg(test)]
mod tests {
    use super::model::{NotePos, Pseudo, StateDiagram, StateKind};
    use super::parse::parse;
    use super::render::FORK_LEN;
    use super::{render_state, render_state_with_regions};
    use crate::model::ElemStyle;
    use crate::svgutil::rgb;
    use crate::{MermaidError, MermaidOptions};

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

    // ---- styling directives ----

    fn style_of<'a>(d: &'a StateDiagram, id: &str) -> &'a ElemStyle {
        &d.states.iter().find(|s| s.id == id).expect("state").style
    }

    #[test]
    fn classdef_and_class_apply() {
        let src = "stateDiagram-v2\n  Running --> Idle\n  classDef hl fill:#ff0\n  class Running hl";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Running").fill, Some([255, 255, 0, 255]));
        // Idle untouched.
        assert_eq!(style_of(&d, "Idle").fill, None);
    }

    #[test]
    fn triple_colon_shorthand() {
        let src = "stateDiagram-v2\n  Running:::hl --> Idle\n  classDef hl fill:#ff0";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Running").fill, Some([255, 255, 0, 255]));
        // Transition recorded with bare ids.
        assert_eq!(d.transitions[0].from, "Running");
        assert_eq!(d.transitions[0].to, "Idle");
    }

    #[test]
    fn triple_colon_with_label() {
        // `:::class` on the target plus a `: label` after it.
        let src = "stateDiagram-v2\n  Idle --> Running:::hl : go\n  classDef hl fill:#ff0";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Running").fill, Some([255, 255, 0, 255]));
        assert_eq!(d.transitions[0].to, "Running");
        assert_eq!(d.transitions[0].label.as_deref(), Some("go"));
    }

    #[test]
    fn style_directive_overrides_class() {
        let src = "stateDiagram-v2\n  Running --> Idle\n  classDef hl fill:#ff0\n  class Running hl\n  style Running fill:#00f";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Running").fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn style_override_in_rendered_svg() {
        let src = "stateDiagram-v2\n  Running --> Idle\n  classDef hl fill:#ff0\n  class Running hl";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains(&rgb([255, 255, 0, 255])), "override fill present: {}", r.svg);
    }

    #[test]
    fn unstyled_states_unchanged() {
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> s2 : x\n  s2 --> [*]";
        let d = parse(src).unwrap();
        for s in &d.states {
            assert_eq!(s.style, ElemStyle::default());
        }
    }

    // ---- composite / nested states ----

    #[test]
    fn parse_composite_nesting() {
        let src = "stateDiagram-v2\n state Active {\n  [*] --> Running\n  Running --> Idle\n }\n [*] --> Active";
        let d = parse(src).unwrap();
        // Active is a composite.
        let active = d.states.iter().find(|s| s.id == "Active").expect("Active");
        assert!(active.composite, "Active should be a composite");
        let active_i = d.states.iter().position(|s| s.id == "Active").unwrap();
        // Running and Idle are children of Active.
        let running = d.states.iter().find(|s| s.id == "Running").expect("Running");
        let idle = d.states.iter().find(|s| s.id == "Idle").expect("Idle");
        assert_eq!(running.parent, Some(active_i));
        assert_eq!(idle.parent, Some(active_i));
        // Active itself is top-level.
        assert_eq!(active.parent, None);
        // Transition Running --> Idle recorded.
        assert!(d
            .transitions
            .iter()
            .any(|t| t.from == "Running" && t.to == "Idle"));
    }

    #[test]
    fn composite_box_encloses_children() {
        let src = "stateDiagram-v2\n state Active {\n  [*] --> Running\n  Running --> Idle\n }\n [*] --> Active";
        let r = render_state(src, &opts()).unwrap();
        // The composite is drawn as a boundary box with its title.
        assert!(r.svg.contains(">Active<"), "composite title present");
        // Children render as their own rects.
        assert!(r.svg.contains(">Running<"));
        assert!(r.svg.contains(">Idle<"));
        // A separator <line> is part of the composite chrome.
        assert!(r.svg.contains("<line"), "composite separator line present");
        assert!(r.svg.starts_with("<svg") && r.svg.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn composite_box_geometrically_encloses_children() {
        let src = "stateDiagram-v2\n state Active {\n  [*] --> Running\n  Running --> Idle\n }\n [*] --> Active";
        let r = render_state(src, &opts()).unwrap();

        // Read attrs of the <rect> immediately preceding a given label text.
        fn rect_before(svg: &str, label: &str) -> (f32, f32, f32, f32) {
            let at = svg.find(&format!(">{label}<")).expect("label");
            let start = svg[..at].rfind("<rect").expect("rect");
            let rect = &svg[start..at];
            let attr = |name: &str| {
                let k = rect.find(name).unwrap() + name.len();
                let end = rect[k..].find('"').unwrap() + k;
                rect[k..end].parse::<f32>().unwrap()
            };
            (attr("x=\""), attr("y=\""), attr("width=\""), attr("height=\""))
        }

        let (bx, by, bw, bh) = rect_before(&r.svg, "Active");
        let (rx, ry, rw, rh) = rect_before(&r.svg, "Running");
        let (ix, iy, iw, ih) = rect_before(&r.svg, "Idle");

        // The composite box must enclose both children's rects.
        for (x, y, w, h) in [(rx, ry, rw, rh), (ix, iy, iw, ih)] {
            assert!(bx <= x + 0.5, "composite left {bx} <= child left {x}");
            assert!(by <= y + 0.5, "composite top {by} <= child top {y}");
            assert!(bx + bw >= x + w - 0.5, "composite right encloses child right");
            assert!(by + bh >= y + h - 0.5, "composite bottom encloses child bottom");
        }
    }

    // ---- fork / join / choice ----

    #[test]
    fn parse_fork_join_choice() {
        let src = "stateDiagram-v2\n state f <<fork>>\n state j <<join>>\n state c <<choice>>";
        let d = parse(src).unwrap();
        assert_eq!(d.states.iter().find(|s| s.id == "f").unwrap().kind, StateKind::Fork);
        assert_eq!(d.states.iter().find(|s| s.id == "j").unwrap().kind, StateKind::Join);
        assert_eq!(d.states.iter().find(|s| s.id == "c").unwrap().kind, StateKind::Choice);
    }

    #[test]
    fn fork_join_render_bars() {
        let src = "stateDiagram-v2\n state f <<fork>>\n [*] --> f\n f --> A\n f --> B";
        let r = render_state(src, &opts()).unwrap();
        // The fork bar is a thin rect of fixed bar size.
        assert!(
            r.svg.contains(&format!("width=\"{:.2}\"", FORK_LEN)),
            "fork bar width present"
        );
        // Two transitions split out of the fork.
        assert_eq!(r.svg.matches("marker-end=\"url(#state-arrow)\"").count(), 3);
    }

    #[test]
    fn join_render_bar() {
        let src = "stateDiagram-v2\n state j <<join>>\n A --> j\n B --> j\n j --> [*]";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains(&format!("width=\"{:.2}\"", FORK_LEN)));
    }

    #[test]
    fn choice_render_diamond() {
        let src = "stateDiagram-v2\n state c <<choice>>\n [*] --> c\n c --> A\n c --> B";
        let r = render_state(src, &opts()).unwrap();
        // Choice renders as a polygon (diamond).
        assert!(r.svg.contains("<polygon"), "choice diamond present");
    }

    // ---- notes ----

    #[test]
    fn parse_note_inline() {
        let src = "stateDiagram-v2\n A --> B\n note right of A: hello";
        let d = parse(src).unwrap();
        assert_eq!(d.notes.len(), 1);
        assert_eq!(d.notes[0].target, "A");
        assert_eq!(d.notes[0].pos, NotePos::Right);
        assert_eq!(d.notes[0].text, "hello");
    }

    #[test]
    fn note_renders_rect_and_text() {
        let src = "stateDiagram-v2\n A --> B\n note right of A: hello";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains(">hello<"), "note text present");
        // The pale note fill color is present.
        assert!(r.svg.contains(&rgb([255, 245, 181, 255])), "note fill present");
    }

    #[test]
    fn parse_note_block_multiline() {
        let src =
            "stateDiagram-v2\n A --> B\n note left of A\n  line one\n  line two\n end note";
        let d = parse(src).unwrap();
        assert_eq!(d.notes.len(), 1);
        assert_eq!(d.notes[0].pos, NotePos::Left);
        assert_eq!(d.notes[0].text, "line one\nline two");
    }

    #[test]
    fn note_over_parsed() {
        let src = "stateDiagram-v2\n A --> B\n note over A: spanning";
        let d = parse(src).unwrap();
        assert_eq!(d.notes[0].pos, NotePos::Over);
        assert_eq!(d.notes[0].target, "A");
    }

    // ---- alias / direction ----

    #[test]
    fn state_alias_form() {
        let src = "stateDiagram-v2\n state \"Long description\" as S\n [*] --> S";
        let d = parse(src).unwrap();
        let s = d.states.iter().find(|s| s.id == "S").expect("S");
        assert_eq!(s.label, "Long description");
    }

    #[test]
    fn direction_ignored() {
        let src = "stateDiagram-v2\n direction LR\n A --> B";
        let d = parse(src).unwrap();
        assert_eq!(d.transitions.len(), 1);
    }

    #[test]
    fn state_keyword_bare_decl() {
        // `state X` declares X; a later `X : desc` sets its label.
        let src = "stateDiagram-v2\n state Foo\n Foo --> Bar";
        let d = parse(src).unwrap();
        assert!(d.states.iter().any(|s| s.id == "Foo"));
        assert!(d.transitions.iter().any(|t| t.from == "Foo" && t.to == "Bar"));
    }

    #[test]
    fn simple_diagram_no_clusters_unchanged() {
        // A no-composite/no-special diagram must produce no composite chrome.
        let src = "stateDiagram-v2\n  [*] --> s1\n  s1 --> s2 : next\n  s2 --> [*]";
        let r = render_state(src, &opts()).unwrap();
        assert!(!r.svg.contains("<line"), "no composite separator in simple diagram");
        assert!(!r.svg.contains("<polygon"), "no diamonds in simple diagram");
        // Two real states → two <rect>.
        assert_eq!(r.svg.matches("<rect").count(), 2);
    }

    #[test]
    fn state_name_renders_inline_math() {
        // A state described with `$…$` renders the embedded math group.
        let src = "stateDiagram-v2\n  s1 : energy $x^2$\n  s1 --> s2";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains("<g transform"), "expected math group: {}", r.svg);
        assert!(r.svg.contains("<path"), "expected math path: {}", r.svg);
    }

    #[test]
    fn transition_label_renders_bold_markdown() {
        // A `**bold**` transition label renders a bold run, not literal `**`.
        let src = "stateDiagram-v2\n  s1 --> s2 : **go**";
        let r = render_state(src, &opts()).unwrap();
        assert!(r.svg.contains("font-weight=\"bold\""), "expected bold run: {}", r.svg);
        assert!(!r.svg.contains("**go**"), "raw markdown leaked: {}", r.svg);
    }

    #[test]
    fn composite_deterministic() {
        let src = "stateDiagram-v2\n state Active {\n  [*] --> Running\n  Running --> Idle\n }\n [*] --> Active";
        let a = render_state(src, &opts()).unwrap();
        let b = render_state(src, &opts()).unwrap();
        assert_eq!(a, b);
    }

    // ---- click / interaction ----

    #[test]
    fn click_sets_link_and_tooltip() {
        let src = "stateDiagram-v2\n s1 --> s2\n click s1 \"https://x\" \"tip\"\n";
        let d = parse(src).unwrap();
        let s1 = d.states.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(s1.link.as_deref(), Some("https://x"));
        assert_eq!(s1.tooltip.as_deref(), Some("tip"));
        assert!(s1.callback.is_none());
        // Unknown id is skipped, not fabricated.
        let d2 = parse("stateDiagram-v2\n s1 --> s2\n click Ghost \"https://y\"\n").unwrap();
        assert!(d2.states.iter().all(|s| s.id != "Ghost"));
    }

    #[test]
    fn regions_carry_click_data() {
        let src = "stateDiagram-v2\n s1 --> s2\n click s1 \"https://x\" \"tip\"\n";
        let (render, regions) = render_state_with_regions(src, &opts()).unwrap();
        let r = regions.iter().find(|r| r.id == "s1").unwrap();
        assert_eq!(r.link.as_deref(), Some("https://x"));
        assert_eq!(r.tooltip.as_deref(), Some("tip"));
        assert!(r.w > 0.0 && r.h > 0.0);
        assert!(r.x >= 0.0 && r.y >= 0.0);
        assert!(r.x + r.w <= render.width_px + 1.0);
        assert!(r.y + r.h <= render.height_px + 1.0);
    }

    #[test]
    fn regions_without_click_and_svg_unchanged() {
        let src = "stateDiagram-v2\n s1 --> s2\n s2 --> s3\n";
        let plain = render_state(src, &opts()).unwrap();
        let (with_regions, regions) = render_state_with_regions(src, &opts()).unwrap();
        // A region for each of s1/s2/s3 (plus pseudo states), all click-free.
        for id in ["s1", "s2", "s3"] {
            let r = regions.iter().find(|r| r.id == id).unwrap();
            assert!(r.link.is_none() && r.callback.is_none() && r.tooltip.is_none());
            assert!(r.w > 0.0 && r.h > 0.0);
        }
        assert_eq!(plain.svg, with_regions.svg);
        assert_eq!(plain.width_px, with_regions.width_px);
        assert_eq!(plain.height_px, with_regions.height_px);
    }
}
