//! `er` (entity-relationship) diagram (`erDiagram`).
//!
//! Self-contained: parse → build a `hiker_graph` layered (dagre) graph → lay
//! out → draw one SVG document. Supported subset:
//!
//! * relationships `CUSTOMER ||--o{ ORDER : places` — two entity names, a
//!   cardinality token pair `<left>--<right>` (identifying / solid) or
//!   `<left>..<right>` (non-identifying / dashed), and an optional `: label`.
//!   Cardinality tokens: `||` exactly-one, `|{`/`}|` one-or-many, `o{`/`}o`
//!   zero-or-many, `o|`/`|o` zero-or-one.
//! * entities are auto-created in first-seen order.
//! * an entity attribute block `CUSTOMER { string name PK \n int age }` — each
//!   row is `type name [keys...]`; rows render under the entity's name header.
//!
//! Cardinality is rendered with proper **crow's-foot notation**: small
//! line/path marks drawn at each entity end of the relationship line, oriented
//! along that line's terminal segment (a double bar for exactly-one, an open
//! circle for the zero forms, a splayed crow's foot for the many forms).
//! Non-identifying relationships draw a dashed line.
//!
//! Split across submodules by stage: [`model`] (parsed types), [`parse`] (text →
//! model), [`render`] (sizing + layout + SVG drawing).

mod model;
mod parse;
mod render;

use crate::{HitRegion, MermaidError, MermaidOptions, MermaidRender};

/// Render a mermaid `er` diagram to SVG.
pub fn render_er(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    Ok(render::render(src, opts)?.0)
}

/// Like [`render_er`], but also returns one [`HitRegion`] per entity box (its
/// drawn rect plus any `click` data), in SVG-px coords. Used by
/// `render_with_regions` to make ER diagrams interactive.
pub fn render_er_with_regions(
    src: &str,
    opts: &MermaidOptions,
) -> Result<(MermaidRender, Vec<HitRegion>), MermaidError> {
    render::render(src, opts)
}

#[cfg(test)]
mod tests {
    use super::model::{Cardinality, ErDiagram};
    use super::parse::parse;
    use super::{render_er, render_er_with_regions};
    use crate::model::ElemStyle;
    use crate::svgutil::rgb;
    use crate::{MermaidError, MermaidOptions};

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    #[test]
    fn parse_relationship_basic() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places";
        let d = parse(src).unwrap();
        assert_eq!(d.entities.len(), 2);
        assert_eq!(d.entities[0].name, "CUSTOMER");
        assert_eq!(d.entities[1].name, "ORDER");
        assert_eq!(d.relationships.len(), 1);
        let r = &d.relationships[0];
        assert_eq!(r.left_card, Cardinality::ExactlyOne);
        assert_eq!(r.right_card, Cardinality::ZeroOrMore);
        assert!(!r.dashed);
        assert_eq!(r.label.as_deref(), Some("places"));
    }

    #[test]
    fn parse_all_cardinalities() {
        let src = "erDiagram\n  A |{--}o B\n  C o|--|o D";
        let d = parse(src).unwrap();
        assert_eq!(d.relationships[0].left_card, Cardinality::OneOrMore);
        assert_eq!(d.relationships[0].right_card, Cardinality::ZeroOrMore);
        assert_eq!(d.relationships[1].left_card, Cardinality::ZeroOrOne);
        assert_eq!(d.relationships[1].right_card, Cardinality::ZeroOrOne);
    }

    #[test]
    fn parse_non_identifying_is_dashed() {
        let src = "erDiagram\n  A ||..o{ B";
        let d = parse(src).unwrap();
        assert!(d.relationships[0].dashed);
    }

    #[test]
    fn parse_attribute_block() {
        let src = "erDiagram\n  CUSTOMER {\n    string name PK\n    int age\n  }";
        let d = parse(src).unwrap();
        assert_eq!(d.entities.len(), 1);
        let attrs = &d.entities[0].attrs;
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].ty, "string");
        assert_eq!(attrs[0].name, "name");
        assert_eq!(attrs[0].keys, vec!["PK"]);
        assert_eq!(attrs[0].comment, None);
        assert_eq!(attrs[1].ty, "int");
        assert_eq!(attrs[1].name, "age");
        assert!(attrs[1].keys.is_empty());
        assert_eq!(attrs[1].comment, None);
    }

    #[test]
    fn parse_attribute_keys_and_comment() {
        // From the task: `CUSTOMER { string name PK "the name" int age }`.
        let src = "erDiagram\n  CUSTOMER {\n    string name PK \"the name\"\n    int age\n  }";
        let d = parse(src).unwrap();
        let attrs = &d.entities[0].attrs;
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].name, "name");
        assert_eq!(attrs[0].keys, vec!["PK"]);
        assert_eq!(attrs[0].comment.as_deref(), Some("the name"));
        assert!(attrs[0].is_pk());
        // `age` has no key and no comment.
        assert_eq!(attrs[1].name, "age");
        assert!(attrs[1].keys.is_empty());
        assert_eq!(attrs[1].comment, None);
        assert!(!attrs[1].is_pk());
    }

    #[test]
    fn parse_multiple_keys_comma_and_space() {
        let src = "erDiagram\n  T {\n    int a PK,FK \"c1\"\n    int b PK UK\n  }";
        let d = parse(src).unwrap();
        let attrs = &d.entities[0].attrs;
        assert_eq!(attrs[0].keys, vec!["PK", "FK"]);
        assert_eq!(attrs[0].comment.as_deref(), Some("c1"));
        assert_eq!(attrs[1].keys, vec!["PK", "UK"]);
        assert_eq!(attrs[1].comment, None);
    }

    #[test]
    fn bad_header_errors() {
        assert!(parse("graph TD\n a --> b").is_err());
    }

    #[test]
    fn no_header_errors() {
        assert!(parse("\n\n").is_err());
    }

    #[test]
    fn render_wellformed_svg() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_entity_and_relationship_counts() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE : has";
        let r = render_er(src, &opts()).unwrap();
        // Three entities → at least three entity <rect> (label bg rects add more).
        assert!(r.svg.matches("<rect").count() >= 3);
        // Two relationships → two edge <path>s (fill="none"). Crow's-foot
        // circles also use fill="none", so count the <path…fill="none"> form.
        assert_eq!(r.svg.matches("<path d=").count(), 2);
        // Relationship labels present.
        assert!(r.svg.contains(">places<"));
        assert!(r.svg.contains(">has<"));
    }

    #[test]
    fn dashed_line_for_non_identifying() {
        let src = "erDiagram\n  A ||..o{ B";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains("stroke-dasharray=\"4 3\""));
    }

    #[test]
    fn solid_line_has_no_dash() {
        let src = "erDiagram\n  A ||--o{ B";
        let r = render_er(src, &opts()).unwrap();
        assert!(!r.svg.contains("stroke-dasharray"));
    }

    #[test]
    fn attribute_rows_rendered() {
        let src = "erDiagram\n  CUSTOMER {\n    string name PK\n  }\n  CUSTOMER ||--o{ ORDER : x";
        let r = render_er(src, &opts()).unwrap();
        // Attribute cells appear (type, name, key each in their own <text>).
        assert!(r.svg.contains(">string<"));
        assert!(r.svg.contains(">name<"));
        assert!(r.svg.contains(">PK<"));
        // Separator <line> under the header.
        assert!(r.svg.contains("<line"));
    }

    #[test]
    fn keys_and_comment_rendered() {
        let src = "erDiagram\n  CUSTOMER {\n    string name PK \"the name\"\n    int age\n  }";
        let r = render_er(src, &opts()).unwrap();
        // Key marker and comment text both present as their own cells.
        assert!(r.svg.contains(">PK<"), "PK key rendered: {}", r.svg);
        assert!(r.svg.contains(">the name<"), "comment rendered: {}", r.svg);
        // PK row is emphasized (bold) somewhere.
        assert!(r.svg.contains("font-weight=\"bold\""));
    }

    #[test]
    fn box_grows_to_fit_keys_and_comments() {
        // Same entity with vs. without keys/comment → the keyed box is wider.
        let bare = render_er("erDiagram\n  C {\n    string name\n  }", &opts()).unwrap();
        let keyed = render_er(
            "erDiagram\n  C {\n    string name PK \"a long-ish comment\"\n  }",
            &opts(),
        )
        .unwrap();
        assert!(
            keyed.width_px > bare.width_px,
            "keyed/comment box ({}) should be wider than bare ({})",
            keyed.width_px,
            bare.width_px,
        );
    }

    #[test]
    fn name_only_entity_has_no_attr_cells() {
        // An attribute-less entity renders unchanged: no separator line, no cells.
        let src = "erDiagram\n  LONE";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains(">LONE<"));
        assert!(!r.svg.contains("<line"), "no attr separator for name-only box");
        assert!(!r.svg.contains("font-weight=\"bold\""));
    }

    #[test]
    fn crows_foot_marks_drawn() {
        // `||` (exactly-one) → a double bar of perpendicular ticks; `o{`
        // (zero-or-many) → a crow's foot plus an open circle. No textual glyphs.
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER";
        let r = render_er(src, &opts()).unwrap();
        // Old textual cardinality glyphs are gone.
        assert!(!r.svg.contains(">1</text>"));
        assert!(!r.svg.contains(">0+</text>"));
        // The zero-or-many end draws an open circle.
        assert!(r.svg.contains("<circle"), "zero-cardinality end has a circle");
        // Crow's-foot / tick marks are <line> elements (beyond the relationship
        // <path>). There should be several.
        assert!(r.svg.matches("<line").count() >= 4, "expected tick/foot lines: {}", r.svg);
    }

    #[test]
    fn exactly_one_has_no_circle() {
        // `||--||` → both ends are exactly-one (double bars), no circles.
        let src = "erDiagram\n  A ||--|| B";
        let r = render_er(src, &opts()).unwrap();
        assert!(!r.svg.contains("<circle"), "exactly-one ends draw no circle");
    }

    #[test]
    fn zero_or_one_end_has_circle() {
        // `o|` → zero-or-one: an open circle marker.
        let src = "erDiagram\n  A o|--|| B";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains("<circle"));
    }

    #[test]
    fn xml_escapes_label() {
        let src = "erDiagram\n  A ||--o{ B : a & b < c";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains("a &amp; b &lt; c"));
        assert!(!r.svg.contains("a & b"));
    }

    #[test]
    fn empty_diagram_errors() {
        assert_eq!(render_er("erDiagram\n", &opts()), Err(MermaidError::Empty));
    }

    #[test]
    fn deterministic() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE : has";
        let a = render_er(src, &opts()).unwrap();
        let b = render_er(src, &opts()).unwrap();
        assert_eq!(a, b);
    }

    // ---- styling directives ----

    fn style_of<'a>(d: &'a ErDiagram, name: &str) -> &'a ElemStyle {
        &d.entities.iter().find(|e| e.name == name).expect("entity").style
    }

    #[test]
    fn classdef_and_class_apply() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  classDef big fill:#00f\n  class CUSTOMER big";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "CUSTOMER").fill, Some([0, 0, 255, 255]));
        // ORDER untouched.
        assert_eq!(style_of(&d, "ORDER").fill, None);
    }

    #[test]
    fn triple_colon_shorthand() {
        let src = "erDiagram\n  CUSTOMER:::big ||--o{ ORDER : places\n  classDef big fill:#00f";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "CUSTOMER").fill, Some([0, 0, 255, 255]));
        // Relationship recorded with bare names.
        assert_eq!(d.relationships[0].left, "CUSTOMER");
        assert_eq!(d.relationships[0].right, "ORDER");
        assert_eq!(d.relationships[0].label.as_deref(), Some("places"));
    }

    #[test]
    fn style_directive_overrides_class() {
        let src = "erDiagram\n  A ||--o{ B\n  classDef big fill:#00f\n  class A big\n  style A fill:#0f0";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "A").fill, Some([0, 255, 0, 255]));
    }

    #[test]
    fn style_override_in_rendered_svg() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  classDef big fill:#00f\n  class CUSTOMER big";
        let r = render_er(src, &opts()).unwrap();
        assert!(r.svg.contains(&rgb([0, 0, 255, 255])), "override fill present: {}", r.svg);
    }

    #[test]
    fn unstyled_entities_unchanged() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places";
        let d = parse(src).unwrap();
        for e in &d.entities {
            assert_eq!(e.style, ElemStyle::default());
        }
    }

    // ---- click / interaction ----

    #[test]
    fn click_sets_link_and_tooltip() {
        let src = "erDiagram\n CUSTOMER ||--o{ ORDER : places\n click CUSTOMER \"https://x\" \"tip\"\n";
        let d = parse(src).unwrap();
        let c = d.entities.iter().find(|e| e.name == "CUSTOMER").unwrap();
        assert_eq!(c.link.as_deref(), Some("https://x"));
        assert_eq!(c.tooltip.as_deref(), Some("tip"));
        assert!(c.callback.is_none());
        // Unknown id skipped, not fabricated.
        let d2 = parse("erDiagram\n CUSTOMER ||--o{ ORDER : places\n click GHOST \"https://y\"\n").unwrap();
        assert_eq!(d2.entities.len(), 2);
    }

    #[test]
    fn regions_carry_click_data() {
        let src = "erDiagram\n CUSTOMER ||--o{ ORDER : places\n click CUSTOMER \"https://x\" \"tip\"\n";
        let (render, regions) = render_er_with_regions(src, &opts()).unwrap();
        assert_eq!(regions.len(), 2);
        let c = regions.iter().find(|r| r.id == "CUSTOMER").unwrap();
        assert_eq!(c.link.as_deref(), Some("https://x"));
        assert_eq!(c.tooltip.as_deref(), Some("tip"));
        assert!(c.w > 0.0 && c.h > 0.0);
        assert!(c.x >= 0.0 && c.y >= 0.0);
        assert!(c.x + c.w <= render.width_px + 1.0);
        assert!(c.y + c.h <= render.height_px + 1.0);
        let o = regions.iter().find(|r| r.id == "ORDER").unwrap();
        assert!(o.link.is_none() && o.callback.is_none() && o.tooltip.is_none());
    }

    #[test]
    fn regions_without_click_and_svg_unchanged() {
        let src = "erDiagram\n CUSTOMER ||--o{ ORDER : places\n";
        let plain = render_er(src, &opts()).unwrap();
        let (with_regions, regions) = render_er_with_regions(src, &opts()).unwrap();
        assert_eq!(regions.len(), 2);
        assert!(regions.iter().all(|r| r.link.is_none()
            && r.callback.is_none()
            && r.tooltip.is_none()
            && r.w > 0.0
            && r.h > 0.0));
        assert_eq!(plain.svg, with_regions.svg);
        assert_eq!(plain.width_px, with_regions.width_px);
        assert_eq!(plain.height_px, with_regions.height_px);
    }
}
