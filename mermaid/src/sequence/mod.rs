//! Sequence diagram (self-contained: parse + layout + draw, no dagre).
//!
//! Mermaid sequence syntax (subset supported here):
//! ```text
//! sequenceDiagram
//!     participant A
//!     participant B as Bob
//!     actor C
//!     A->>B: Hello Bob
//!     B-->>A: Hi Alice
//!     A-)B: async
//!     A->>A: think
//! ```
//! Self-layout: participants become **columns** (x positions) with vertical
//! dashed lifelines; messages become **horizontal arrows** at increasing y.
//!
//! ## Supported
//! - Header `sequenceDiagram`.
//! - `participant <id>` / `participant <id> as <Label>` / `actor <id>`.
//! - Auto-created participants (first appearance order) for ids used in a
//!   message but never declared.
//! - Messages with arrow tokens between two ids and an optional `: text`:
//!   `->>`/`-->>` (filled arrowhead, solid/**dashed**), `->`/`-->` (open V),
//!   `-)`/`--)` (async open V), `-x`/`--x` (cross end). The `--` variants are
//!   dashed.
//! - Self-messages (`A->>A: text`) draw a small loop to the right of A's
//!   lifeline.
//!
//! ## Advanced features (v2)
//! - **Notes** — `Note left of A: t`, `Note right of A: t`, `Note over A: t`,
//!   `Note over A,B: t`. Drawn as a themed rectangle on a vertical row.
//! - **Activations** — `activate A` / `deactivate A`, and the `+`/`-` suffixes
//!   on message targets/sources (`A->>+B`, `B-->>-A`). Drawn as narrow vertical
//!   bars on the lifeline; nested activations offset horizontally.
//! - **Block frames** — `loop`/`opt`/`alt`/`par`/`break`/`critical` … `end`,
//!   with `else`/`and`/`option` section dividers. Drawn as labeled frames.
//! - **autonumber** — `autonumber` (optionally `autonumber <start>` /
//!   `autonumber <start> <step>`) prefixes each subsequent message with a small
//!   numbered badge.
//! - **rect background blocks** — `rect <color> … end` (color as `rgb(...)`,
//!   `rgba(...)`, or `#hex`) draws a translucent highlight behind the contained
//!   rows; nests freely with block frames.
//!
//! ## Skipped (intentionally)
//! Participant `links`/`box` grouping, and `break`-specific styling beyond a
//! plain frame.
//!
//! Split across submodules by stage: [`model`] (parsed types), [`parse`] (text →
//! model), [`render`] (self-layout + SVG drawing).

mod model;
mod parse;
mod render;

use crate::{MermaidError, MermaidOptions, MermaidRender};

/// Render a mermaid `sequenceDiagram` to SVG.
pub fn render_sequence(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    render::render(src, opts)
}

#[cfg(test)]
mod tests {
    use super::model::{
        ArrowStyle, BlockKind, Item, Message, Note, NotePlacement, Participant, Rgba,
        SequenceDiagram,
    };
    use super::parse::parse;
    use super::render_sequence;
    use crate::{MermaidError, MermaidOptions};

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }
    /// Flatten the item tree (recursing into blocks) into the messages it
    /// contains, in order — keeps the older message-shape tests concise.
    fn flat_messages(d: &SequenceDiagram) -> Vec<Message> {
        fn walk(items: &[Item], out: &mut Vec<Message>) {
            for it in items {
                match it {
                    Item::Message(m) => out.push(m.clone()),
                    Item::Block(b) => walk(&b.items, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&d.items, &mut out);
        out
    }

    // --- Parsing ---

    #[test]
    fn parses_declared_and_alias_participants() {
        let d = parse(
            "sequenceDiagram\n participant A\n participant B as Bob\n actor C\n A->>B: hi\n",
        )
        .unwrap();
        assert_eq!(d.participants.len(), 3);
        assert_eq!(d.participants[0], Participant { id: "A".into(), label: "A".into() });
        assert_eq!(d.participants[1], Participant { id: "B".into(), label: "Bob".into() });
        assert_eq!(d.participants[2], Participant { id: "C".into(), label: "C".into() });
    }

    #[test]
    fn auto_creates_undeclared_participants_in_first_appearance_order() {
        let d = parse("sequenceDiagram\n X->>Y: hi\n Y-->>Z: bye\n").unwrap();
        let ids: Vec<_> = d.participants.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["X", "Y", "Z"]);
    }

    #[test]
    fn parses_each_arrow_kind() {
        let d = parse(
            "sequenceDiagram\n A->>B: a\n A-->>B: b\n A->B: c\n A-->B: d\n A-)B: e\n A-xB: f\n A--xB: g\n",
        )
        .unwrap();
        let msgs = flat_messages(&d);
        assert_eq!(msgs.len(), 7);
        assert_eq!(msgs[0].style, ArrowStyle::Filled);
        assert!(!msgs[0].dashed);
        assert_eq!(msgs[1].style, ArrowStyle::Filled);
        assert!(msgs[1].dashed);
        assert_eq!(msgs[2].style, ArrowStyle::Open);
        assert_eq!(msgs[3].style, ArrowStyle::Open);
        assert!(msgs[3].dashed);
        assert_eq!(msgs[4].style, ArrowStyle::Async);
        assert_eq!(msgs[5].style, ArrowStyle::Cross);
        assert_eq!(msgs[6].style, ArrowStyle::Cross);
        assert!(msgs[6].dashed);
    }

    #[test]
    fn parses_message_text_and_endpoints() {
        let d = parse("sequenceDiagram\n Alice ->> Bob : Hello Bob\n").unwrap();
        let msgs = flat_messages(&d);
        let m = &msgs[0];
        assert_eq!(m.from, "Alice");
        assert_eq!(m.to, "Bob");
        assert_eq!(m.text, "Hello Bob");
    }

    #[test]
    fn parses_self_message() {
        let d = parse("sequenceDiagram\n A->>A: think\n").unwrap();
        let msgs = flat_messages(&d);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].from, "A");
        assert_eq!(msgs[0].to, "A");
        // Only one participant created.
        assert_eq!(d.participants.len(), 1);
    }

    #[test]
    fn parses_blocks_and_notes_into_tree() {
        let d = parse(
            "sequenceDiagram\n participant A\n participant B\n loop every minute\n A->>B: ping\n end\n Note over A,B: hi\n",
        )
        .unwrap();
        assert_eq!(d.participants.len(), 2);
        // The loop is a Block; the note follows at top level.
        assert_eq!(d.items.len(), 2);
        match &d.items[0] {
            Item::Block(b) => {
                assert_eq!(b.kind, BlockKind::Loop);
                assert_eq!(b.label, "every minute");
                assert_eq!(b.items.len(), 1);
                assert!(matches!(b.items[0], Item::Message(_)));
            }
            other => panic!("expected a loop block, got {other:?}"),
        }
        match &d.items[1] {
            Item::Note(n) => {
                assert_eq!(n.placement, NotePlacement::Over);
                assert_eq!(n.targets, vec!["A".to_string(), "B".to_string()]);
                assert_eq!(n.text, "hi");
            }
            other => panic!("expected a note, got {other:?}"),
        }
        // Still exactly one message total.
        assert_eq!(flat_messages(&d).len(), 1);
    }

    #[test]
    fn alias_updates_label_for_message_first_id() {
        // B appears in a message first, then a declaration aliases it.
        let d = parse("sequenceDiagram\n A->>B: hi\n participant B as Bob\n").unwrap();
        let b = d.participants.iter().find(|p| p.id == "B").unwrap();
        assert_eq!(b.label, "Bob");
    }

    #[test]
    fn missing_header_is_parse_error() {
        assert!(parse("participant A\n A->>B: hi\n").is_err());
    }

    #[test]
    fn malformed_message_is_parse_error() {
        // No arrow token.
        assert!(parse("sequenceDiagram\n A B C\n").is_err());
    }

    // --- Rendering ---

    #[test]
    fn renders_svg_envelope() {
        let r = render_sequence("sequenceDiagram\n A->>B: hi\n", &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"), "got {}", &r.svg[..r.svg.len().min(40)]);
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn one_box_and_lifeline_per_participant() {
        let r = render_sequence(
            "sequenceDiagram\n participant A\n participant B\n A->>B: hi\n",
            &opts(),
        )
        .unwrap();
        // Two participant <rect>s.
        assert_eq!(r.svg.matches("<rect").count(), 2);
        // Two dashed lifelines (3 3 dash); message dashes use 4 3.
        assert_eq!(r.svg.matches("stroke-dasharray=\"3 3\"").count(), 2);
    }

    #[test]
    fn message_line_and_text_present() {
        let r = render_sequence("sequenceDiagram\n A->>B: Hello\n", &opts()).unwrap();
        // At least one <line> for the message (plus 2 lifelines).
        assert!(r.svg.matches("<line").count() >= 3);
        assert!(r.svg.contains(">Hello</text>"));
    }

    #[test]
    fn filled_arrow_uses_marker() {
        let r = render_sequence("sequenceDiagram\n A->>B: hi\n", &opts()).unwrap();
        assert!(r.svg.contains("marker-end=\"url(#seq-arrow)\""));
        assert!(r.svg.contains("<marker id=\"seq-arrow\""));
    }

    #[test]
    fn dashed_message_has_dash_pattern() {
        let r = render_sequence("sequenceDiagram\n A-->>B: hi\n", &opts()).unwrap();
        // Message dash pattern (distinct from lifeline 3 3).
        assert!(r.svg.contains("stroke-dasharray=\"4 3\""));
    }

    #[test]
    fn solid_message_has_no_message_dash() {
        let r = render_sequence("sequenceDiagram\n A->>B: hi\n", &opts()).unwrap();
        assert!(!r.svg.contains("stroke-dasharray=\"4 3\""));
    }

    #[test]
    fn cross_arrow_draws_a_cross() {
        let r = render_sequence("sequenceDiagram\n A-xB: bye\n", &opts()).unwrap();
        // The cross has two strokes joined by an `M` move within one path.
        assert!(r.svg.contains(" M"), "expected a cross path with a move-to: {}", r.svg);
    }

    #[test]
    fn self_message_renders_loop_path() {
        let r = render_sequence("sequenceDiagram\n A->>A: think\n", &opts()).unwrap();
        // The loop is a multi-segment <path>.
        assert!(r.svg.contains("<path d=\"M"));
        assert!(r.svg.contains(">think</text>"));
    }

    #[test]
    fn auto_created_participant_from_message_renders() {
        // Z never declared; should still get a box.
        let r = render_sequence("sequenceDiagram\n participant A\n A->>Z: hi\n", &opts()).unwrap();
        assert_eq!(r.svg.matches("<rect").count(), 2);
        assert!(r.svg.contains(">Z</text>"));
    }

    #[test]
    fn xml_escapes_message_text() {
        let r = render_sequence("sequenceDiagram\n A->>B: a & b < c\n", &opts()).unwrap();
        assert!(r.svg.contains("a &amp; b &lt; c"));
        assert!(!r.svg.contains("a & b"));
    }

    #[test]
    fn empty_input_is_error() {
        // No header at all → a Parse error (not Empty).
        assert!(matches!(render_sequence("", &opts()), Err(MermaidError::Parse(_))));
    }

    #[test]
    fn no_participants_is_empty_error() {
        // Header only, no participants / messages.
        assert!(matches!(render_sequence("sequenceDiagram\n", &opts()), Err(MermaidError::Empty)));
    }

    #[test]
    fn deterministic_output() {
        let src = "sequenceDiagram\n participant A\n participant B as Bob\n A->>B: hi\n B-->>A: yo\n A->>A: think\n";
        let a = render_sequence(src, &opts()).unwrap();
        let b = render_sequence(src, &opts()).unwrap();
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }

    // --- Advanced: parsing ---

    #[test]
    fn parses_alt_else_block_tree() {
        let d = parse(
            "sequenceDiagram\n alt is ok\n A->>B: yes\n else not ok\n A->>B: no\n end\n",
        )
        .unwrap();
        assert_eq!(d.items.len(), 1);
        let Item::Block(b) = &d.items[0] else { panic!("expected block") };
        assert_eq!(b.kind, BlockKind::Alt);
        assert_eq!(b.label, "is ok");
        // Two messages, one section divider before the second.
        assert_eq!(b.items.len(), 2);
        assert_eq!(b.sections.len(), 1);
        assert_eq!(b.sections[0], (1, "not ok".to_string()));
    }

    #[test]
    fn parses_nested_loop_in_alt() {
        let d = parse(
            "sequenceDiagram\n alt x\n loop y\n A->>B: m\n end\n else z\n A->>B: n\n end\n",
        )
        .unwrap();
        let Item::Block(alt) = &d.items[0] else { panic!() };
        assert_eq!(alt.kind, BlockKind::Alt);
        assert!(matches!(alt.items[0], Item::Block(_)));
        // The else divider sits before the second item (index 1).
        assert_eq!(alt.sections, vec![(1, "z".to_string())]);
    }

    #[test]
    fn parses_note_placements() {
        let d = parse(
            "sequenceDiagram\n participant A\n participant B\n Note left of A: l\n Note right of B: r\n Note over A,B: o\n",
        )
        .unwrap();
        let notes: Vec<&Note> = d.items.iter().filter_map(|i| match i {
            Item::Note(n) => Some(n),
            _ => None,
        }).collect();
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0].placement, NotePlacement::LeftOf);
        assert_eq!(notes[0].targets, vec!["A".to_string()]);
        assert_eq!(notes[1].placement, NotePlacement::RightOf);
        assert_eq!(notes[2].placement, NotePlacement::Over);
        assert_eq!(notes[2].targets, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn parses_activation_suffixes() {
        let d = parse("sequenceDiagram\n A->>+B: go\n B-->>-A: done\n").unwrap();
        let msgs = flat_messages(&d);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0].activate_to);
        assert!(!msgs[0].deactivate_from);
        assert_eq!(msgs[0].to, "B");
        assert!(msgs[1].deactivate_from);
        assert!(!msgs[1].activate_to);
        assert_eq!(msgs[1].to, "A");
        assert_eq!(msgs[1].from, "B");
    }

    #[test]
    fn parses_explicit_activate_deactivate() {
        let d = parse(
            "sequenceDiagram\n activate A\n A->>B: hi\n deactivate A\n",
        )
        .unwrap();
        assert!(matches!(d.items[0], Item::Activate(ref s) if s == "A"));
        assert!(matches!(d.items[2], Item::Deactivate(ref s) if s == "A"));
    }

    #[test]
    fn unterminated_block_is_error() {
        assert!(parse("sequenceDiagram\n loop x\n A->>B: hi\n").is_err());
    }

    #[test]
    fn stray_end_is_error() {
        assert!(parse("sequenceDiagram\n A->>B: hi\n end\n").is_err());
    }

    // --- Advanced: rendering ---

    #[test]
    fn renders_loop_frame_with_keyword() {
        let r = render_sequence(
            "sequenceDiagram\n loop retry\n A->>B: ping\n end\n",
            &opts(),
        )
        .unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        // Frame keyword tab label + opening label.
        assert!(r.svg.contains(">loop</text>"), "no loop keyword: {}", r.svg);
        assert!(r.svg.contains(">retry</text>"));
        // The frame and the tab both draw <rect>/<path>; at least a frame rect.
        assert!(r.svg.matches("<rect").count() >= 3);
    }

    #[test]
    fn renders_alt_else_divider() {
        let r = render_sequence(
            "sequenceDiagram\n alt ok\n A->>B: yes\n else fail\n A->>B: no\n end\n",
            &opts(),
        )
        .unwrap();
        assert!(r.svg.contains(">alt</text>"));
        assert!(r.svg.contains(">ok</text>"));
        // The else section label.
        assert!(r.svg.contains(">fail</text>"));
        // A dashed divider line uses the 3 3 pattern (like lifelines): lifelines
        // = 2, plus at least one divider ⇒ >= 3.
        assert!(r.svg.matches("stroke-dasharray=\"3 3\"").count() >= 3);
    }

    #[test]
    fn renders_note_rect_and_text() {
        let r = render_sequence(
            "sequenceDiagram\n participant A\n participant B\n Note over A,B: hello note\n",
            &opts(),
        )
        .unwrap();
        // Pale-yellow note fill.
        assert!(r.svg.contains("fill=\"rgb(255,255,221)\""), "no note rect: {}", r.svg);
        assert!(r.svg.contains(">hello note</text>"));
    }

    #[test]
    fn renders_activation_bar_on_lifeline() {
        let r = render_sequence(
            "sequenceDiagram\n A->>+B: go\n B-->>-A: done\n",
            &opts(),
        )
        .unwrap();
        // The activation bar is a node-fill rect (same fill as participant
        // boxes). Boxes (2) + at least one activation bar ⇒ >= 3 such rects.
        let fill = "fill=\"rgb(236,236,255)\"";
        assert!(r.svg.matches(fill).count() >= 3, "no activation bar rect: {}", r.svg);
    }

    #[test]
    fn plain_diagram_unchanged_by_advanced_code() {
        // A diagram with no blocks/notes/activations should render exactly the
        // same structure as before: only participant rects + lifelines + the
        // message, no frames/notes/bars.
        let r = render_sequence(
            "sequenceDiagram\n participant A\n participant B\n A->>B: hi\n",
            &opts(),
        )
        .unwrap();
        // Exactly two rects (the two participant boxes; no frames/notes/bars).
        assert_eq!(r.svg.matches("<rect").count(), 2);
        // Exactly two dashed 3 3 lifelines, no dividers.
        assert_eq!(r.svg.matches("stroke-dasharray=\"3 3\"").count(), 2);
        // No note fill.
        assert!(!r.svg.contains("rgb(255,255,221)"));
    }

    #[test]
    fn nested_activation_offsets_horizontally() {
        // Two stacked activations on B → two bars, the inner offset.
        let r = render_sequence(
            "sequenceDiagram\n A->>+B: a\n A->>+B: b\n B-->>-A: c\n B-->>-A: d\n",
            &opts(),
        )
        .unwrap();
        let fill = "fill=\"rgb(236,236,255)\"";
        // 2 boxes + 2 activation bars.
        assert!(r.svg.matches(fill).count() >= 4, "expected nested bars: {}", r.svg);
    }

    #[test]
    fn advanced_features_deterministic() {
        let src = "sequenceDiagram\n participant A\n participant B\n loop r\n A->>+B: go\n Note right of B: working\n B-->>-A: ok\n end\n alt x\n A->>B: y\n else z\n A->>B: n\n end\n";
        let a = render_sequence(src, &opts()).unwrap();
        let b = render_sequence(src, &opts()).unwrap();
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }

    #[test]
    fn message_label_renders_inline_math() {
        // A message whose text contains `$…$` now renders the embedded math
        // group rather than a plain `<text>` with the raw dollar signs.
        let r = render_sequence("sequenceDiagram\n A->>B: speed $x^2$\n", &opts()).unwrap();
        assert!(r.svg.contains("<g transform"), "expected math group: {}", r.svg);
        assert!(r.svg.contains("<path"), "expected math path: {}", r.svg);
    }

    #[test]
    fn note_label_renders_bold_markdown() {
        // A `**bold**` note renders a bold tspan/text instead of literal `**`.
        let r = render_sequence(
            "sequenceDiagram\n participant A\n Note over A: **warn**\n",
            &opts(),
        )
        .unwrap();
        assert!(r.svg.contains("font-weight=\"bold\""), "expected bold run: {}", r.svg);
        assert!(!r.svg.contains("**warn**"), "raw markdown leaked: {}", r.svg);
    }

    #[test]
    fn note_xml_escapes() {
        let r = render_sequence(
            "sequenceDiagram\n participant A\n Note over A: a & b < c\n",
            &opts(),
        )
        .unwrap();
        assert!(r.svg.contains("a &amp; b &lt; c"));
    }

    // --- autonumber ---

    #[test]
    fn autonumber_assigns_sequential_numbers_to_messages_only() {
        let d = parse(
            "sequenceDiagram\n autonumber\n A->>B: a\n Note over A: skip\n A->>B: b\n A->>B: c\n",
        )
        .unwrap();
        let msgs = flat_messages(&d);
        assert_eq!(msgs.len(), 3);
        // Notes don't consume a number; messages are 1,2,3.
        assert_eq!(msgs[0].number, Some(1));
        assert_eq!(msgs[1].number, Some(2));
        assert_eq!(msgs[2].number, Some(3));
    }

    #[test]
    fn no_autonumber_leaves_messages_unnumbered() {
        let d = parse("sequenceDiagram\n A->>B: a\n A->>B: b\n").unwrap();
        let msgs = flat_messages(&d);
        assert!(msgs.iter().all(|m| m.number.is_none()));
    }

    #[test]
    fn autonumber_start_and_step() {
        let d = parse(
            "sequenceDiagram\n autonumber 10 5\n A->>B: a\n A->>B: b\n",
        )
        .unwrap();
        let msgs = flat_messages(&d);
        assert_eq!(msgs[0].number, Some(10));
        assert_eq!(msgs[1].number, Some(15));
    }

    #[test]
    fn autonumber_renders_badge_with_number() {
        let r = render_sequence(
            "sequenceDiagram\n autonumber\n A->>B: hi\n B->>A: yo\n",
            &opts(),
        )
        .unwrap();
        // Two badge circles, one per message.
        assert_eq!(r.svg.matches("<circle").count(), 2);
        assert!(r.svg.contains(">1</text>"), "no number 1 badge: {}", r.svg);
        assert!(r.svg.contains(">2</text>"));
    }

    #[test]
    fn no_autonumber_renders_no_badge() {
        let r = render_sequence("sequenceDiagram\n A->>B: hi\n", &opts()).unwrap();
        assert!(!r.svg.contains("<circle"));
    }

    // --- rect background blocks ---

    #[test]
    fn parses_rect_block_with_rgb_color() {
        let d = parse(
            "sequenceDiagram\n rect rgb(230,230,250)\n A->>B: x\n end\n",
        )
        .unwrap();
        assert_eq!(d.items.len(), 1);
        let Item::Rect(rb) = &d.items[0] else { panic!("expected rect block: {:?}", d.items) };
        assert_eq!(rb.color, Rgba { r: 230, g: 230, b: 250, a: 80 });
        assert_eq!(rb.items.len(), 1);
        assert!(matches!(rb.items[0], Item::Message(_)));
    }

    #[test]
    fn parses_rect_rgba_and_hex() {
        let d = parse(
            "sequenceDiagram\n rect rgba(10,20,30,0.5)\n A->>B: x\n end\n rect #abc\n A->>B: y\n end\n",
        )
        .unwrap();
        let Item::Rect(a) = &d.items[0] else { panic!() };
        assert_eq!(a.color, Rgba { r: 10, g: 20, b: 30, a: 128 });
        let Item::Rect(b) = &d.items[1] else { panic!() };
        assert_eq!(b.color, Rgba { r: 0xaa, g: 0xbb, b: 0xcc, a: 80 });
    }

    #[test]
    fn renders_rect_translucent_background() {
        let r = render_sequence(
            "sequenceDiagram\n rect rgb(230,230,250)\n A->>B: x\n end\n",
            &opts(),
        )
        .unwrap();
        // A translucent fill behind the message.
        assert!(
            r.svg.contains("fill=\"rgb(230,230,250)\" fill-opacity="),
            "no rect background: {}",
            r.svg
        );
        // The contained message still renders.
        assert!(r.svg.contains(">x</text>"));
    }

    #[test]
    fn rect_nests_with_block_frames() {
        let r = render_sequence(
            "sequenceDiagram\n loop r\n rect rgb(200,255,200)\n A->>B: x\n end\n end\n",
            &opts(),
        )
        .unwrap();
        // Both the loop frame keyword and the rect background show.
        assert!(r.svg.contains(">loop</text>"));
        assert!(r.svg.contains("fill=\"rgb(200,255,200)\" fill-opacity="));
    }

    #[test]
    fn bad_rect_color_is_parse_error() {
        assert!(parse(
            "sequenceDiagram\n rect notacolor\n A->>B: x\n end\n"
        )
        .is_err());
    }

    #[test]
    fn plain_diagram_unchanged_by_autonumber_and_rect_code() {
        // No autonumber, no rect ⇒ no badges, no translucent backgrounds.
        let r = render_sequence(
            "sequenceDiagram\n participant A\n participant B\n A->>B: hi\n",
            &opts(),
        )
        .unwrap();
        assert!(!r.svg.contains("<circle"));
        assert!(!r.svg.contains("fill-opacity"));
    }

    #[test]
    fn autonumber_and_rect_deterministic() {
        let src = "sequenceDiagram\n autonumber\n rect rgb(230,230,250)\n A->>B: a\n B-->>A: b\n end\n A->>B: c\n";
        let a = render_sequence(src, &opts()).unwrap();
        let b = render_sequence(src, &opts()).unwrap();
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
